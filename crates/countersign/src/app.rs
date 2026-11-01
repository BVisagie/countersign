//! Application state shared by the main window and the quick panel.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use countersign_ipc::{
    ClipboardEvent, ClipboardState, ErrorCode, Event, Field, IpcError, PROTOCOL_VERSION, Request, Response,
    Secret, Settings, Status, VaultSnapshot, VaultState,
};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::demo::Demo;
use crate::socket::SocketClient;

#[derive(Clone)]
enum Backend {
    Socket(Rc<SocketClient>),
    Demo(Rc<Demo>),
}

/// Something that shows app state: the main window or the quick panel.
pub trait View {
    fn status_changed(&self, status: &Status);
    fn vault_changed(&self, vault: &Rc<VaultSnapshot>);
    fn clipboard(&self, event: &ClipboardEvent);
}

pub struct App {
    pub gtk: adw::Application,
    backend: RefCell<Option<Backend>>,
    connecting: Cell<bool>,
    events: async_channel::Sender<Option<Event>>,
    pub status: RefCell<Status>,
    pub vault: RefCell<Rc<VaultSnapshot>>,
    /// Recently copied item ids, newest first.
    pub recent: RefCell<Vec<String>>,
    pub settings: RefCell<Settings>,
    views: RefCell<Vec<Weak<dyn View>>>,
    vault_reload: RefCell<Option<glib::SourceId>>,
    last_activity: Cell<Option<std::time::Instant>>,
}

impl App {
    pub fn new(gtk: adw::Application, demo: Option<bool>) -> Rc<Self> {
        let (events, incoming) = async_channel::unbounded();
        let demo = demo.map(|unlocked| Rc::new(Demo::new(events.clone(), unlocked)));
        let app = Rc::new(App {
            gtk,
            backend: RefCell::new(demo.clone().map(Backend::Demo)),
            connecting: Cell::new(false),
            events,
            status: RefCell::new(Status {
                cli: countersign_ipc::CliState::Checking,
                vault: VaultState::Unknown,
                account: None,
                server_url: None,
                last_sync: None,
                syncing: false,
            }),
            vault: RefCell::new(Rc::new(VaultSnapshot::default())),
            recent: RefCell::default(),
            settings: RefCell::new(Settings::default()),
            views: RefCell::new(Vec::new()),
            vault_reload: RefCell::new(None),
            last_activity: Cell::new(None),
        });
        let weak = Rc::downgrade(&app);
        glib::spawn_future_local(async move {
            while let Ok(event) = incoming.recv().await {
                let Some(app) = weak.upgrade() else { break };
                app.handle_event(event);
            }
        });
        app
    }

    pub fn add_view(&self, view: Weak<dyn View>) {
        self.views.borrow_mut().push(view);
    }

    fn each_view(&self, f: impl Fn(&dyn View)) {
        let views: Vec<Rc<dyn View>> = self.views.borrow().iter().filter_map(Weak::upgrade).collect();
        for view in views {
            f(view.as_ref());
        }
        self.views.borrow_mut().retain(|v| v.strong_count() > 0);
    }

    /// Connects (starting the daemon if needed) and loads the initial state.
    pub async fn start(self: &Rc<Self>) {
        match self.request(Request::Hello { protocol: PROTOCOL_VERSION }).await {
            Ok(Response::Hello { status, .. }) => self.set_status(status),
            Ok(_) => {}
            Err(e) => self.set_status(Status {
                cli: countersign_ipc::CliState::Failed { message: e.message },
                vault: VaultState::Unknown,
                account: None,
                server_url: None,
                last_sync: None,
                syncing: false,
            }),
        }
        if let Ok(Response::Settings { settings }) = self.request(Request::Settings).await {
            *self.settings.borrow_mut() = settings;
        }
    }

