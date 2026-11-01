//! What Countersign shows before the vault is open: setting up the CLI,
//! signing in and unlocking.

use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{CliState, ErrorCode, Request, Response, Secret, Settings, Status};
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

fn mark() -> gtk::Label {
    let mark = gtk::Label::new(Some("COUNTERSIGN"));
    mark.add_css_class("cs-gate-mark");
    mark.set_margin_bottom(8);
    mark
}

/// The unlock form, used by the main window and quick access.
pub struct Unlock {
    pub root: gtk::Widget,
    entry: gtk::PasswordEntry,
    button: gtk::Button,
    error: gtk::Label,
    account: gtk::Label,
    server: gtk::Label,
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
        let button = gtk::Button::builder().label("Unlock").css_classes(["suggested-action", "pill"]).build();
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_halign(gtk::Align::Center);
        content.append(&gtk::Label::new(Some("Unlock")));
        button.set_child(Some(&content));
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
        let unlock = Rc::new(Unlock { root: wrap(&column), entry, button, error, account, server });

        let submit = {
            let (app, me) = (app.clone(), Rc::downgrade(&unlock));
            move || {
                let Some(me) = me.upgrade() else { return };
                let text = me.entry.text();
                if text.is_empty() {
                    return;
                }
                let password = Secret::new(text.as_str());
                me.entry.set_text("");
                me.set_busy(true);
                let app = app.clone();
                glib::spawn_future_local(async move {
                    let result = app.request(Request::Unlock { password }).await;
                    me.set_busy(false);
                    if let Err(e) = result {
                        me.error.set_label(if e.code == ErrorCode::WrongPassword {
                            "That master password is incorrect."
                        } else {
                            &e.message
                        });
                        me.error.set_visible(true);
                        me.entry.grab_focus();
                    }
                });
            }
        };
        let s = submit.clone();
        unlock.entry.connect_activate(move |_| s());
        unlock.button.connect_clicked(move |_| submit());
        {
            let app = app.clone();
            sign_out.connect_clicked(move |button| confirm_sign_out(&app, button.upcast_ref()));
        }
        {
            // Start the CLI while the password is typed.
            let app = app.clone();
            unlock.root.connect_map(move |_| app.fire(Request::PrepareUnlock));
        }
        unlock
    }

    fn set_busy(&self, busy: bool) {
        self.entry.set_sensitive(!busy);
        self.button.set_sensitive(!busy);
        if let Some(content) = self.button.child().and_downcast::<gtk::Box>() {
            if busy && content.first_child().and_downcast::<adw::Spinner>().is_none() {
                content.prepend(&adw::Spinner::new());
            } else if !busy && let Some(spinner) = content.first_child().and_downcast::<adw::Spinner>() {
                content.remove(&spinner);
            }
        }
        if busy {
            self.error.set_visible(false);
        }
    }

    pub fn set_status(&self, status: &Status) {
        let account = status.account.as_ref();
        self.account.set_label(account.and_then(|a| a.email.as_deref()).unwrap_or(""));
        self.server.set_label(&server_name(account.and_then(|a| a.server_url.as_deref())));
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
pub struct SignIn {
    pub root: gtk::Widget,
    server: adw::ToggleGroup,
    custom: gtk::Entry,
    error: gtk::Label,
    waiting: gtk::Box,
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
        {
            let custom = custom.clone();
            server.connect_active_name_notify(move |g| {
                custom.set_visible(g.active_name().as_deref() == Some("self"))
            });
        }
        let email = gtk::Button::builder()
            .label("Sign in with email")
            .css_classes(["suggested-action", "pill"])
            .build();
        let api_key = gtk::Button::builder().label("Use an API key").css_classes(["flat"]).build();
        let error = label("", &["cs-error"]);
        error.set_visible(false);
        error.set_wrap(true);
        let waiting = gtk::Box::new(gtk::Orientation::Vertical, 8);
        waiting.set_visible(false);
        let waiting_text =
            label("Finish signing in in the terminal window, then come back here.", &["cs-gate-body"]);
        waiting_text.set_wrap(true);
        let done = gtk::Button::builder().label("I've signed in").css_classes(["pill"]).build();
        waiting.append(&waiting_text);
        waiting.append(&done);

        column.append(&title);
        column.append(&body);
        column.append(&server);
        column.append(&custom);
        column.append(&email);
        column.append(&api_key);
        column.append(&error);
        column.append(&waiting);
        let sign_in = Rc::new(SignIn { root: wrap(&column), server, custom, error, waiting });

        {
            let (app, me) = (app.clone(), Rc::downgrade(&sign_in));
            email.connect_clicked(move |button| {
                let Some(me) = me.upgrade() else { return };
                let (app, button) = (app.clone(), button.clone());
                glib::spawn_future_local(async move {
                    if !me.apply_server(&app).await {
                        return;
                    }
                    match app.request(Request::LoginTerminal).await {
                        Ok(Response::Terminal { launch }) => {
                            crate::terminal::login(&app, button.upcast_ref(), launch, me.waiting.clone())
                        }
                        Ok(_) => {}
                        Err(e) => me.show_error(&e.message),
                    }
                });
            });
        }
        {
            let app = app.clone();
            done.connect_clicked(move |_| app.fire(Request::LoginFinished { session: None }));
        }
        {
            let (app, me) = (app.clone(), Rc::downgrade(&sign_in));
            api_key.connect_clicked(move |button| {
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

    /// Sends the chosen server to the CLI. False if the address is invalid.
    async fn apply_server(&self, app: &Rc<App>) -> bool {
        self.error.set_visible(false);
        let url = match self.server.active_name().as_deref() {
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
        match app.request(Request::SetServer { url }).await {
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
        if self.waiting.is_visible() {
            return;
        }
        self.server.set_active_name(Some(name));
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
            let app = app.clone();
            glib::spawn_future_local(async move {
                if !me.apply_server(&app).await {
                    return;
                }
                if let Err(e) =
                    app.request(Request::LoginApiKey { client_id: id, client_secret: secret }).await
                {
                    me.show_error(&e.message);
                }
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
        let retry = gtk::Button::builder().label("Check again").css_classes(["flat"]).build();
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
            retry.connect_clicked(move |_| app.fire(Request::Refresh));
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
