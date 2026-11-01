//! Settings. Changes apply as you make them; the daemon stores them in
//! `~/.config/countersign/settings.toml`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use countersign_ipc::{CliState, Request, Response, Settings};
use gtk::{gdk, glib};

use super::gates::{confirm_sign_out, server_name};
use crate::app::App;

const QUICK_BINDING: &str = r#"o.bind("SUPER + SHIFT + P", "Passwords", "countersign --quick")"#;
const AUTOSTART: &str = r#"o.launch_on_start("countersign --background")"#;

pub fn present(app: &Rc<App>) {
    let parent = super::window::current().map(|w| w.window.clone().upcast::<gtk::Widget>());
    let dialog = build(app);
    dialog.present(parent.as_ref());
}

fn spin(title: &str, subtitle: &str, min: f64, max: f64, value: u32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_value(f64::from(value));
    row
}

fn switch(title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder().title(title).subtitle(subtitle).active(active).build()
}

/// A row showing text you can copy, such as a Hyprland binding.
fn snippet(title: &str, text: &'static str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(text).subtitle_selectable(true).build();
    row.add_css_class("property");
    let copy = super::icon_button("edit-copy-symbolic", "Copy");
    copy.connect_clicked(move |button| {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(text);
        }
        button.set_icon_name("object-select-symbolic");
    });
    row.add_suffix(&copy);
    row
}