    async fn backend(self: &Rc<Self>) -> Result<Backend, IpcError> {
        if let Some(backend) = self.backend.borrow().clone() {
            match &backend {
                Backend::Socket(s) if !s.is_alive() => {}
                _ => return Ok(backend),
            }
        }
        while self.connecting.get() {
            glib::timeout_future(Duration::from_millis(20)).await;
            if let Some(backend) = self.backend.borrow().clone() {
                return Ok(backend);
            }
        }
        self.connecting.set(true);
        let result = SocketClient::connect(self.events.clone()).await;
        self.connecting.set(false);
        let client = result?;
        let backend = Backend::Socket(client);
        *self.backend.borrow_mut() = Some(backend.clone());
        // A new daemon may be in another state; say hello again.
        if let Backend::Socket(client) = &backend
            && let Ok(Response::Hello { status, .. }) =
                client.request(Request::Hello { protocol: PROTOCOL_VERSION }).await
        {
            self.set_status(status);
        }
        Ok(backend)
    }

    pub async fn request(self: &Rc<Self>, request: Request) -> Result<Response, IpcError> {
        match self.backend().await? {
            Backend::Socket(client) => client.request(request).await,
            Backend::Demo(demo) => demo.request(request).await,
        }
    }

    /// Sends a request whose answer does not matter.
    pub fn fire(self: &Rc<Self>, request: Request) {
        let app = self.clone();
        glib::spawn_future_local(async move {
            let _ = app.request(request).await;
        });
    }

    /// Tells the daemon the user is active, at most every 20 seconds.
    pub fn activity(self: &Rc<Self>) {
        let now = std::time::Instant::now();
        if self.last_activity.get().is_some_and(|t| now.duration_since(t) < Duration::from_secs(20)) {
            return;
        }
        self.last_activity.set(Some(now));
        self.fire(Request::Activity);
    }

    fn handle_event(self: &Rc<Self>, event: Option<Event>) {
        match event {
            Some(Event::Status { status }) => self.set_status(status),
            Some(Event::VaultChanged) => self.schedule_vault_reload(),
            Some(Event::Recent { ids }) => *self.recent.borrow_mut() = ids,
            Some(Event::Settings { settings }) => *self.settings.borrow_mut() = settings,
            Some(Event::Clipboard { clipboard }) => {
                self.each_view(|v| v.clipboard(&clipboard));
                self.notify_clipboard(&clipboard);
            }
            None => {
                // The daemon went away; reconnect on the next request.
                let app = self.clone();
                glib::timeout_add_local_once(Duration::from_millis(400), move || {
                    glib::spawn_future_local(async move { app.start().await });
                });
            }
        }
    }

    fn set_status(self: &Rc<Self>, status: Status) {
        let was = self.status.borrow().vault;
        let now = status.vault;
        *self.status.borrow_mut() = status.clone();
        if now == VaultState::Unlocked && was != VaultState::Unlocked {
            self.schedule_vault_reload();
        } else if now != VaultState::Unlocked && was == VaultState::Unlocked {
            *self.vault.borrow_mut() = Rc::new(VaultSnapshot::default());
            let vault = self.vault.borrow().clone();
            self.each_view(|v| v.vault_changed(&vault));
        }
        self.each_view(|v| v.status_changed(&status));
    }

