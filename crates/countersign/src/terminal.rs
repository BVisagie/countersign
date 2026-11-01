//! Runs the official `bw login` in a terminal inside the app, so its own
//! prompts handle the email, master password, two-step and new-device codes.
//! When it succeeds, the session key `bw login --raw` prints is handed to the
//! daemon so the vault opens without typing the password twice; the terminal
//! is then wiped.
//!
//! The terminal pastes the clipboard with the keys a text field uses, and
//! from a button, one line at a time: a paste never presses Enter or answers
//! a later prompt.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{Request, Response, Secret, TerminalLaunch, VaultState};
use gtk::glib;
use vte4::prelude::*;

use crate::app::App;

/// Runs `bw login` in a dialog over `parent` and returns once the dialog
/// closes. Until the CLI draws its first prompt, the dialog says so instead
/// of showing an empty terminal. Once it succeeds, the dialog closes and the
/// window shows the vault opening.
pub async fn login(app: &Rc<App>, parent: &gtk::Widget, launch: TerminalLaunch) -> Result<(), String> {
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
    let title = adw::WindowTitle::new("Sign in with the Bitwarden CLI", "");
    let paste_button = gtk::Button::builder()
        .icon_name("edit-paste-symbolic")
        .tooltip_text("Paste (Ctrl+V)")
        .focus_on_click(false)
        .sensitive(false)
        .build();
    paste_button.update_property(&[gtk::accessible::Property::Label("Paste")]);
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    header.pack_start(&paste_button);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&frame));
    let dialog = adw::Dialog::builder().title("Sign in with the Bitwarden CLI").child(&view).build();
    let failure: Rc<RefCell<Option<String>>> = Rc::default();
    let child: Rc<Cell<Option<glib::Pid>>> = Rc::default();
    let exited = Rc::new(Cell::new(false));
    let dismissed = Rc::new(Cell::new(false));
    let (closed_tx, closed) = async_channel::bounded::<()>(1);
    {
        let (child, exited, dismissed) = (child.clone(), exited.clone(), dismissed.clone());
        dialog.connect_closed(move |_| {
            // Closing the dialog ends a sign-in still in progress, so a
            // second one cannot run beside it.
            dismissed.set(true);
            if !exited.get()
                && let Some(pid) = child.get()
            {
                hang_up(pid);
            }
            let _ = closed_tx.try_send(());
        });
    }
    {
        let (notice, exited, paste_button) = (notice.clone(), exited.clone(), paste_button.clone());
        terminal.connect_contents_changed(move |terminal| {
            // The CLI sends control sequences before its first prompt, and
            // VTE reports those as changes too: wait for visible text.
            let drawn = || terminal.text_format(vte4::Format::Text).is_some_and(|t| !t.trim().is_empty());
            if notice.is_visible() && !exited.get() && drawn() {
                notice.set_visible(false);
                terminal.set_opacity(1.0);
                terminal.grab_focus();
                paste_button.set_sensitive(true);
            }
        });
    }
    let paste = {
        // Weak: the terminal holds this through its shortcuts.
        let (terminal, title, notice, exited) =
            (terminal.downgrade(), title.downgrade(), notice.downgrade(), exited.clone());
        Rc::new(move || {
            let (terminal, title, notice, exited) =
                (terminal.clone(), title.clone(), notice.clone(), exited.clone());
            glib::spawn_future_local(async move {
                let Some(clipboard) = terminal.upgrade().map(|t| t.clipboard()) else { return };
                let text = clipboard.read_text_future().await.ok().flatten();
                let text = zeroize::Zeroizing::new(text.map(|t| t.to_string()).unwrap_or_default());
                let (Some(terminal), Some(title), Some(notice)) =
                    (terminal.upgrade(), title.upgrade(), notice.upgrade())
                else {
                    return;
                };
                // Only into a prompt the CLI is showing.
                if exited.get() || notice.is_visible() {
                    return;
                }
                let (line, more) = first_line(&text);
                let message = if line.is_empty() {
                    "The clipboard has no text"
                } else {
                    terminal.paste_text(line);
                    if more { "Pasted the first line of the clipboard" } else { "Pasted from the clipboard" }
                };
                terminal.grab_focus();
                flash(&title, message);
            });
        })
    };
    {
        let paste = paste.clone();
        paste_button.connect_clicked(move |_| paste());
    }
    {
        // Ahead of VTE, which sends Ctrl+V to the CLI as a character it
        // ignores and pastes the selection, not the clipboard, on Shift+Insert.
        // Omarchy's Super+V sends Ctrl+V.
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
        for accel in ["<Control>v", "<Control><Shift>v", "<Shift>Insert", "<Control><Shift>Insert"] {
            let paste = paste.clone();
            let action = gtk::CallbackAction::new(move |_, _| {
                paste();
                glib::Propagation::Stop
            });
            shortcuts
                .add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string(accel), Some(action)));
        }
        terminal.add_controller(shortcuts);
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
                // Closed before the CLI started: end it at once.
                Ok(pid) if dismissed.get() => hang_up(pid),
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
            paste_button.set_sensitive(false);
            let session =
                (status == 0).then(|| terminal.text_format(vte4::Format::Text)).flatten().and_then(|text| {
                    text.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
                });
            // Wipe what the terminal showed, including the session key.
            terminal.reset(true, true);
            terminal.set_opacity(0.0);
            let signed_in = session.is_some();
            let finished = Request::LoginFinished { session: session.map(Secret::new) };
            if signed_in {
                // From here the window shows the vault opening, before the
                // dialog goes, and offers nothing that would stop it.
                let opening = app.begin_opening();
                let app = app.clone();
                glib::spawn_future_local(async move {
                    let result = app.request(finished).await;
                    drop(opening);
                    if let Ok(Response::Status { status }) = result
                        && status.vault == VaultState::Locked
                    {
                        crate::ui::window::toast(
                            "You're signed in. Enter your master password to open your vault.",
                        );
                    }
                });
            } else {
                app.fire(finished);
            }
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    dialog.present(Some(parent));
    let _ = closed.recv().await;
    match failure.take() {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// The clipboard's first line of text, without line breaks, and whether
/// more text followed. VTE would send each line break as Enter.
fn first_line(text: &str) -> (&str, bool) {
    let text = text.trim_start_matches(['\r', '\n']);
    match text.find(['\r', '\n']) {
        Some(end) => (&text[..end], !text[end..].trim().is_empty()),
        None => (text, false),
    }
}

/// Shows `message` under the dialog's title for a few seconds.
fn flash(title: &adw::WindowTitle, message: &str) {
    title.set_subtitle(message);
    let (title, message) = (title.downgrade(), message.to_owned());
    glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
        // A newer message keeps its own time.
        if let Some(title) = title.upgrade()
            && title.subtitle() == message
        {
            title.set_subtitle("");
        }
    });
}

/// Ends the sign-in CLI, which has not been reaped yet, so the pid is still
/// its own.
fn hang_up(pid: glib::Pid) {
    // SAFETY: kill has no memory effects.
    unsafe { libc::kill(pid.0, libc::SIGHUP) };
}

/// VTE's flag for spawning without the parent environment
/// (`VTE_SPAWN_NO_PARENT_ENVV`), which glib's flags do not name.
const VTE_SPAWN_NO_PARENT_ENVV: u32 = 1 << 25;

#[cfg(test)]
mod tests {
    use super::first_line;

    #[test]
    fn pastes_one_line_without_its_line_break() {
        assert_eq!(first_line("correct horse "), ("correct horse ", false));
        assert_eq!(first_line("secret\n"), ("secret", false));
        assert_eq!(first_line("secret\r\n"), ("secret", false));
        assert_eq!(first_line("secret\nsomething else"), ("secret", true));
        assert_eq!(first_line("\r\n\nsecret\n"), ("secret", false));
        assert_eq!(first_line("\n\n"), ("", false));
        assert_eq!(first_line(""), ("", false));
    }
}
