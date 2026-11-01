//! What Countersign shows before the vault is open: setting up the CLI,
//! signing in and unlocking.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{CliState, ErrorCode, Request, Response, Secret, Settings, Status, VaultState};
use gtk::glib;

use super::label;
use crate::app::App;

fn gate_box() -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.add_css_class("cs-gate");
    column.set_valign(gtk::Align::Center);
    column.set_halign(gtk::Align::Center);
    column.set_width_request(340);
    column
}

fn wrap(widget: &impl IsA<gtk::Widget>) -> gtk::Widget {
    adw::Clamp::builder().maximum_size(420).child(widget).build().upcast()
}

/// A button whose label makes room for a spinner while its work runs.
fn busy_button(text: &str, classes: &[&str]) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_halign(gtk::Align::Center);
    content.append(&gtk::Label::new(Some(text)));
    gtk::Button::builder().css_classes(classes.to_vec()).child(&content).build()
}

/// Shows or hides the spinner in a [`busy_button`].
fn set_button_busy(button: &gtk::Button, busy: bool) {
    let Some(content) = button.child().and_downcast::<gtk::Box>() else { return };
    match (busy, content.first_child().and_downcast::<adw::Spinner>()) {
        (true, None) => content.prepend(&adw::Spinner::new()),
        (false, Some(spinner)) => content.remove(&spinner),
        _ => {}
    }
}

/// The sentry on watch above the wordmark, heading every page shown before
/// the vault opens.
pub fn mark() -> gtk::Box {
    let mark = gtk::Box::new(gtk::Orientation::Vertical, 14);
    mark.set_margin_bottom(8);
    let sentry = gtk::Image::builder()
        .icon_name(super::SENTRY_ICON)
        .pixel_size(72)
        .css_classes(["cs-gate-mascot"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let wordmark = gtk::Label::new(Some("COUNTERSIGN"));
    wordmark.add_css_class("cs-gate-mark");
    mark.append(&sentry);
    mark.append(&wordmark);
    mark
}

/// The unlock form, used by the main window and quick access.
///
/// While the vault opens, from this form or anywhere else, the form takes
/// no input and offers no way to sign out.
pub struct Unlock {
    pub root: gtk::Widget,
    entry: gtk::PasswordEntry,
    button: gtk::Button,
    sign_out: gtk::Button,
    error: gtk::Label,
    account: gtk::Label,
    server: gtk::Label,
    busy: Cell<bool>,
}

impl Unlock {
    pub fn new(app: &Rc<App>, compact: bool) -> Rc<Self> {
        let column = gate_box();
        if !compact {
            column.append(&mark());
        }
        let title = label("Unlock your vault", &["cs-gate-title"]);
        title.set_halign(gtk::Align::Center);
        let account = label("", &["cs-gate-body"]);
        account.set_halign(gtk::Align::Center);
        let server = label("", &["cs-meta"]);
        server.set_halign(gtk::Align::Center);
        let entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .placeholder_text("Master password")
            .margin_top(10)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Master password")]);
        let error = label("", &["cs-error"]);
        error.set_visible(false);
        error.set_wrap(true);
        let button = busy_button("Unlock", &["suggested-action", "pill"]);
        let sign_out =
            gtk::Button::builder().label("Sign out").css_classes(["flat"]).halign(gtk::Align::Center).build();
        column.append(&title);
        column.append(&account);
        column.append(&server);
        column.append(&entry);
        column.append(&error);
        column.append(&button);
        if !compact {
            column.append(&sign_out);
        }
        let unlock = Rc::new(Unlock {
            root: wrap(&column),
            entry,
            button,
            sign_out,
            error,
            account,
            server,
            busy: Cell::new(false),
        });

        let submit = {
            let (app, me) = (app.clone(), Rc::downgrade(&unlock));
            move || {
                let Some(me) = me.upgrade() else { return };
                let text = me.entry.text();
                if text.is_empty() || me.busy.get() {
                    return;
                }
                let password = Secret::new(text.as_str());
                me.entry.set_text("");
                me.error.set_visible(false);
                let opening = app.begin_opening();
                let app = app.clone();
                glib::spawn_future_local(async move {
                    let result = app.request(Request::Unlock { password }).await;
                    if let Err(e) = result {
                        me.error.set_label(if e.code == ErrorCode::WrongPassword {
                            "That master password is incorrect."
                        } else {
                            &e.message
                        });
                        me.error.set_visible(true);
                    }
                    // The form stays busy until the daemon says the attempt
                    // is over; see `set_busy`.
                    drop(opening);
                });
            }
        };
        let s = submit.clone();
        unlock.entry.connect_activate(move |_| s());
        unlock.button.connect_clicked(move |_| submit());
        {
            let app = app.clone();
            unlock.sign_out.connect_clicked(move |button| confirm_sign_out(&app, button.upcast_ref()));
        }
        {
            // Start the CLI while the password is typed. An open already
            // under way has its own.
            let app = app.clone();
            unlock.root.connect_map(move |_| {
                if !app.opening() {
                    app.fire(Request::PrepareUnlock);
                }
            });
        }
        unlock
    }

    fn set_busy(&self, busy: bool) {
        let was = self.busy.replace(busy);
        self.entry.set_sensitive(!busy);
        self.button.set_sensitive(!busy);
        self.sign_out.set_sensitive(!busy);
        set_button_busy(&self.button, busy);
        if busy && !was {
            self.error.set_visible(false);
        }
        // A failed attempt hands the field back for another try.
        if was && !busy && self.error.is_visible() && self.entry.is_mapped() {
            self.entry.grab_focus();
        }
    }

    /// Follows the daemon's status; `opening` is [`App::opening`]. Once the
    /// vault is open the form stays busy as it fades out.
    pub fn set_status(&self, status: &Status, opening: bool) {
        let account = status.account.as_ref();
        self.account.set_label(account.and_then(|a| a.email.as_deref()).unwrap_or(""));
        self.server.set_label(&server_name(account.and_then(|a| a.server_url.as_deref())));
        self.set_busy(opening || status.vault == VaultState::Unlocked);
    }

    pub fn focus(&self) {
        self.entry.grab_focus();
    }
}

pub fn server_name(url: Option<&str>) -> String {
    match url.map(|u| u.trim_end_matches('/')) {
        None | Some("") | Some("https://vault.bitwarden.com") => "Bitwarden cloud · US".into(),
        Some("https://vault.bitwarden.eu") => "Bitwarden cloud · EU".into(),
        Some(other) => other.trim_start_matches("https://").to_owned(),
    }
}

pub fn confirm_sign_out(app: &Rc<App>, parent: &gtk::Widget) {
    let dialog = adw::AlertDialog::builder()
        .heading("Sign out?")
        .body("Countersign will forget this account on this computer. Your vault stays safe on the server.")
        .close_response("cancel")
        .default_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("sign-out", "Sign out")]);
    dialog.set_response_appearance("sign-out", adw::ResponseAppearance::Destructive);
    let app = app.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "sign-out" {
            app.fire(Request::Logout);
        }
    });
    dialog.present(Some(parent));
}

