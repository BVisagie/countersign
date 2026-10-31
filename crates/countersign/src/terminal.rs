//! Runs the official `bw login` so its own prompts handle the email, master
//! password, two-step and new-device codes.
//!
//! With the `terminal` feature the CLI runs in an embedded terminal. When it
//! succeeds, the session key `bw login --raw` prints is handed to the daemon
//! so the vault opens without typing the password twice; the terminal is
//! then wiped. Without the feature, your own terminal runs `bw login`.

use std::rc::Rc;

use countersign_ipc::TerminalLaunch;

use crate::app::App;

#[cfg(feature = "terminal")]
pub fn login(app: &Rc<App>, parent: &gtk::Widget, launch: TerminalLaunch, _waiting: gtk::Box) {
    use adw::prelude::*;
    use countersign_ipc::{Request, Secret};
    use gtk::glib;
    use vte4::prelude::*;

    let terminal = vte4::Terminal::new();
    terminal.set_vexpand(true);
    terminal.set_hexpand(true);
    terminal.set_scrollback_lines(200);
    terminal.set_size_request(640, 360);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_margin_start(12);
    frame.set_margin_end(12);
    frame.set_margin_bottom(12);
    frame.append(&terminal);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&frame));
    let dialog = adw::Dialog::builder().title("Sign in with the Bitwarden CLI").child(&view).build();

    let mut argv: Vec<String> = vec![launch.program.to_string_lossy().into_owned()];
    argv.extend(launch.args.iter().cloned());
    let mut envv: Vec<String> = launch.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    envv.push("TERM=xterm-256color".into());
    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
    let envv_ref: Vec<&str> = envv.iter().map(String::as_str).collect();
    let home = std::env::var("HOME").ok();
    terminal.spawn_async(
        vte4::PtyFlags::DEFAULT,
        home.as_deref(),
        &argv_ref,
        &envv_ref,
        // VTE adds `envv` to the app's own environment unless told not to;
        // the CLI gets only the scrubbed environment from the daemon.
        glib::SpawnFlags::from_bits_retain(VTE_SPAWN_NO_PARENT_ENVV),
        || {},
        -1,
        gtk::gio::Cancellable::NONE,
        |result| {
            if let Err(e) = result {
                tracing_warn(&format!("cannot start bw login: {e}"));
            }
        },
    );
    {
        let (app, dialog) = (app.clone(), dialog.clone());
        terminal.connect_child_exited(move |terminal, status| {
            let session =
                (status == 0).then(|| terminal.text_format(vte4::Format::Text)).flatten().and_then(|text| {
                    text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
                });
            // Wipe what the terminal showed, including the session key.
            terminal.reset(true, true);
            let app = app.clone();
            let dialog = dialog.clone();
            glib::spawn_future_local(async move {
                let _ = app.request(Request::LoginFinished { session: session.map(Secret::new) }).await;
                dialog.close();
            });
        });
    }
    dialog.present(Some(parent));
    terminal.grab_focus();
}

/// VTE's flag for spawning without the parent environment
/// (`VTE_SPAWN_NO_PARENT_ENVV`), which glib's flags do not name.
#[cfg(feature = "terminal")]
const VTE_SPAWN_NO_PARENT_ENVV: u32 = 1 << 25;

#[cfg(feature = "terminal")]
fn tracing_warn(message: &str) {
    eprintln!("countersign: {message}");
}

#[cfg(not(feature = "terminal"))]
pub fn login(_app: &Rc<App>, _parent: &gtk::Widget, launch: TerminalLaunch, waiting: gtk::Box) {
    use gtk::prelude::*;

    // Plain `bw login`: its session key would otherwise stay on screen.
    let mut command = std::process::Command::new("xdg-terminal-exec");
    // `env -i` gives the CLI only the daemon's scrubbed environment.
    command.arg("env").arg("-i");
    for (k, v) in &launch.env {
        command.arg(format!("{k}={v}"));
    }
    command.arg(&launch.program).arg("login");
    match command.spawn() {
        Ok(_) => waiting.set_visible(true),
        Err(e) => crate::ui::window::toast(&format!("Cannot open a terminal: {e}")),
    }
}
