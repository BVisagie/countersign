//! Countersign: a fast, keyboard-first Bitwarden app for Omarchy.
//!
//! `countersign` opens the main window. `countersign --quick` toggles quick
//! access, `--lock` locks the vault and `--background` starts without a
//! window. Further invocations go to the running instance, which stays in
//! the background so quick access opens instantly.

mod app;
mod demo;
mod screenshot;
mod search;
mod socket;
mod terminal;
mod theme;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::app::App;

const APP_ID: &str = "io.github.bvisagie.Countersign";

fn main() -> glib::ExitCode {
    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if std::env::var_os("COUNTERSIGN_SCREENSHOT").is_some() {
        // Never hand screenshot runs to a running Countersign.
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let gtk_app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    gtk_app.add_main_option(
        "quick",
        glib::Char::from(b'q'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Toggle quick access",
        None,
    );
    gtk_app.add_main_option(
        "lock",
        glib::Char::from(b'l'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Lock the vault",
        None,
    );
    gtk_app.add_main_option(
        "background",
        glib::Char::from(b'b'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start without opening a window",
        None,
    );
    gtk_app.add_main_option(
        "version",
        glib::Char::from(b'v'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Show the version",
        None,
    );

    gtk_app.connect_handle_local_options(|_, options| {
        if options.contains("version") {
            println!("countersign {}", env!("CARGO_PKG_VERSION"));
            return std::ops::ControlFlow::Break(glib::ExitCode::SUCCESS);
        }
        std::ops::ControlFlow::Continue(())
    });

    let state: Rc<RefCell<Option<Rc<App>>>> = Rc::default();
    {
        let state = state.clone();
        gtk_app.connect_startup(move |gtk_app| {
            theme::install();
            let demo = match std::env::var("COUNTERSIGN_DEMO").ok().as_deref() {
                Some("0") => None,
                Some("locked") => Some(false),
                Some(_) => Some(true),
                None => std::env::var_os("COUNTERSIGN_SCREENSHOT").map(|_| true),
            };
            let app = App::new(gtk_app.clone(), demo);
            install_actions(&app);
            // Stay running when windows close, so quick access is instant.
            std::mem::forget(gtk_app.hold());
            if let Some(dir) = std::env::var_os("COUNTERSIGN_SCREENSHOT") {
                screenshot::run(&app, dir.into());
            } else {
                let starting = app.clone();
                glib::spawn_future_local(async move {
                    starting.start().await;
                    // Build quick access ahead of time so it opens instantly.
                    ui::quick::get_or_create(&starting);
                });
            }
            *state.borrow_mut() = Some(app);
        });
    }
    gtk_app.connect_command_line(move |_, command_line| {
        let Some(app) = state.borrow().clone() else { return glib::ExitCode::FAILURE };
        if std::env::var_os("COUNTERSIGN_SCREENSHOT").is_some() {
            return glib::ExitCode::SUCCESS;
        }
        let options = command_line.options_dict();
        if options.contains("quick") {
            app.toggle_quick();
        } else if options.contains("lock") {
            app.fire(countersign_ipc::Request::Lock);
        } else if !options.contains("background") {
            app.show_main();
        }
        glib::ExitCode::SUCCESS
    });
    gtk_app.run()
}

fn install_actions(app: &Rc<App>) {
    let add = |name: &str, run: Rc<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| run());
        app.gtk.add_action(&action);
    };
    {
        let app = app.clone();
        add("quick", Rc::new(move || app.toggle_quick()));
    }
    {
        let app = app.clone();
        add("show", Rc::new(move || app.show_main()));
    }
    {
        let app = app.clone();
        add(
            "quit",
            Rc::new(move || {
                // Lock before leaving, so quitting never leaves the vault open.
                let app = app.clone();
                glib::spawn_future_local(async move {
                    let _ = app.request(countersign_ipc::Request::Lock).await;
                    app.gtk.quit();
                });
            }),
        );
    }
    app.gtk.set_accels_for_action("app.quit", &["<Control>q"]);
}