    fn schedule_vault_reload(self: &Rc<Self>) {
        if let Some(source) = self.vault_reload.borrow_mut().take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(30), move || {
            let Some(app) = weak.upgrade() else { return };
            app.vault_reload.borrow_mut().take();
            glib::spawn_future_local(async move { app.reload_vault().await });
        });
        *self.vault_reload.borrow_mut() = Some(source);
    }

    pub async fn reload_vault(self: &Rc<Self>) {
        let start = std::time::Instant::now();
        if let Ok(Response::Vault { vault }) = self.request(Request::Vault).await {
            let fetched = start.elapsed();
            let count = vault.items.len();
            *self.recent.borrow_mut() = vault.recent.clone();
            let vault = Rc::new(vault);
            *self.vault.borrow_mut() = vault.clone();
            self.each_view(|v| v.vault_changed(&vault));
            timing(&format!("vault: {count} items fetched in {fetched:?}, shown in {:?}", start.elapsed()));
        }
    }

    /// Copies a field, asking for the master password first if the item
    /// requires it. Returns the error to show, if any.
    pub async fn copy(self: &Rc<Self>, parent: &gtk::Widget, id: &str, field: Field) -> Result<(), IpcError> {
        let request = || Request::Copy { id: id.to_owned(), field: field.clone() };
        match self.request(request()).await {
            Err(e) if e.code == ErrorCode::RepromptRequired => {
                if self.reprompt(parent, id).await? {
                    self.request(request()).await.map(drop)
                } else {
                    Ok(())
                }
            }
            other => other.map(drop),
        }
    }

    /// Reveals a field, re-prompting first if needed.
    pub async fn reveal(
        self: &Rc<Self>,
        parent: &gtk::Widget,
        id: &str,
        field: Field,
    ) -> Result<Option<Secret>, IpcError> {
        let request = || Request::Reveal { id: id.to_owned(), field: field.clone() };
        let response = match self.request(request()).await {
            Err(e) if e.code == ErrorCode::RepromptRequired => {
                if !self.reprompt(parent, id).await? {
                    return Ok(None);
                }
                self.request(request()).await?
            }
            other => other?,
        };
        match response {
            Response::Secret { value } => Ok(Some(value)),
            _ => Ok(None),
        }
    }

    /// Runs a request that may need the master password re-entered.
    pub async fn with_reprompt(
        self: &Rc<Self>,
        parent: &gtk::Widget,
        id: &str,
        request: impl Fn() -> Request,
    ) -> Result<Option<Response>, IpcError> {
        match self.request(request()).await {
            Err(e) if e.code == ErrorCode::RepromptRequired => {
                if !self.reprompt(parent, id).await? {
                    return Ok(None);
                }
                self.request(request()).await.map(Some)
            }
            other => other.map(Some),
        }
    }

    /// Asks for the master password for one item. `Ok(false)` if cancelled.
    pub async fn reprompt(self: &Rc<Self>, parent: &gtk::Widget, id: &str) -> Result<bool, IpcError> {
        crate::ui::reprompt::ask(self, parent, id).await
    }

    /// Tells the user about a copy made outside the main window, such as
    /// from quick access, which closes as it copies.
    fn notify_clipboard(&self, event: &ClipboardEvent) {
        const ID: &str = "countersign-clipboard";
        if std::env::var_os("COUNTERSIGN_SCREENSHOT").is_some() {
            return;
        }
        // The main window shows its own receipt, so an earlier notification
        // is out of date once it is focused, as it is once the value clears.
        let focused = crate::ui::window::current().is_some_and(|w| w.window.is_active());
        if event.state == ClipboardState::Cleared || focused {
            self.gtk.withdraw_notification(ID);
            return;
        }
        let notification = gio::Notification::new(&event.label);
        let mut body = Vec::new();
        if let Some(item) = &event.item_name {
            body.push(item.clone());
        }
        if let Some(secs) = event.follow_up_in {
            body.push(format!("the code replaces it in {secs} s"));
        }
        if let Some(secs) = event.clears_in {
            body.push(format!("clears in {secs} s"));
        }
        notification.set_body(Some(&body.join(" · ")));
        if event.follow_up_in.is_some() {
            notification.add_button("Keep password", "app.skip-follow-up");
        }
        if event.state == ClipboardState::Failed {
            notification.set_priority(gio::NotificationPriority::High);
        }
        self.gtk.send_notification(Some(ID), &notification);
    }

    pub fn show_main(self: &Rc<Self>) {
        crate::ui::window::present(self);
    }

    pub fn toggle_quick(self: &Rc<Self>) {
        crate::ui::quick::toggle(self);
    }
}

/// Prints a timing line when `COUNTERSIGN_TIMING` is set.
pub fn timing(message: &str) {
    if std::env::var_os("COUNTERSIGN_TIMING").is_some() {
        eprintln!("countersign timing: {message}");
    }
}
