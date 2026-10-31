//! The daemon's state: the CLI, the unlocked session and the vault index.
//!
//! Locking never waits for other work. It takes the session out first, so
//! no request can use it, then asks the CLI to lock and stops it. Work that
//! was in flight fails with a closed connection. An epoch counter stops a
//! slow unlock, sync or index rebuild from storing results after a lock.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use countersign_bw::{AuthStatus, BwError, Cli, ListOptions, PINNED_VERSION, Serve, ServeClient, SessionKey};
use countersign_ipc::{
    Account, CliState, ClipboardEvent, ClipboardState, Collection, ErrorCode, Event, Folder, IpcError,
    Organization, Settings, Status, VaultSnapshot, VaultState, summarize,
};
use secrecy::SecretString;
use serde_json::Value;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::clipboard::Clipboard;
use crate::paths::{Paths, ensure_private_dir};

/// How long a re-prompted item stays available without asking again.
const REPROMPT_GRANT: Duration = Duration::from_secs(300);
/// How long a CLI started ahead of an unlock waits to be used.
const PREPARED_TTL: Duration = Duration::from_secs(120);
const RECENT_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockReason {
    Requested,
    Idle,
    ScreenLocked,
    Sleep,
    Shutdown,
    SignedOut,
}

struct Session {
    serve: Serve,
    key: SessionKey,
}

#[derive(Default)]
pub(crate) struct CopyTimers {
    pub clear: Option<JoinHandle<()>>,
    pub follow_up: Option<JoinHandle<()>>,
}

impl CopyTimers {
    pub fn cancel(&mut self) {
        for task in [self.clear.take(), self.follow_up.take()].into_iter().flatten() {
            task.abort();
        }
    }
}

pub struct Daemon {
    pub paths: Paths,
    events: broadcast::Sender<Event>,
    status: watch::Sender<Status>,
    settings: watch::Sender<Settings>,
    /// Serializes changes of sign-in and lock state.
    pub(crate) ops: tokio::sync::Mutex<()>,
    /// Serializes rebuilding the vault index.
    rebuild: tokio::sync::Mutex<()>,
    cli: Mutex<Option<Cli>>,
    session: Mutex<Option<Session>>,
    prepared: Mutex<Option<(Serve, Instant)>>,
    epoch: Arc<AtomicU64>,
    vault: Mutex<Arc<VaultSnapshot>>,
    pub(crate) clipboard: Arc<Clipboard>,
    pub(crate) copy_timers: Mutex<CopyTimers>,
    activity: Mutex<Instant>,
    grants: Mutex<HashMap<String, Instant>>,
    recent: Mutex<VecDeque<String>>,
    clients: AtomicUsize,
    last_client: Mutex<Instant>,
}