/// Choosing a server and signing in with the official CLI.
///
/// Every step waits on the CLI, so the page shows what is running and
/// accepts no second sign-in until the first ends.
pub struct SignIn {
    pub root: gtk::Widget,
    server: adw::ToggleGroup,
    custom: gtk::Entry,
    email: gtk::Button,
    api_key: gtk::Button,
    error: gtk::Label,
    progress_spinner: adw::Spinner,
    progress_text: gtk::Label,
    /// A sign-in the user started is running, or its terminal is open.
    busy: Cell<bool>,
    /// The CLI is being pointed at the chosen cloud.
    switching: Cell<bool>,
    /// The choice changed while a switch ran.
    switch_again: Cell<bool>,
    /// The selector is following the daemon's status, not the user.
    from_status: Cell<bool>,
}

const SERVERS: [(&str, &str, Option<&str>); 3] = [
    ("us", "US cloud", None),
    ("eu", "EU cloud", Some("https://vault.bitwarden.eu")),
    ("self", "Self-hosted", None),
];

impl SignIn {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let column = gate_box();
        column.append(&mark());
        let title = label("Sign in to Bitwarden", &["cs-gate-title"]);
        title.set_halign(gtk::Align::Center);
        let body = label(
            "Countersign signs in through the official Bitwarden CLI. Your email, master password and two-step code go straight to it.",
            &["cs-gate-body"],
        );
        body.set_wrap(true);
        body.set_justify(gtk::Justification::Center);
        body.set_halign(gtk::Align::Center);
        let server = adw::ToggleGroup::new();
        for (name, text, _) in SERVERS {
            server.add(adw::Toggle::builder().name(name).label(text).build());
        }
        server.set_active_name(Some("us"));
        server.set_margin_top(12);
        let custom =
            gtk::Entry::builder().placeholder_text("https://vault.example.com").visible(false).build();
        custom.update_property(&[gtk::accessible::Property::Label("Self-hosted server address")]);
        let email = busy_button("Sign in with email", &["suggested-action", "pill"]);
        let api_key = busy_button("Use an API key", &["flat"]);
        let error = label("", &["cs-error"]);
        error.set_visible(false);
        error.set_wrap(true);
        // Always laid out, so the centred page does not move as it shows.
        let progress = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        progress.set_halign(gtk::Align::Center);
        let progress_spinner = adw::Spinner::new();
        progress_spinner.set_visible(false);
        let progress_text = label("", &["cs-meta"]);
        progress_text.set_wrap(true);
        progress.append(&progress_spinner);
        progress.append(&progress_text);