fn build(app: &Rc<App>) -> adw::PreferencesDialog {
    let settings = app.settings.borrow().clone();
    let dialog = adw::PreferencesDialog::builder().title("Settings").search_enabled(false).build();
    let page = adw::PreferencesPage::new();

    let clipboard = adw::PreferencesGroup::builder().title("Clipboard").build();
    let clear = spin(
        "Clear after",
        "Seconds a copied value stays on the clipboard",
        5.0,
        600.0,
        settings.clipboard_clear_secs,
    );
    let follow = switch(
        "Copy the verification code next",
        "After you copy a password, its one-time code replaces it",
        settings.totp_follow_up,
    );
    let follow_delay = spin("Code follows after", "Seconds", 1.0, 120.0, settings.totp_follow_up_secs);
    follow.bind_property("active", &follow_delay, "sensitive").sync_create().build();
    clipboard.add(&clear);
    clipboard.add(&follow);
    clipboard.add(&follow_delay);

    let locking = adw::PreferencesGroup::builder().title("Locking").build();
    let idle = spin(
        "Lock after inactivity",
        "Minutes; 0 never locks for inactivity",
        0.0,
        1440.0,
        settings.lock_after_idle_mins,
    );
    let screen = switch(
        "Lock with the screen",
        "When Hyprland or logind locks the session",
        settings.lock_on_screen_lock,
    );
    let sleep =
        switch("Lock before sleep", "Before the computer suspends or hibernates", settings.lock_on_suspend);
    locking.add(&idle);
    locking.add(&screen);
    locking.add(&sleep);

    let sync = adw::PreferencesGroup::builder().title("Sync").build();
    let interval =
        spin("Sync every", "Minutes; 0 syncs only when unlocking", 0.0, 1440.0, settings.sync_interval_mins);
    sync.add(&interval);

    let desktop = adw::PreferencesGroup::builder()
        .title("Desktop")
        .description("Add these lines to ~/.config/hypr/bindings.lua and autostart.lua.")
        .build();
    let background =
        switch("Keep running when closed", "Quick access opens instantly", settings.close_to_background);
    desktop.add(&background);
    desktop.add(&snippet("Quick access key", QUICK_BINDING));
    desktop.add(&snippet("Start with Hyprland", AUTOSTART));

    let status = app.status.borrow().clone();
    let account = adw::PreferencesGroup::builder().title("Account").build();
    let email =
        status.account.as_ref().and_then(|a| a.email.clone()).unwrap_or_else(|| "Not signed in".into());
    let account_row = adw::ActionRow::builder()
        .title(email)
        .subtitle(server_name(status.account.as_ref().and_then(|a| a.server_url.as_deref())))
        .build();
    let sign_out = gtk::Button::builder()
        .label("Sign out…")
        .valign(gtk::Align::Center)
        .css_classes(["destructive-action"])
        .sensitive(status.account.is_some())
        .build();
    account_row.add_suffix(&sign_out);
    account.add(&account_row);

    let cli = adw::PreferencesGroup::builder().title("Bitwarden CLI").build();
    let (cli_title, cli_subtitle) = match &status.cli {
        CliState::Ready { version, managed: true } => {
            (format!("Official CLI {version}"), "Installed and checked by Countersign".to_owned())
        }
        CliState::Ready { version, managed: false } => (
            format!("Official CLI {version}"),
            settings.cli_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "From PATH".into()),
        ),
        CliState::Unsupported { found, minimum } => {
            (format!("Unsupported CLI {found}"), format!("Needs {minimum} or newer"))
        }
        _ => ("Not available".into(), String::new()),
    };
    let cli_row = adw::ActionRow::builder().title(cli_title).subtitle(cli_subtitle).build();
    cli.add(&cli_row);
    if settings.cli_path.is_some() {
        let managed = gtk::Button::builder().label("Use the managed CLI").valign(gtk::Align::Center).build();
        cli_row.add_suffix(&managed);
        let (app, dialog) = (app.clone(), dialog.clone());
        managed.connect_clicked(move |_| {
            let settings = Settings { cli_path: None, ..app.settings.borrow().clone() };
            save(&app, settings);
            dialog.close();
        });
    }

    page.add(&clipboard);
    page.add(&locking);
    page.add(&sync);
    page.add(&desktop);
    page.add(&account);
    page.add(&cli);
    dialog.add(&page);

    // Gather every control into one settings value and save it, at most
    // every 300 ms while a spin button is held.
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let apply: Rc<dyn Fn()> = Rc::new({
        let app = app.clone();
        let (clear, follow, follow_delay, idle, screen, sleep, interval, background) = (
            clear.clone(),
            follow.clone(),
            follow_delay.clone(),
            idle.clone(),
            screen.clone(),
            sleep.clone(),
            interval.clone(),
            background.clone(),
        );
        move || {
            let current = app.settings.borrow().clone();
            let next = Settings {
                clipboard_clear_secs: clear.value() as u32,
                totp_follow_up: follow.is_active(),
                totp_follow_up_secs: follow_delay.value() as u32,
                lock_after_idle_mins: idle.value() as u32,
                lock_on_screen_lock: screen.is_active(),
                lock_on_suspend: sleep.is_active(),
                sync_interval_mins: interval.value() as u32,
                close_to_background: background.is_active(),
                ..current.clone()
            };
            if next == current {
                return;
            }
            if let Some(source) = pending.borrow_mut().take() {
                source.remove();
            }
            let (app, slot) = (app.clone(), pending.clone());
            let source = glib::timeout_add_local_once(Duration::from_millis(300), move || {
                slot.borrow_mut().take();
                save(&app, next);
            });
            *pending.borrow_mut() = Some(source);
        }
    });
    for row in [&clear, &follow_delay, &idle, &interval] {
        let apply = apply.clone();
        row.connect_value_notify(move |_| apply());
    }
    for row in [&follow, &screen, &sleep, &background] {
        let apply = apply.clone();
        row.connect_active_notify(move |_| apply());
    }
    {
        let app = app.clone();
        sign_out.connect_clicked(move |button| confirm_sign_out(&app, button.upcast_ref()));
    }
    dialog
}

fn save(app: &Rc<App>, settings: Settings) {
    *app.settings.borrow_mut() = settings.clone();
    let app = app.clone();
    glib::spawn_future_local(async move {
        match app.request(Request::UpdateSettings { settings }).await {
            Ok(Response::Settings { settings }) => *app.settings.borrow_mut() = settings,
            Ok(_) => {}
            Err(e) => super::window::toast(&e.message),
        }
    });
}
