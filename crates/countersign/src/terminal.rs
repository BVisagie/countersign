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

/// How a sign-in ended. Each build uses one of these.
pub enum Login {
    /// The embedded terminal closed.
    #[cfg_attr(not(feature = "terminal"), expect(dead_code))]
    Closed,
    /// The user's own terminal runs `bw login`; they say when it is done.
    #[cfg_attr(feature = "terminal", expect(dead_code))]
    External,
}

/// Runs `bw login` in a dialog over `parent` and returns once the dialog
/// closes. Until the CLI draws its first prompt, and while the vault opens
/// after it succeeds, the dialog says so instead of showing an empty
/// terminal.
#[cfg(feature = "terminal")]
pub async fn login(app: &Rc<App>, parent: &gtk::Widget, launch: TerminalLaunch) -> Result<Login, String> {
    use std::cell::{Cell, RefCell};

    use adw::prelude::*;
    use countersign_ipc::{Request, Secret};
    use gtk::glib;
    use vte4::prelude::*;

    let terminal = vte4::Terminal::new();
    terminal.set_vexpand(true);
    terminal.set_hexpand(true);
    terminal.set_scrollback_lines(200);
    terminal.set_size_request(640, 360);
    // Hidden, not unmapped, so the CLI starts at the terminal's real size.
    terminal.set_opacity(0.0);
    let spinner = adw::Spinner::new();
    spinner.set_size_request(32, 32);
    let notice_text = crate::ui::label("Starting the Bitwarden CLI…", &["cs-gate-body"]);
    let notice = gtk::Box::new(gtk::Orientation::Vertical, 12);
    notice.set_valign(gtk::Align::Center);
    notice.set_halign(gtk::Align::Center);
    notice.append(&spinner);
    notice.append(&notice_text);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&terminal));
    overlay.add_overlay(&notice);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_margin_start(12);
    frame.set_margin_end(12);
    frame.set_margin_bottom(12);
    frame.append(&overlay);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&frame));
    let dialog = adw::Dialog::builder().title("Sign in with the Bitwarden CLI").child(&view).build();
    let failure: Rc<RefCell<Option<String>>> = Rc::default();
    let child: Rc<Cell<Option<glib::Pid>>> = Rc::default();
    let exited = Rc::new(Cell::new(false));
    let (closed_tx, closed) = async_channel::bounded::<()>(1);
    {
        let (child, exited) = (child.clone(), exited.clone());
        dialog.connect_closed(move |_| {
            // Closing the dialog ends a sign-in still in progress, so a
            // second one cannot run beside it.
            if !exited.get()
                && let Some(pid) = child.get()
            {
                // SAFETY: kill has no memory effects. The child has not been
                // reaped yet, so the pid is still its own.
                unsafe { libc::kill(pid.0, libc::SIGHUP) };
            }
            let _ = closed_tx.try_send(());
        });
    }
    {
        let (notice, exited) = (notice.clone(), exited.clone());
        terminal.connect_contents_changed(move |terminal| {
            if notice.is_visible() && !exited.get() {
                notice.set_visible(false);
                terminal.set_opacity(1.0);
                terminal.grab_focus();
            }
        });
    }

    let mut argv: Vec<String> = vec![launch.program.to_string_lossy().into_owned()];
    argv.extend(launch.args.iter().cloned());
    let mut envv: Vec<String> = launch.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    envv.push("TERM=xterm-256color".into());
    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
    let envv_ref: Vec<&str> = envv.iter().map(String::as_str).collect();
    let home = std::env::var("HOME").ok();
    {
        let (dialog, failure) = (dialog.downgrade(), failure.clone());
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
            move |result| match result {
                Ok(pid) => child.set(Some(pid)),
                Err(e) => {
                    *failure.borrow_mut() =
                        Some(format!("Countersign couldn't start the Bitwarden CLI: {e}"));
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.close();
                    }
                }
            },
        );
    }
    {
        // Weak: the terminal belongs to the dialog.
        let (app, dialog) = (app.clone(), dialog.downgrade());
        terminal.connect_child_exited(move |terminal, status| {
            exited.set(true);
            let session =
                (status == 0).then(|| terminal.text_format(vte4::Format::Text)).flatten().and_then(|text| {
                    text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
                });
            // Wipe what the terminal showed, including the session key.
            terminal.reset(true, true);
            terminal.set_opacity(0.0);
            let signed_in = session.is_some();
            let finished = Request::LoginFinished { session: session.map(Secret::new) };
            match dialog.upgrade() {
                Some(dialog) if signed_in => {
                    notice_text.set_label("Opening your vault…");
                    notice.set_visible(true);
                    dialog.set_can_close(false);
                    let app = app.clone();
                    glib::spawn_future_local(async move {
                        let _ = app.request(finished).await;
                        dialog.force_close();
                    });
                }
                dialog => {
                    app.fire(finished);
                    if let Some(dialog) = dialog {
                        dialog.close();
                    }
                }
            }
        });
    }
    dialog.present(Some(parent));
    let _ = closed.recv().await;
    match failure.take() {
        Some(message) => Err(message),
        None => Ok(Login::Closed),
    }
}

/// VTE's flag for spawning without the parent environment
/// (`VTE_SPAWN_NO_PARENT_ENVV`), which glib's flags do not name.
#[cfg(feature = "terminal")]
const VTE_SPAWN_NO_PARENT_ENVV: u32 = 1 << 25;

#[cfg(not(feature = "terminal"))]
pub async fn login(_app: &Rc<App>, _parent: &gtk::Widget, launch: TerminalLaunch) -> Result<Login, String> {
    // Plain `bw login`: its session key would otherwise stay on screen.
    let mut command = std::process::Command::new("xdg-terminal-exec");
    // `env -i` gives the CLI only the daemon's scrubbed environment.
    command.arg("env").arg("-i");
    for (k, v) in &launch.env {
        command.arg(format!("{k}={v}"));
    }
    command.arg(&launch.program).arg("login");
    match command.spawn() {
        Ok(_) => Ok(Login::External),
        Err(e) => Err(format!("Cannot open a terminal: {e}")),
    }
}