        column.append(&title);
        column.append(&body);
        column.append(&server);
        column.append(&custom);
        column.append(&email);
        column.append(&api_key);
        column.append(&progress);
        column.append(&error);
        let sign_in = Rc::new(SignIn {
            root: wrap(&column),
            server,
            custom,
            email,
            api_key,
            error,
            progress_spinner,
            progress_text,
            busy: Cell::new(false),
            switching: Cell::new(false),
            switch_again: Cell::new(false),
            from_status: Cell::new(false),
        });

        {
            let (app, me) = (app.clone(), Rc::downgrade(&sign_in));
            sign_in.server.connect_active_name_notify(move |group| {
                let Some(me) = me.upgrade() else { return };
                let self_hosted = group.active_name().as_deref() == Some("self");
                me.custom.set_visible(self_hosted);
                // A self-hosted address is applied once it is typed.
                if !self_hosted && !me.from_status.get() {
                    me.switch_server(&app);
                }
            });
        }
        {
            let (app, me) = (app.clone(), Rc::downgrade(&sign_in));
            sign_in.custom.connect_activate(move |_| {
                if let Some(me) = me.upgrade() {
                    me.switch_server(&app);
                }
            });
        }
        {
            let (app, me) = (app.clone(), Rc::downgrade(&sign_in));
            sign_in.email.connect_clicked(move |button| {
                let Some(me) = me.upgrade() else { return };
                if !me.begin(button, "Starting the Bitwarden CLI…") {
                    return;
                }
                let (app, button) = (app.clone(), button.clone());
                glib::spawn_future_local(async move {
                    if me.apply_server(&app).await {
                        me.show_progress(Some("Starting the Bitwarden CLI…"), false);
                        match app.request(Request::LoginTerminal).await {
                            Ok(Response::Terminal { launch }) => {
                                if let Err(message) =
                                    crate::terminal::login(&app, button.upcast_ref(), launch).await
                                {
                                    me.show_error(&message);
                                }
                            }
                            Ok(_) => {}
                            Err(e) => me.show_error(&e.message),
                        }
                    }
                    me.end(&button);
                });
            });
        }
        {
            let (app, me) = (app.clone(), Rc::downgrade(&sign_in));
            sign_in.api_key.connect_clicked(move |button| {
                let Some(me) = me.upgrade() else { return };
                me.ask_api_key(&app, button.upcast_ref());
            });
        }
        sign_in
    }

    fn show_error(&self, message: &str) {
        self.error.set_label(message);
        self.error.set_visible(true);
    }

    /// Says what is running, or hides the line. The spinner is for work no
    /// button shows.
    fn show_progress(&self, text: Option<&str>, spinner: bool) {
        self.progress_text.set_label(text.unwrap_or(""));
        self.progress_spinner.set_visible(spinner && text.is_some());
    }

    /// Makes the controls match what is running. The cloud can still be
    /// changed during a switch; the latest choice wins.
    fn update(&self) {
        let open = !self.busy.get();
        let ready = open && !self.switching.get();
        self.server.set_sensitive(open);
        self.custom.set_sensitive(ready);
        self.email.set_sensitive(ready);
        self.api_key.set_sensitive(ready);
    }

    /// Starts a sign-in from `button`, unless something is already running.
    fn begin(&self, button: &gtk::Button, text: &str) -> bool {
        if self.busy.get() || self.switching.get() {
            return false;
        }
        self.busy.set(true);
        self.error.set_visible(false);
        set_button_busy(button, true);
        self.show_progress(Some(text), false);
        self.update();
        true
    }

    fn end(&self, button: &gtk::Button) {
        self.busy.set(false);
        set_button_busy(button, false);
        self.show_progress(None, false);
        self.update();
    }

    /// Points the CLI at the chosen cloud as soon as it is chosen, so
    /// signing in does not wait for it.
    fn switch_server(self: &Rc<Self>, app: &Rc<App>) {
        if self.switching.get() {
            self.switch_again.set(true);
            return;
        }
        self.switching.set(true);
        self.update();
        let (me, app) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            loop {
                me.switch_again.set(false);
                let typing =
                    me.server.active_name().as_deref() == Some("self") && me.custom.text().is_empty();
                if !typing {
                    me.apply_server(&app).await;
                }
                if !me.switch_again.get() {
                    break;
                }
            }
            me.switching.set(false);
            me.show_progress(None, false);
            me.update();
        });
    }

    /// Sends the chosen server to the CLI. False if that failed.
    async fn apply_server(&self, app: &Rc<App>) -> bool {
        self.error.set_visible(false);
        let choice = self.server.active_name();
        let url = match choice.as_deref() {
            Some("eu") => Some("https://vault.bitwarden.eu".to_owned()),
            Some("self") => Some(self.custom.text().trim().to_owned()),
            _ => None,
        };
        let current = app.status.borrow().server_url.clone();
        let wanted = url.clone().unwrap_or_else(|| "https://vault.bitwarden.com".into());
        if current.as_deref().map(|c| c.trim_end_matches('/')) == Some(wanted.trim_end_matches('/'))
            || (current.is_none() && url.is_none())
        {
            return true;
        }
        let name = match choice.as_deref() {
            Some("self") => server_name(url.as_deref()),
            Some("eu") => "EU cloud".to_owned(),
            _ => "US cloud".to_owned(),
        };
        self.show_progress(Some(&format!("Switching to {name}…")), !self.busy.get());
        match app.request(Request::SetServer { url }).await {
            Ok(Response::Status { status }) => {
                // The status event may come after this answer; the next
                // switch compares against the server set here.
                app.set_status(status);
                true
            }
            Ok(_) => true,
            Err(e) => {
                self.show_error(&e.message);
                false
            }
        }
    }

    pub fn set_status(&self, status: &Status) {
        let name = match status.server_url.as_deref().map(|u| u.trim_end_matches('/')) {
            None | Some("https://vault.bitwarden.com") => "us",
            Some("https://vault.bitwarden.eu") => "eu",
            Some(other) => {
                if self.custom.text().is_empty() {
                    self.custom.set_text(other);
                }
                "self"
            }
        };
        // Leave the user's choice alone while it is being applied.
        if self.busy.get() || self.switching.get() {
            return;
        }
        self.from_status.set(true);
        self.server.set_active_name(Some(name));
        self.from_status.set(false);
    }

    fn ask_api_key(self: &Rc<Self>, app: &Rc<App>, parent: &gtk::Widget) {
        let client_id = adw::EntryRow::builder().title("client_id").build();
        let client_secret = adw::PasswordEntryRow::builder().title("client_secret").build();
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.append(&client_id);
        list.append(&client_secret);
        let dialog = adw::AlertDialog::builder()
            .heading("Sign in with an API key")
            .body("Find your personal API key in the Bitwarden web vault under Settings → Security → Keys. You will still unlock with your master password.")
            .extra_child(&list)
            .close_response("cancel")
            .default_response("sign-in")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("sign-in", "Sign in")]);
        dialog.set_response_appearance("sign-in", adw::ResponseAppearance::Suggested);
        let (app, me) = (app.clone(), Rc::downgrade(self));
        dialog.connect_response(None, move |_, response| {
            if response != "sign-in" {
                return;
            }
            let Some(me) = me.upgrade() else { return };
            let id = Secret::new(client_id.text().trim());
            let secret = Secret::new(client_secret.text().trim());
            client_secret.set_text("");
            if !me.begin(&me.api_key, "Signing in…") {
                return;
            }
            let app = app.clone();
            glib::spawn_future_local(async move {
                if me.apply_server(&app).await {
                    me.show_progress(Some("Signing in…"), false);
                    if let Err(e) =
                        app.request(Request::LoginApiKey { client_id: id, client_secret: secret }).await
                    {
                        me.show_error(&e.message);
                    }
                }
                me.end(&me.api_key);
            });
        });
        dialog.present(Some(parent));
    }
}