impl Daemon {
    pub fn new(paths: Paths, settings: Settings) -> Arc<Self> {
        let status = Status {
            cli: CliState::Checking,
            vault: VaultState::Unknown,
            account: None,
            server_url: None,
            last_sync: None,
            syncing: false,
        };
        Arc::new(Daemon {
            paths,
            events: broadcast::channel(256).0,
            status: watch::channel(status).0,
            settings: watch::channel(settings).0,
            ops: tokio::sync::Mutex::new(()),
            rebuild: tokio::sync::Mutex::new(()),
            cli: Mutex::new(None),
            session: Mutex::new(None),
            prepared: Mutex::new(None),
            epoch: Arc::new(AtomicU64::new(0)),
            vault: Mutex::new(Arc::new(VaultSnapshot::default())),
            clipboard: Arc::new(Clipboard::default()),
            copy_timers: Mutex::new(CopyTimers::default()),
            activity: Mutex::new(Instant::now()),
            grants: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::new()),
            clients: AtomicUsize::new(0),
            last_client: Mutex::new(Instant::now()),
        })
    }

    // ----- Status, settings and events ---------------------------------

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<Status> {
        self.status.subscribe()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    pub fn subscribe_settings(&self) -> watch::Receiver<Settings> {
        self.settings.subscribe()
    }

    pub(crate) fn set_settings(&self, settings: Settings) {
        self.settings.send_replace(settings);
    }

    pub(crate) fn update_status(&self, change: impl FnOnce(&mut Status)) {
        self.status.send_modify(change);
        let _ = self.events.send(Event::Status { status: self.status() });
    }

    pub(crate) fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn is_unlocked(&self) -> bool {
        self.status.borrow().vault == VaultState::Unlocked
    }

    pub fn touch(&self) {
        *self.activity.lock().expect("activity") = Instant::now();
    }

    pub fn idle_for(&self) -> Duration {
        self.activity.lock().expect("activity").elapsed()
    }

    pub fn client_connected(&self) {
        self.clients.fetch_add(1, Ordering::SeqCst);
    }

    pub fn client_disconnected(&self) {
        self.clients.fetch_sub(1, Ordering::SeqCst);
        *self.last_client.lock().expect("clients") = Instant::now();
    }

    /// True when nothing needs the daemon: no app is connected, the vault is
    /// not open and none of our values are on the clipboard.
    pub fn can_exit(&self, grace: Duration) -> bool {
        self.clients.load(Ordering::SeqCst) == 0
            && self.last_client.lock().expect("clients").elapsed() >= grace
            && !self.is_unlocked()
            && self.copy_timers.lock().expect("timers").clear.as_ref().is_none_or(JoinHandle::is_finished)
    }

    // ----- The CLI ------------------------------------------------------

    pub(crate) fn cli(&self) -> Result<Cli, IpcError> {
        self.cli
            .lock()
            .expect("cli")
            .clone()
            .ok_or_else(|| IpcError::new(ErrorCode::CliUnavailable, "The Bitwarden CLI is not ready."))
    }

    /// The configured CLI, else the managed copy, else `bw` on `PATH`.
    fn resolve_cli(&self) -> Option<(PathBuf, bool)> {
        if let Some(path) = self.settings().cli_path {
            return Some((path, false));
        }
        let pinned = self.paths.managed_cli(&PINNED_VERSION.to_string());
        if pinned.is_file() {
            return Some((pinned, true));
        }
        if let Ok(entries) = std::fs::read_dir(&self.paths.managed_cli_root) {
            let mut versions: Vec<(countersign_bw::CliVersion, PathBuf)> = entries
                .flatten()
                .filter_map(|e| {
                    let version = countersign_bw::CliVersion::parse(e.file_name().to_str()?)?;
                    let bw = e.path().join("bw");
                    bw.is_file().then_some((version, bw))
                })
                .collect();
            versions.sort();
            if let Some((_, path)) = versions.pop() {
                return Some((path, true));
            }
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|dir| dir.join("bw")).find(|p| p.is_file()).map(|p| (p, false))
    }

    /// Rechecks the CLI version and the account's sign-in state.
    pub async fn refresh(&self) {
        if let Err(e) = ensure_private_dir(&self.paths.profile_dir) {
            self.update_status(|s| s.cli = CliState::Failed { message: e.to_string() });
            return;
        }
        let Some((path, managed)) = self.resolve_cli() else {
            *self.cli.lock().expect("cli") = None;
            self.update_status(|s| {
                s.cli = CliState::Missing;
                s.vault = VaultState::Unknown;
            });
            return;
        };
        let cli = Cli::new(path, &self.paths.profile_dir);
        let cli_state = match cli.version().await {
            Ok(version) => CliState::Ready { version: version.to_string(), managed },
            Err(BwError::CliMissing(_)) => CliState::Missing,
            Err(BwError::Unsupported { found, minimum }) => {
                CliState::Unsupported { found, minimum: minimum.to_string() }
            }
            Err(e) => CliState::Failed { message: e.to_string() },
        };
        if !matches!(cli_state, CliState::Ready { .. }) {
            *self.cli.lock().expect("cli") = None;
            self.update_status(|s| {
                s.cli = cli_state;
                s.vault = VaultState::Unknown;
            });
            return;
        }
        *self.cli.lock().expect("cli") = Some(cli.clone());
        if self.is_unlocked() {
            self.update_status(|s| s.cli = cli_state);
            return;
        }
        match cli.status().await {
            Ok(status) => {
                let vault = match status.status {
                    AuthStatus::Unauthenticated => VaultState::LoggedOut,
                    // Without our session key the CLI cannot be unlocked.
                    AuthStatus::Locked | AuthStatus::Unlocked => VaultState::Locked,
                };
                let account = (vault != VaultState::LoggedOut).then(|| Account {
                    email: status.user_email.clone(),
                    server_url: status.server_url.clone(),
                });
                self.update_status(|s| {
                    s.cli = cli_state;
                    if s.vault != VaultState::Unlocked {
                        s.vault = vault;
                    }
                    s.account = account;
                    s.server_url = status.server_url;
                    s.last_sync = status.last_sync;
                });
            }
            Err(e) => self.update_status(|s| {
                s.cli = CliState::Failed { message: e.to_string() };
                s.vault = VaultState::Unknown;
            }),
        }
    }

    // ----- Unlocking and locking ---------------------------------------

    /// Starts the CLI so an unlock does not wait for it.
    pub async fn prepare_unlock(&self) -> Result<(), IpcError> {
        if self.status().vault != VaultState::Locked {
            return Ok(());
        }
        let fresh = self
            .prepared
            .lock()
            .expect("prepared")
            .as_mut()
            .is_some_and(|(serve, at)| at.elapsed() < PREPARED_TTL && !serve.has_exited());
        if fresh {
            return Ok(());
        }
        let cli = self.cli()?;
        let serve = Serve::start(&cli, None).await.map_err(ipc_error)?;
        let old = self.prepared.lock().expect("prepared").replace((serve, Instant::now()));
        if let Some((old, _)) = old {
            tokio::spawn(old.shutdown());
        }
        Ok(())
    }

    fn take_prepared(&self) -> Option<Serve> {
        let (mut serve, at) = self.prepared.lock().expect("prepared").take()?;
        if at.elapsed() < PREPARED_TTL && !serve.has_exited() {
            Some(serve)
        } else {
            tokio::spawn(serve.shutdown());
            None
        }
    }

    pub async fn unlock(&self, password: SecretString) -> Result<(), IpcError> {
        let _op = self.ops.lock().await;
        match self.status().vault {
            VaultState::Locked => {}
            VaultState::Unlocked => return Ok(()),
            VaultState::LoggedOut => return Err(IpcError::new(ErrorCode::LoggedOut, "Sign in first.")),
            _ => return Err(IpcError::new(ErrorCode::Busy, "Countersign is still starting.")),
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.update_status(|s| s.vault = VaultState::Unlocking);
        let cli = self.cli()?;
        let serve = match self.take_prepared() {
            Some(serve) => serve,
            None => match Serve::start(&cli, None).await {
                Ok(serve) => serve,
                Err(e) => {
                    self.update_status(|s| s.vault = VaultState::Locked);
                    return Err(ipc_error(e));
                }
            },
        };
        let key = match serve.client().unlock(&password).await {
            Ok(key) => key,
            Err(e) => {
                // Keep the started CLI for the next attempt.
                *self.prepared.lock().expect("prepared") = Some((serve, Instant::now()));
                self.update_status(|s| s.vault = VaultState::Locked);
                return Err(password_error(e));
            }
        };
        drop(password);
        self.install_session(serve, key, epoch).await
    }

    /// Opens the vault with a session key printed by `bw login --raw`.
    pub async fn adopt_session(&self, key: SessionKey) -> Result<(), IpcError> {
        let _op = self.ops.lock().await;
        if self.status().vault != VaultState::Locked {
            return Ok(());
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let cli = self.cli()?;
        let serve = Serve::start(&cli, Some(&key)).await.map_err(ipc_error)?;
        match serve.client().status().await {
            Ok(s) if s.status == AuthStatus::Unlocked => self.install_session(serve, key, epoch).await,
            _ => {
                serve.shutdown().await;
                Err(IpcError::new(ErrorCode::Locked, "Unlock to continue."))
            }
        }
    }

    async fn install_session(&self, serve: Serve, key: SessionKey, epoch: u64) -> Result<(), IpcError> {
        // Build the index before the vault counts as open.
        let client = serve.client().clone();
        if let Err(e) = self.rebuild(&client, epoch).await {
            tracing::warn!("building the vault index failed: {e}");
        }
        self.touch();
        match self.commit_session(Session { serve, key }, epoch, true) {
            Ok(()) => {
                self.emit(Event::VaultChanged);
                Ok(())
            }
            Err(Session { serve, key }) => {
                let _ = tokio::time::timeout(Duration::from_secs(3), serve.client().lock()).await;
                serve.shutdown().await;
                drop(key);
                self.update_status(|s| {
                    if s.vault == VaultState::Unlocking {
                        s.vault = VaultState::Locked;
                    }
                });
                Err(IpcError::new(ErrorCode::Locked, "The vault was locked while it was opening."))
            }
        }
    }

    /// Stores a session unless a lock happened since `epoch`. `lock` bumps
    /// the epoch under the same mutex, so the check and the store cannot be
    /// separated by a lock. Hands the session back when refused.
    #[allow(clippy::result_large_err, reason = "hands the refused session back to be shut down")]
    fn commit_session(&self, session: Session, epoch: u64, open: bool) -> Result<(), Session> {
        let mut slot = self.session.lock().expect("session");
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Err(session);
        }
        *slot = Some(session);
        if open {
            self.update_status(|s| s.vault = VaultState::Unlocked);
        }
        Ok(())
    }

    /// Locks immediately. Safe to call from any task, any number of times.
    pub async fn lock(&self, reason: LockReason) {
        let session = {
            let mut slot = self.session.lock().expect("session");
            self.epoch.fetch_add(1, Ordering::SeqCst);
            self.grants.lock().expect("grants").clear();
            self.update_status(|s| {
                if matches!(s.vault, VaultState::Unlocked | VaultState::Unlocking) {
                    s.vault = VaultState::Locked;
                }
                s.syncing = false;
            });
            slot.take()
        };
        let prepared = self.prepared.lock().expect("prepared").take();
        *self.vault.lock().expect("vault") = Arc::new(VaultSnapshot::default());
        let was_unlocked = session.is_some();
        self.copy_timers.lock().expect("timers").cancel();
        let clipboard = self.clipboard.clone();
        let cleared =
            tokio::task::spawn_blocking(move || clipboard.clear_if_ours(None)).await.unwrap_or(false);
        if cleared {
            self.emit(Event::Clipboard {
                clipboard: ClipboardEvent {
                    state: ClipboardState::Cleared,
                    label: "Clipboard cleared".into(),
                    item_name: None,
                    clears_in: None,
                    follow_up_in: None,
                },
            });
        }
        if let Some(Session { serve, key }) = session {
            if tokio::time::timeout(Duration::from_secs(3), serve.client().lock()).await.is_err() {
                tracing::warn!("the CLI did not confirm the lock in time");
            }
            serve.shutdown().await;
            drop(key);
        }
        if let Some((serve, _)) = prepared {
            serve.shutdown().await;
        }
        if was_unlocked {
            tracing::info!("vault locked ({reason:?})");
            self.emit(Event::VaultChanged);
        }
    }

    // ----- Using the session -------------------------------------------

    /// A client for the unlocked CLI. Restarts the CLI with the session key
    /// if it stopped on its own.
    pub(crate) async fn client(&self) -> Result<ServeClient, IpcError> {
        {
            let mut session = self.session.lock().expect("session");
            match session.as_mut() {
                None => return Err(self.closed_error()),
                Some(s) => {
                    if !s.serve.has_exited() {
                        return Ok(s.serve.client().clone());
                    }
                }
            }
        }
        let _op = self.ops.lock().await;
        let epoch = self.epoch.load(Ordering::SeqCst);
        let Some(Session { serve, key }) = self.session.lock().expect("session").take() else {
            return Err(self.closed_error());
        };
        serve.shutdown().await;
        tracing::warn!("the Bitwarden CLI stopped; restarting it");
        let cli = self.cli()?;
        let serve = match Serve::start(&cli, Some(&key)).await {
            Ok(serve) => serve,
            Err(e) => {
                drop(key);
                self.lock(LockReason::Requested).await;
                return Err(ipc_error(e));
            }
        };
        let client = serve.client().clone();
        if let Err(Session { serve, .. }) = self.commit_session(Session { serve, key }, epoch, false) {
            serve.shutdown().await;
            return Err(self.closed_error());
        }
        Ok(client)
    }

    fn closed_error(&self) -> IpcError {
        match self.status().vault {
            VaultState::LoggedOut => IpcError::new(ErrorCode::LoggedOut, "Sign in first."),
            _ => IpcError::new(ErrorCode::Locked, "The vault is locked."),
        }
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// The epoch counter itself, for checks that run off the async runtime.
    pub(crate) fn epoch_counter(&self) -> Arc<AtomicU64> {
        self.epoch.clone()
    }

    /// Whether a re-prompt was answered recently for this item.
    pub(crate) fn has_grant(&self, id: &str) -> bool {
        self.grants.lock().expect("grants").get(id).is_some_and(|at| at.elapsed() < REPROMPT_GRANT)
    }

    /// Checks the master password again for items that require it, and
    /// adopts the session key the CLI rotates in the process.
    pub async fn verify_password(&self, item_id: &str, password: SecretString) -> Result<(), IpcError> {
        let client = self.client().await?;
        let _op = self.ops.lock().await;
        let epoch = self.epoch();
        let key = client.unlock(&password).await.map_err(password_error)?;
        drop(password);
        // A lock during the check clears grants under this mutex; a stale
        // answer must not restore them.
        let mut slot = self.session.lock().expect("session");
        match slot.as_mut() {
            Some(session) if self.epoch() == epoch => {
                session.key = key;
                self.grants.lock().expect("grants").insert(item_id.to_owned(), Instant::now());
                Ok(())
            }
            _ => Err(IpcError::new(ErrorCode::Locked, "The vault is locked.")),
        }
    }

    /// Refuses access to an item that requires the master password unless
    /// it was entered recently for that item.
    pub(crate) fn check_reprompt(&self, id: &str, item: &Value) -> Result<(), IpcError> {
        if item["reprompt"].as_i64() != Some(1) {
            return Ok(());
        }
        if self.has_grant(id) {
            Ok(())
        } else {
            Err(IpcError::new(ErrorCode::RepromptRequired, "Enter your master password to use this item."))
        }
    }

    // ----- The vault index ---------------------------------------------

    pub fn vault(&self) -> VaultSnapshot {
        let mut snapshot = (**self.vault.lock().expect("vault")).clone();
        snapshot.recent = self.recent.lock().expect("recent").iter().cloned().collect();
        snapshot
    }

    pub(crate) fn note_recent(&self, id: &str) {
        let mut recent = self.recent.lock().expect("recent");
        recent.retain(|r| r != id);
        recent.push_front(id.to_owned());
        recent.truncate(RECENT_LIMIT);
    }

    /// Rebuilds the metadata index from the CLI.
    pub(crate) async fn rebuild(&self, client: &ServeClient, epoch: u64) -> Result<(), BwError> {
        let _guard = self.rebuild.lock().await;
        let items = client.list_items(&ListOptions::default()).await?;
        let trash = client.list_items(&ListOptions { trash: true, ..Default::default() }).await?;
        let folders = client.list_folders().await?;
        let collections = client.list_collections().await?;
        let organizations = client.list_organizations().await?;
        let text = |v: &Value, k: &str| v[k].as_str().map(str::to_owned);
        let mut snapshot = VaultSnapshot {
            items: items.iter().filter_map(summarize).collect(),
            trash: trash.iter().filter_map(summarize).collect(),
            folders: folders
                .iter()
                .filter_map(|f| Some(Folder { id: text(f, "id")?, name: text(f, "name")? }))
                .collect(),
            collections: collections
                .iter()
                .filter_map(|c| {
                    Some(Collection {
                        id: text(c, "id")?,
                        organization_id: text(c, "organizationId")?,
                        name: text(c, "name")?,
                    })
                })
                .collect(),
            organizations: organizations
                .iter()
                .filter_map(|o| Some(Organization { id: text(o, "id")?, name: text(o, "name")? }))
                .collect(),
            recent: Vec::new(),
        };
        drop(items);
        drop(trash);
        let by_name = |a: &String, b: &String| a.to_lowercase().cmp(&b.to_lowercase());
        snapshot.items.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.trash.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.folders.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.collections.sort_by(|a, b| by_name(&a.name, &b.name));
        if self.epoch.load(Ordering::SeqCst) == epoch {
            let ids: std::collections::HashSet<&str> = snapshot.items.iter().map(|i| i.id.as_str()).collect();
            self.recent.lock().expect("recent").retain(|r| ids.contains(r.as_str()));
            *self.vault.lock().expect("vault") = Arc::new(snapshot);
        }
        Ok(())
    }

    /// Syncs with the server and rebuilds the index.
    pub async fn sync(&self) -> Result<(), IpcError> {
        let client = self.client().await?;
        let epoch = self.epoch();
        self.update_status(|s| s.syncing = true);
        let result = async {
            client.sync().await?;
            self.rebuild(&client, epoch).await?;
            client.status().await
        }
        .await;
        if self.epoch() != epoch {
            return Err(self.closed_error());
        }
        match result {
            Ok(status) => {
                self.update_status(|s| {
                    s.syncing = false;
                    s.last_sync = status.last_sync;
                });
                self.emit(Event::VaultChanged);
                Ok(())
            }
            Err(e) => {
                self.update_status(|s| s.syncing = false);
                Err(ipc_error(e))
            }
        }
    }

    /// Rebuilds the index after a change and tells the app.
    pub(crate) async fn changed(&self, client: &ServeClient) {
        let epoch = self.epoch();
        if let Err(e) = self.rebuild(client, epoch).await {
            tracing::warn!("refreshing the vault index failed: {e}");
        }
        self.emit(Event::VaultChanged);
    }
}

/// Failures from checking a master password. The CLI reports a wrong
/// password differently when the vault is already unlocked.
pub(crate) fn password_error(error: BwError) -> IpcError {
    match &error {
        BwError::Cli(message)
            if message.contains("Invalid master password")
                || message.contains("decryption operation failed") =>
        {
            IpcError::new(ErrorCode::WrongPassword, "That master password is incorrect.")
        }
        _ => ipc_error(error),
    }
}

/// Translates CLI failures into messages the app can act on.
pub(crate) fn ipc_error(error: BwError) -> IpcError {
    match error {
        BwError::Locked => IpcError::new(ErrorCode::Locked, "The vault is locked."),
        BwError::NotLoggedIn => IpcError::new(ErrorCode::LoggedOut, "Sign in first."),
        BwError::NotFound => IpcError::new(ErrorCode::NotFound, "The item no longer exists."),
        BwError::CliMissing(_) | BwError::Unsupported { .. } => {
            IpcError::new(ErrorCode::CliUnavailable, error.to_string())
        }
        BwError::Cli(message) if message.contains("Premium") => IpcError::new(ErrorCode::Premium, message),
        BwError::Cli(message) => IpcError::new(ErrorCode::Cli, message),
        BwError::Disconnected => IpcError::new(ErrorCode::Locked, "The vault is locked."),
        other => IpcError::new(ErrorCode::Internal, other.to_string()),
    }
}
