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
use std::time::{Duration, Instant, SystemTime};

use countersign_bw::{
    AuthStatus, BwError, Cli, CliVersion, ListOptions, PINNED_VERSION, PinnedArchive, Serve, ServeClient,
    SessionKey,
};
use countersign_ipc::{
    Account, CliState, ClipboardEvent, ClipboardState, Collection, ErrorCode, Event, Folder, IpcError,
    ItemSummary, Organization, Settings, Status, VaultSnapshot, VaultState, summarize,
};
use secrecy::SecretString;
use serde_json::Value;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::clipboard::Clipboard;
use crate::paths::{Paths, ensure_private_dir};
use crate::unlisted::Unlisted;

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

/// Identifies one CLI executable: replacing or updating it changes this.
#[derive(Debug, PartialEq, Eq)]
struct CliFile {
    path: PathBuf,
    inode: u64,
    len: u64,
    modified: Option<SystemTime>,
}

impl CliFile {
    fn of(path: &std::path::Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(CliFile {
            path: path.to_owned(),
            inode: meta.ino(),
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

#[derive(Default)]
pub(crate) struct CopyTimers {
    pub clear: Option<JoinHandle<()>>,
    pub follow_up: Option<JoinHandle<()>>,
    /// When the current copy clears, and what it came from, so a skipped
    /// follow-up can re-announce the copy accurately.
    pub clear_at: Option<Instant>,
    pub item_name: Option<String>,
    /// Decides whether a pending code follow-up replaces the password or is
    /// cancelled; see `copy::FollowUpState`.
    pub follow_up_state: Option<Arc<crate::copy::FollowUpState>>,
}

impl CopyTimers {
    pub fn cancel(&mut self) {
        for task in [self.clear.take(), self.follow_up.take()].into_iter().flatten() {
            task.abort();
        }
        self.clear_at = None;
        self.item_name = None;
        self.follow_up_state = None;
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
    /// Serializes changes to the vault, so what each left is recorded in
    /// the order the CLI made them. The CLI answers one request at a time.
    changes: tokio::sync::Mutex<()>,
    cli: Mutex<Option<Cli>>,
    /// The last executable whose version passed, so `bw --version` runs once
    /// per file rather than on every refresh.
    checked_version: Mutex<Option<(CliFile, CliVersion)>>,
    session: Mutex<Option<Session>>,
    /// A CLI being unlocked, kept where `lock` can stop it.
    opening: Mutex<Option<Serve>>,
    prepared: Mutex<Option<(Serve, Instant)>>,
    epoch: Arc<AtomicU64>,
    vault: Mutex<Arc<VaultSnapshot>>,
    /// Changes the CLI's lists may not show yet; see `unlisted`.
    unlisted: Mutex<Unlisted>,
    pub(crate) clipboard: Arc<Clipboard>,
    pub(crate) copy_timers: Mutex<CopyTimers>,
    /// Orders clipboard events: a copy announces itself, and a timer checks
    /// that no newer copy started before reporting, under this lock.
    pub(crate) clipboard_events: Mutex<()>,
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
            changes: tokio::sync::Mutex::new(()),
            cli: Mutex::new(None),
            checked_version: Mutex::new(None),
            session: Mutex::new(None),
            opening: Mutex::new(None),
            prepared: Mutex::new(None),
            epoch: Arc::new(AtomicU64::new(0)),
            vault: Mutex::new(Arc::new(VaultSnapshot::default())),
            unlisted: Mutex::new(Unlisted::default()),
            clipboard: Arc::new(Clipboard::default()),
            copy_timers: Mutex::new(CopyTimers::default()),
            clipboard_events: Mutex::new(()),
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

    /// Applies new settings and tells every connected app.
    pub(crate) fn set_settings(&self, settings: Settings) {
        self.settings.send_replace(settings.clone());
        let _ = self.events.send(Event::Settings { settings });
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
        let cli_state = match self.version(&cli).await {
            Ok(version) => CliState::Ready {
                version: version.to_string(),
                managed,
                update: offered_update(version, managed).map(|v| v.to_string()),
            },
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

    /// The CLI's version, read once per executable file.
    async fn version(&self, cli: &Cli) -> Result<CliVersion, BwError> {
        let file = CliFile::of(cli.path());
        if let Some(file) = &file
            && let Some((checked, version)) = &*self.checked_version.lock().expect("version")
            && checked == file
        {
            return Ok(*version);
        }
        let version = cli.version().await?;
        if let Some(file) = file {
            *self.checked_version.lock().expect("version") = Some((file, version));
        }
        Ok(version)
    }

    /// Makes the next refresh read the CLI's version again.
    pub(crate) fn forget_version(&self) {
        *self.checked_version.lock().expect("version") = None;
    }

    // ----- Unlocking and locking ---------------------------------------

    /// Starts the CLI so an unlock does not wait for it. Runs under `ops`,
    /// so it never starts a CLI that an install or a change of CLI is
    /// replacing.
    pub async fn prepare_unlock(&self) -> Result<(), IpcError> {
        let _op = self.ops.lock().await;
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
        if let Some(unused) = self.keep_prepared(serve) {
            tokio::spawn(unused.shutdown());
        }
        Ok(())
    }

    /// Keeps a started CLI for the next unlock, and returns the one to stop:
    /// the one it replaces, or this one if an unlock began while it started.
    /// Runs under the session mutex, as `open` does, so an open never leaves
    /// an idle CLI behind.
    fn keep_prepared(&self, serve: Serve) -> Option<Serve> {
        let _slot = self.session.lock().expect("session");
        if self.status().vault != VaultState::Locked {
            return Some(serve);
        }
        let old = self.prepared.lock().expect("prepared").replace((serve, Instant::now()));
        old.map(|(old, _)| old)
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
        let cli = self.cli()?;
        let epoch = self.begin_unlocking();
        let serve = match self.take_prepared() {
            Some(serve) => serve,
            None => match Serve::start(&cli, None).await {
                Ok(serve) => serve,
                Err(e) => {
                    self.settle_locked(epoch);
                    return Err(ipc_error(e));
                }
            },
        };
        let client = self.stage(serve, epoch).await?;
        let key = match client.unlock(&password).await {
            Ok(key) => key,
            Err(e) => {
                // Keep the started CLI for the next attempt, unless a lock took it.
                if let Some(serve) = self.unstage(epoch) {
                    *self.prepared.lock().expect("prepared") = Some((serve, Instant::now()));
                }
                self.settle_locked(epoch);
                return Err(password_error(e));
            }
        };
        drop(password);
        self.open(key, epoch).await
    }

    /// Opens the vault with a session key printed by `bw login --raw`.
    pub async fn adopt_session(&self, key: SessionKey) -> Result<(), IpcError> {
        let _op = self.ops.lock().await;
        if self.status().vault != VaultState::Locked {
            return Ok(());
        }
        let cli = self.cli()?;
        let epoch = self.begin_unlocking();
        let client = match self.start_staged(&cli, &key, epoch).await {
            Ok(client) => client,
            Err(e) => {
                self.settle_locked(epoch);
                return Err(e);
            }
        };
        match client.status().await {
            Ok(s) if s.status == AuthStatus::Unlocked => self.open(key, epoch).await,
            _ => {
                if let Some(serve) = self.unstage(epoch) {
                    serve.shutdown().await;
                }
                self.settle_locked(epoch);
                Err(IpcError::new(ErrorCode::Locked, "Unlock to continue."))
            }
        }
    }

    /// Starts a CLI that opens with `key`, staged from the moment it exists
    /// so `lock` can stop it while it is still starting.
    async fn start_staged(&self, cli: &Cli, key: &SessionKey, epoch: u64) -> Result<ServeClient, IpcError> {
        let serve = Serve::spawn(cli, Some(key)).await.map_err(ipc_error)?;
        let client = self.stage(serve, epoch).await?;
        if let Err(e) = Serve::ready(&client).await {
            if let Some(serve) = self.unstage(epoch) {
                serve.shutdown().await;
            }
            return Err(ipc_error(e));
        }
        Ok(client)
    }

    /// Moves the staged CLI into the session, unless a lock took it.
    fn promote(&self, key: SessionKey, epoch: u64) -> Result<ServeClient, IpcError> {
        let mut slot = self.session.lock().expect("session");
        let staged = if self.epoch() == epoch { self.opening.lock().expect("opening").take() } else { None };
        let Some(serve) = staged else { return Err(locked_while_opening()) };
        let client = serve.client().clone();
        *slot = Some(Session { serve, key });
        Ok(client)
    }

    /// Marks the vault as unlocking and returns the epoch of this attempt.
    /// Runs under the session mutex, as `lock` does, so a lock either comes
    /// first or settles this attempt; it can never leave it stuck.
    fn begin_unlocking(&self) -> u64 {
        let _slot = self.session.lock().expect("session");
        self.update_status(|s| s.vault = VaultState::Unlocking);
        self.epoch()
    }

    /// Puts a CLI that is about to be unlocked where `lock` can stop it.
    async fn stage(&self, serve: Serve, epoch: u64) -> Result<ServeClient, IpcError> {
        let refused = {
            let _slot = self.session.lock().expect("session");
            if self.epoch() == epoch {
                let client = serve.client().clone();
                *self.opening.lock().expect("opening") = Some(serve);
                return Ok(client);
            }
            serve
        };
        refused.shutdown().await;
        Err(locked_while_opening())
    }

    /// Takes the staged CLI back, unless a lock already took it.
    fn unstage(&self, epoch: u64) -> Option<Serve> {
        let _slot = self.session.lock().expect("session");
        if self.epoch() != epoch {
            return None;
        }
        self.opening.lock().expect("opening").take()
    }

    /// Marks the vault locked after a failed unlock, unless a lock already
    /// settled the state.
    fn settle_locked(&self, epoch: u64) {
        let _slot = self.session.lock().expect("session");
        if self.epoch() == epoch {
            self.update_status(|s| {
                if s.vault == VaultState::Unlocking {
                    s.vault = VaultState::Locked;
                }
            });
        }
    }

    /// Moves the staged, unlocked CLI into the session, builds the index,
    /// then marks the vault open. A lock at any point wins.
    async fn open(&self, key: SessionKey, epoch: u64) -> Result<(), IpcError> {
        let client = self.promote(key, epoch)?;
        if let Err(e) = self.rebuild(&client, epoch, None).await {
            tracing::warn!("building the vault index failed: {e}");
        }
        self.touch();
        if let Some(unused) = self.mark_open(epoch)? {
            tokio::spawn(unused.shutdown());
        }
        self.emit(Event::VaultChanged);
        Ok(())
    }

    /// Marks the vault open, unless a lock came first, and returns the CLI
    /// started for an unlock this open made unneeded, such as one the
    /// unlock page asked for during a sign-in.
    fn mark_open(&self, epoch: u64) -> Result<Option<Serve>, IpcError> {
        let slot = self.session.lock().expect("session");
        if self.epoch() != epoch || slot.is_none() {
            return Err(locked_while_opening());
        }
        self.update_status(|s| s.vault = VaultState::Unlocked);
        Ok(self.prepared.lock().expect("prepared").take().map(|(serve, _)| serve))
    }

    /// Locks immediately. Safe to call from any task, any number of times.
    pub async fn lock(&self, reason: LockReason) {
        let was_unlocked = self.stop_session().await;
        // An unlock or restart in flight now fails; let it finish cleaning
        // up before reporting the vault locked.
        let _ = tokio::time::timeout(Duration::from_secs(2), self.ops.lock()).await;
        self.report_locked(was_unlocked, reason);
    }

    /// Locks from inside an operation, which already holds `ops`, so no
    /// unlock can be in flight.
    pub(crate) async fn lock_within_op(&self, reason: LockReason) {
        let was_unlocked = self.stop_session().await;
        self.report_locked(was_unlocked, reason);
    }

    /// Ends the session and stops every CLI it started. Returns whether the
    /// vault was open.
    async fn stop_session(&self) -> bool {
        let (session, opening, prepared) = {
            let mut slot = self.session.lock().expect("session");
            self.epoch.fetch_add(1, Ordering::SeqCst);
            self.grants.lock().expect("grants").clear();
            *self.vault.lock().expect("vault") = Arc::new(VaultSnapshot::default());
            *self.unlisted.lock().expect("unlisted") = Unlisted::default();
            self.update_status(|s| {
                if matches!(s.vault, VaultState::Unlocked | VaultState::Unlocking) {
                    s.vault = VaultState::Locked;
                }
                s.syncing = false;
            });
            (
                slot.take(),
                self.opening.lock().expect("opening").take(),
                self.prepared.lock().expect("prepared").take(),
            )
        };
        let was_unlocked = session.is_some();
        self.copy_timers.lock().expect("timers").cancel();
        let clipboard = self.clipboard.clone();
        let cleared = tokio::task::spawn_blocking(move || {
            clipboard.clear_if_ours(None) == crate::clipboard::Clear::Cleared
        })
        .await
        .unwrap_or(false);
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
        let unlocked = session.map(|Session { serve, .. }| serve).into_iter().chain(opening);
        for serve in unlocked {
            if tokio::time::timeout(Duration::from_secs(3), serve.client().lock()).await.is_err() {
                tracing::warn!("the CLI did not confirm the lock in time");
            }
            serve.shutdown().await;
        }
        if let Some((serve, _)) = prepared {
            serve.shutdown().await;
        }
        was_unlocked
    }

    fn report_locked(&self, was_unlocked: bool, reason: LockReason) {
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
        let (epoch, dead, key) = {
            let mut slot = self.session.lock().expect("session");
            let epoch = self.epoch();
            match slot.take() {
                None => return Err(self.closed_error()),
                Some(mut session) => {
                    // Another request may have restarted it while we waited.
                    if !session.serve.has_exited() {
                        let client = session.serve.client().clone();
                        *slot = Some(session);
                        return Ok(client);
                    }
                    let Session { serve, key } = session;
                    (epoch, serve, key)
                }
            }
        };
        dead.shutdown().await;
        tracing::warn!("the Bitwarden CLI stopped; restarting it");
        let cli = self.cli()?;
        match self.start_staged(&cli, &key, epoch).await {
            Ok(_) => self.promote(key, epoch),
            Err(e) => {
                drop(key);
                drop(_op);
                self.lock(LockReason::Requested).await;
                Err(e)
            }
        }
    }

    fn closed_error(&self) -> IpcError {
        match self.status().vault {
            VaultState::LoggedOut => IpcError::new(ErrorCode::LoggedOut, "Sign in first."),
            _ => IpcError::new(ErrorCode::Locked, "The vault is locked."),
        }
    }

    /// A copy of the session key, for the one-off Send commands that
    /// `bw serve` cannot run or answers stale; see `Cli::send_list`.
    pub(crate) fn session_key(&self) -> Result<SessionKey, IpcError> {
        self.session
            .lock()
            .expect("session")
            .as_ref()
            .map(|s| s.key.clone())
            .ok_or_else(|| self.closed_error())
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

    /// Builds the vault index from the CLI's lists. After a sync, `synced`
    /// is the mark taken when it started: once the lists from after the sync
    /// are read, what was remembered before it is retired, in the same step
    /// that lays the rest over them.
    pub(crate) async fn rebuild(
        &self,
        client: &ServeClient,
        epoch: u64,
        synced: Option<u64>,
    ) -> Result<(), BwError> {
        let _guard = self.rebuild.lock().await;
        let items = client.list_items(&ListOptions::default()).await?;
        let trash = client.list_items(&ListOptions { trash: true, ..Default::default() }).await?;
        let archived = client.list_items(&ListOptions { archived: true, ..Default::default() }).await?;
        // The trash list leaves out items that are also archived.
        let archived_trash =
            client.list_items(&ListOptions { trash: true, archived: true, ..Default::default() }).await?;
        let folders = client.list_folders().await?;
        let collections = client.list_collections().await?;
        let organizations = client.list_organizations().await?;
        let text = |v: &Value, k: &str| v[k].as_str().map(str::to_owned);
        let mut snapshot = VaultSnapshot {
            items: items.iter().filter_map(summarize).collect(),
            trash: trash.iter().filter_map(summarize).chain(only_archived(&archived_trash)).collect(),
            archived: only_archived(&archived).collect(),
            folders: folders.iter().filter_map(folder).collect(),
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
        drop(archived);
        drop(archived_trash);
        {
            let mut unlisted = self.unlisted.lock().expect("unlisted");
            if let Some(mark) = synced
                && self.epoch() == epoch
            {
                unlisted.forget_before(mark);
            }
            unlisted.apply(&mut snapshot);
        }
        let by_name = |a: &String, b: &String| a.to_lowercase().cmp(&b.to_lowercase());
        snapshot.items.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.trash.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.archived.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.folders.sort_by(|a, b| by_name(&a.name, &b.name));
        snapshot.collections.sort_by(|a, b| by_name(&a.name, &b.name));
        let _slot = self.session.lock().expect("session");
        if self.epoch.load(Ordering::SeqCst) == epoch {
            let ids: std::collections::HashSet<&str> = snapshot.items.iter().map(|i| i.id.as_str()).collect();
            self.recent.lock().expect("recent").retain(|r| ids.contains(r.as_str()));
            *self.vault.lock().expect("vault") = Arc::new(snapshot);
        }
        Ok(())
    }

    /// Runs a CLI call on an object the index knows, asking again for up to
    /// two seconds while the CLI answers "not found": `bw serve` can for a
    /// moment not find an item or folder it has just created. A call that
    /// found nothing changed nothing, so asking again is safe.
    pub(crate) async fn on_known<T, F>(&self, known: bool, mut call: impl FnMut() -> F) -> Result<T, BwError>
    where
        F: std::future::Future<Output = Result<T, BwError>>,
    {
        let mut attempts = 0;
        loop {
            match call().await {
                Err(BwError::NotFound) if known && attempts < 20 => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                result => return result,
            }
        }
    }

    /// Reads an item, asking again for up to two seconds while the CLI
    /// answers from before the last change; see `unlisted`. After that it
    /// gives what the CLI has.
    pub(crate) async fn read_item(&self, client: &ServeClient, id: &str) -> Result<Value, BwError> {
        let known = self.knows_item(id);
        let mut attempts = 0;
        loop {
            let item = self.on_known(known, || client.get_item(id)).await?;
            if attempts == 20 || !self.read_behind(id, &item) {
                return Ok(item);
            }
            attempts += 1;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Reads an item to build a change on. A change built on a read that is
    /// behind would undo the one before it, so that is an error here.
    pub(crate) async fn read_item_to_change(
        &self,
        client: &ServeClient,
        id: &str,
    ) -> Result<Value, IpcError> {
        let item = self.read_item(client, id).await.map_err(ipc_error)?;
        if self.read_behind(id, &item) {
            return Err(IpcError::new(ErrorCode::Busy, "This item is still saving. Try again in a moment."));
        }
        Ok(item)
    }

    fn read_behind(&self, id: &str, item: &Value) -> bool {
        self.unlisted.lock().expect("unlisted").behind(id, item["revisionDate"].as_str())
    }

    pub(crate) fn knows_item(&self, id: &str) -> bool {
        let vault = self.vault.lock().expect("vault").clone();
        vault.items.iter().chain(&vault.trash).chain(&vault.archived).any(|i| i.id == id)
    }

    pub(crate) fn knows_folder(&self, id: &str) -> bool {
        self.vault.lock().expect("vault").folders.iter().any(|f| f.id == id)
    }

    /// Syncs with the server and rebuilds the index.
    pub async fn sync(&self) -> Result<(), IpcError> {
        let client = self.client().await?;
        let epoch = self.epoch();
        self.update_status(|s| s.syncing = true);
        let mark = self.unlisted.lock().expect("unlisted").mark();
        let result = async {
            client.sync().await?;
            self.rebuild(&client, epoch, Some(mark)).await?;
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

    /// Starts a change to the vault: waits for any other change to finish
    /// and notes the session it belongs to.
    pub(crate) async fn begin_change(&self) -> ChangeGuard<'_> {
        let lock = self.changes.lock().await;
        ChangeGuard { _lock: lock, epoch: self.epoch() }
    }

    /// Rebuilds the index after a change and tells the app. What the change
    /// left is kept for the index first, because the CLI's lists can be
    /// behind; see `unlisted`.
    pub(crate) async fn changed(&self, guard: &ChangeGuard<'_>, client: &ServeClient, change: Change<'_>) {
        self.record(guard, change);
        if let Err(e) = self.rebuild(client, guard.epoch, None).await {
            tracing::warn!("refreshing the vault index failed: {e}");
        }
        self.emit(Event::VaultChanged);
    }

    /// Keeps what a change left, unless its session has ended since it
    /// started.
    pub(crate) fn record(&self, guard: &ChangeGuard<'_>, change: Change<'_>) {
        let slot = self.session.lock().expect("session");
        if slot.is_none() || self.epoch() != guard.epoch {
            return;
        }
        let indexed = |id: &str| {
            let vault = self.vault.lock().expect("vault").clone();
            vault.items.iter().chain(&vault.trash).chain(&vault.archived).find(|i| i.id == id).cloned()
        };
        let mut unlisted = self.unlisted.lock().expect("unlisted");
        match change {
            Change::Item(item) => {
                if let Some(summary) = summarize(item) {
                    unlisted.returned(summary);
                }
            }
            // Reading the item back can be behind too, so these follow from
            // what is known already.
            Change::Trashed(id) => unlisted.derive(&id, indexed(&id).as_ref(), |i| i.deleted = true),
            Change::Restored(id) => unlisted.derive(&id, indexed(&id).as_ref(), restored),
            Change::Archived(id) => unlisted.derive(&id, indexed(&id).as_ref(), |i| i.archived = true),
            Change::AttachmentDeleted(id) => unlisted.derive(&id, indexed(&id).as_ref(), |i| {
                i.attachments = i.attachments.saturating_sub(1);
            }),
            Change::Deleted(id) => unlisted.item(id, None),
            Change::Folder(id, folder) => {
                unlisted.folder(id, folder.and_then(self::folder));
            }
        }
    }
}

/// A change to the vault in progress; see `Daemon::begin_change`.
pub(crate) struct ChangeGuard<'a> {
    _lock: tokio::sync::MutexGuard<'a, ()>,
    epoch: u64,
}

/// What a change left behind, for the index; see `unlisted`.
pub(crate) enum Change<'a> {
    /// An item as the change returned it.
    Item(&'a Value),
    /// An item moved to the trash.
    Trashed(String),
    /// An item restored from the trash or the archive.
    Restored(String),
    /// An item moved to the archive.
    Archived(String),
    /// An item that lost one attachment.
    AttachmentDeleted(String),
    /// An item deleted for good.
    Deleted(String),
    /// A folder as the change returned it, or `None` once deleted.
    Folder(String, Option<&'a Value>),
}

/// The pinned release, when Countersign installed `version` itself, it is
/// older than the pin and Bitwarden publishes the pin for this machine.
fn offered_update(version: CliVersion, managed: bool) -> Option<CliVersion> {
    (managed && version < PINNED_VERSION && PinnedArchive::for_this_machine().is_some())
        .then_some(PINNED_VERSION)
}

fn locked_while_opening() -> IpcError {
    IpcError::new(ErrorCode::Locked, "The vault was locked while it was opening.")
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
/// The archived items in a list that asked for them. Without Premium the CLI
/// ignores the request and lists the others, which must not show as archived.
fn only_archived(list: &[Value]) -> impl Iterator<Item = ItemSummary> + '_ {
    list.iter().filter_map(summarize).filter(|i| i.archived)
}

/// What restoring leaves, as the CLI decides it: an item in the trash comes
/// out of the trash, and only one that is not leaves the archive.
fn restored(item: &mut ItemSummary) {
    if item.deleted {
        item.deleted = false;
    } else {
        item.archived = false;
    }
}

/// A folder as the CLI lists it. The CLI also lists its own "No Folder"
/// entry, with an empty id, which is no folder.
fn folder(value: &Value) -> Option<Folder> {
    let id = value["id"].as_str().filter(|id| !id.is_empty())?;
    Some(Folder { id: id.to_owned(), name: value["name"].as_str()?.to_owned() })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon() -> (Arc<Daemon>, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths {
            settings_file: root.path().join("settings.toml"),
            profile_dir: root.path().join("profile"),
            managed_cli_root: root.path().join("cli"),
            runtime_dir: root.path().join("run"),
            socket: root.path().join("run/daemon.sock"),
            lock_file: root.path().join("run/daemon.lock"),
        };
        (Daemon::new(paths, Settings::default()), root)
    }

    /// A lock during an unlock attempt settles it as locked, and the next
    /// attempt can start: the attempt is never left in Unlocking.
    #[tokio::test]
    async fn lock_while_unlocking_settles_locked() {
        let (daemon, _root) = daemon();
        daemon.update_status(|s| s.vault = VaultState::Locked);
        let epoch = daemon.begin_unlocking();
        assert_eq!(daemon.status().vault, VaultState::Unlocking);
        daemon.lock(LockReason::Requested).await;
        assert_eq!(daemon.status().vault, VaultState::Locked);
        // The stale attempt's cleanup leaves the lock's state alone.
        daemon.settle_locked(epoch);
        assert_eq!(daemon.status().vault, VaultState::Locked);
        assert_ne!(daemon.begin_unlocking(), epoch);
        assert_eq!(daemon.status().vault, VaultState::Unlocking);
        // A failed attempt in the current epoch settles back to Locked.
        daemon.settle_locked(daemon.epoch());
        assert_eq!(daemon.status().vault, VaultState::Locked);
    }

    /// A CLI started for an unlock is kept only while the vault is locked,
    /// and opening the vault stops the one kept: none sits idle beside the
    /// open session.
    #[tokio::test]
    async fn no_idle_cli_outlives_an_open() {
        let (daemon, root) = daemon();
        let bw = root.path().join("bw");
        write_script(&bw, "#!/bin/sh\nexec sleep 30\n");
        let cli = Cli::new(&bw, root.path().join("profile"));
        let spawn = || Serve::spawn(&cli, None);

        daemon.update_status(|s| s.vault = VaultState::Locked);
        assert!(daemon.keep_prepared(spawn().await.unwrap()).is_none());
        // A newer one replaces it.
        assert!(daemon.keep_prepared(spawn().await.unwrap()).is_some());
        // One that finishes starting after an unlock began is refused.
        let epoch = daemon.begin_unlocking();
        assert!(daemon.keep_prepared(spawn().await.unwrap()).is_some());
        assert!(daemon.prepared.lock().unwrap().is_some());

        let key = SessionKey::from_raw(format!("{}==", "a".repeat(86))).unwrap();
        *daemon.session.lock().unwrap() = Some(Session { serve: spawn().await.unwrap(), key });
        assert!(daemon.mark_open(epoch).unwrap().is_some());
        assert!(daemon.prepared.lock().unwrap().is_none());
        assert_eq!(daemon.status().vault, VaultState::Unlocked);
        assert!(daemon.keep_prepared(spawn().await.unwrap()).is_some());
        // A lock since the attempt began wins.
        daemon.epoch.fetch_add(1, Ordering::SeqCst);
        assert!(daemon.mark_open(epoch).is_err());
    }

    fn listed(id: &str, deleted: bool, archived: bool) -> Value {
        let date = |set: bool| if set { serde_json::json!("2026-11-06T10:00:00.000Z") } else { Value::Null };
        serde_json::json!({
            "id": id, "type": 2, "name": id, "secureNote": {"type": 0},
            "deletedDate": date(deleted), "archivedDate": date(archived),
        })
    }

    /// Without Premium the CLI answers a request for archived items with
    /// the others, so the archive would otherwise fill with the whole vault.
    #[test]
    fn only_archived_items_count_as_archived() {
        let list = [listed("live", false, false), listed("old", false, true)];
        let ids: Vec<String> = only_archived(&list).map(|i| i.id).collect();
        assert_eq!(ids, ["old"]);
    }

    #[test]
    fn restoring_follows_the_cli() {
        let mut archived = summarize(&listed("a", false, true)).unwrap();
        restored(&mut archived);
        assert!(!archived.archived);

        let mut trashed = summarize(&listed("t", true, false)).unwrap();
        restored(&mut trashed);
        assert!(!trashed.deleted);

        let mut both = summarize(&listed("b", true, true)).unwrap();
        restored(&mut both);
        assert!(!both.deleted && both.archived);
    }

    /// The CLI lists its own "No Folder" entry, with an empty id. The app has
    /// its own row for that, and the entry is no folder to rename or delete.
    #[test]
    fn the_clis_no_folder_entry_is_not_a_folder() {
        let listed = serde_json::json!({"object": "folder", "id": "", "name": "No Folder"});
        assert_eq!(folder(&listed), None);
        assert_eq!(folder(&serde_json::json!({"id": null, "name": "No Folder"})), None);
        let real = serde_json::json!({"id": "f1", "name": "Archive"});
        assert_eq!(folder(&real), Some(Folder { id: "f1".into(), name: "Archive".into() }));
    }

    /// Writes an executable script standing in for the CLI, from a child
    /// process. A file this process had open for writing could be inherited
    /// by a CLI another test is forking, and running it would then fail with
    /// "text file busy".
    fn write_script(path: &std::path::Path, script: &str) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut writer = Command::new("sh")
            .args(["-c", "cat >\"$0\" && chmod 700 \"$0\""])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        assert!(writer.wait().unwrap().success());
    }

    /// Stands in for the CLI: logs each call to the profile folder and
    /// remembers the server it is pointed at.
    const FAKE_CLI: &str = r#"#!/bin/sh
dir="$BITWARDENCLI_APPDATA_DIR"
echo "$*" >> "$dir/calls"
case "$1" in
    --version) echo 2026.10.1 ;;
    config) printf '%s' "$3" > "$dir/server"; echo 'Saved setting `config`.' ;;
    status)
        if [ -f "$dir/server" ]; then url="\"$(cat "$dir/server")\""; else url=null; fi
        printf '{"serverUrl":%s,"lastSync":null,"status":"unauthenticated"}' "$url"
        exec sleep 3 ;;
esac
"#;

    /// Switching servers runs `config server` and `status`; the version is
    /// read once per executable, and again when the user asks to check.
    #[tokio::test]
    async fn server_switch_runs_only_what_it_needs() {
        let (daemon, root) = daemon();
        let bw = root.path().join("bw");
        write_script(&bw, FAKE_CLI);
        daemon.set_settings(Settings { cli_path: Some(bw), ..Settings::default() });
        let calls = || std::fs::read_to_string(root.path().join("profile/calls")).unwrap();

        daemon.refresh().await;
        assert_eq!(daemon.status().vault, VaultState::LoggedOut);
        let started = Instant::now();
        let request = countersign_ipc::Request::SetServer { url: Some("https://vault.bitwarden.eu/".into()) };
        daemon.handle(request).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "waited for the CLI to exit");
        assert_eq!(daemon.status().server_url.as_deref(), Some("https://vault.bitwarden.eu"));
        assert_eq!(calls(), "--version\nstatus\nconfig server https://vault.bitwarden.eu\nstatus\n");

        daemon.handle(countersign_ipc::Request::Refresh).await.unwrap();
        assert!(calls().ends_with("status\n--version\nstatus\n"));
    }

    /// Settings offers the pinned release only in place of an older copy
    /// Countersign installed, never over a CLI you chose.
    #[tokio::test]
    async fn offers_the_pin_only_over_an_older_managed_copy() {
        use countersign_bw::MINIMUM_VERSION;
        let (daemon, root) = daemon();
        let older = MINIMUM_VERSION.to_string();
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo {older} ;;\nstatus) echo '{{\"status\":\"unauthenticated\"}}' ;;\nesac\n"
        );
        let bw = daemon.paths.managed_cli(&older);
        std::fs::create_dir_all(bw.parent().unwrap()).unwrap();
        write_script(&bw, &script);

        daemon.refresh().await;
        let expected = (MINIMUM_VERSION < PINNED_VERSION && PinnedArchive::for_this_machine().is_some())
            .then(|| PINNED_VERSION.to_string());
        assert_eq!(
            daemon.status().cli,
            CliState::Ready { version: older.clone(), managed: true, update: expected }
        );

        let own = root.path().join("bw");
        write_script(&own, &script);
        daemon.set_settings(Settings { cli_path: Some(own), ..Settings::default() });
        daemon.refresh().await;
        assert_eq!(daemon.status().cli, CliState::Ready { version: older, managed: false, update: None });
    }

    /// An operation that locks does not wait for itself to finish.
    #[tokio::test]
    async fn locking_within_an_operation_does_not_wait_for_it() {
        let (daemon, _root) = daemon();
        daemon.update_status(|s| s.vault = VaultState::Unlocking);
        let _op = daemon.ops.lock().await;
        let started = Instant::now();
        daemon.lock_within_op(LockReason::Requested).await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(daemon.status().vault, VaultState::Locked);
    }

    /// The locked page asks for a CLI as soon as an install locks the
    /// vault. It waits for the install to switch CLIs and starts the new
    /// one, never the copy being replaced.
    #[tokio::test]
    async fn preparing_waits_for_a_cli_switch() {
        let (daemon, root) = daemon();
        let fake = |name: &str| {
            let bw = root.path().join(name);
            let log = root.path().join("started");
            write_script(&bw, &format!("#!/bin/sh\necho {name} >> {}\n", log.display()));
            Cli::new(&bw, root.path().join("profile"))
        };
        *daemon.cli.lock().unwrap() = Some(fake("old"));
        daemon.update_status(|s| s.vault = VaultState::Locked);

        let op = daemon.ops.lock().await;
        let preparing = tokio::spawn({
            let daemon = daemon.clone();
            async move { daemon.prepare_unlock().await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        *daemon.cli.lock().unwrap() = Some(fake("new"));
        drop(op);
        // The fake CLI exits at once, so starting it fails; only which one
        // started matters.
        let _ = preparing.await.unwrap();
        assert_eq!(std::fs::read_to_string(root.path().join("started")).unwrap(), "new\n");
    }
}