/// Shown when the official CLI is missing, too old or failing.
pub struct Setup {
    pub root: gtk::Widget,
    title: gtk::Label,
    body: gtk::Label,
    install: gtk::Button,
}

impl Setup {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let column = gate_box();
        column.append(&mark());
        let title = label("", &["cs-gate-title"]);
        title.set_halign(gtk::Align::Center);
        title.set_wrap(true);
        let body = label("", &["cs-gate-body"]);
        body.set_wrap(true);
        body.set_justify(gtk::Justification::Center);
        body.set_halign(gtk::Align::Center);
        let install = gtk::Button::builder()
            .label("Install the official CLI")
            .css_classes(["suggested-action", "pill"])
            .margin_top(10)
            .build();
        let choose =
            gtk::Button::builder().label("Use a CLI on this computer…").css_classes(["flat"]).build();
        let retry = busy_button("Check again", &["flat"]);
        column.append(&title);
        column.append(&body);
        column.append(&install);
        column.append(&choose);
        column.append(&retry);
        {
            let app = app.clone();
            install.connect_clicked(move |button| {
                button.set_sensitive(false);
                let (app, button) = (app.clone(), button.clone());
                glib::spawn_future_local(async move {
                    if let Err(e) = app.request(Request::InstallCli).await {
                        crate::ui::window::toast(&e.message);
                    }
                    button.set_sensitive(true);
                });
            });
        }
        {
            let app = app.clone();
            retry.connect_clicked(move |button| {
                button.set_sensitive(false);
                set_button_busy(button, true);
                let (app, button) = (app.clone(), button.clone());
                glib::spawn_future_local(async move {
                    let _ = app.request(Request::Refresh).await;
                    set_button_busy(&button, false);
                    button.set_sensitive(true);
                });
            });
        }
        {
            let app = app.clone();
            choose.connect_clicked(move |button| {
                let window = button.root().and_downcast::<gtk::Window>();
                let dialog = gtk::FileDialog::builder().title("Choose the Bitwarden CLI").build();
                let app = app.clone();
                glib::spawn_future_local(async move {
                    let Ok(file) = dialog.open_future(window.as_ref()).await else { return };
                    let Some(path) = file.path() else { return };
                    let settings = Settings { cli_path: Some(path), ..app.settings.borrow().clone() };
                    match app.request(Request::UpdateSettings { settings }).await {
                        Ok(Response::Settings { settings }) => *app.settings.borrow_mut() = settings,
                        Ok(_) => {}
                        Err(e) => crate::ui::window::toast(&e.message),
                    }
                });
            });
        }
        Rc::new(Setup { root: wrap(&column), title, body, install })
    }

    pub fn set_status(&self, status: &Status) {
        let (title, body) = match &status.cli {
            CliState::Missing => (
                "Countersign needs the Bitwarden CLI".to_owned(),
                "It does all of Countersign's encryption and talks to Bitwarden for it. Countersign can install the official release for you, checked against Bitwarden's published checksum.".to_owned(),
            ),
            CliState::Unsupported { found, minimum } => (
                "This Bitwarden CLI is too old".to_owned(),
                format!("Found {found}. Countersign needs the official CLI {minimum} or newer."),
            ),
            CliState::Installing => {
                ("Installing the Bitwarden CLI…".to_owned(), "Downloading and checking the official release.".to_owned())
            }
            CliState::Failed { message } => ("Countersign can't reach the Bitwarden CLI".to_owned(), message.clone()),
            _ => ("Starting…".to_owned(), String::new()),
        };
        self.title.set_label(&title);
        self.body.set_label(&body);
        self.install.set_sensitive(!matches!(status.cli, CliState::Installing));
    }
}
