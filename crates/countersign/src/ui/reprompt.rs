//! Asks for the master password before using an item that requires it.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{ErrorCode, IpcError, Request, Secret};
use gtk::glib;

use crate::app::App;
use crate::ui::label;

/// Shows the prompt over `parent`. `Ok(true)` once the password checks out,
/// `Ok(false)` if the user cancels.
pub async fn ask(app: &Rc<App>, parent: &gtk::Widget, id: &str) -> Result<bool, IpcError> {
    let entry = gtk::PasswordEntry::builder().show_peek_icon(true).build();
    super::space_masked(&entry);
    entry.update_property(&[gtk::accessible::Property::Label("Master password")]);
    let error = label("", &["cs-error"]);
    error.set_visible(false);
    let cancel = gtk::Button::with_label("Cancel");
    let verify =
        gtk::Button::builder().label("Continue").css_classes(["suggested-action"]).sensitive(false).build();
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    buttons.append(&cancel);
    buttons.append(&verify);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(6);
    body.set_margin_bottom(20);
    body.set_margin_start(20);
    body.set_margin_end(20);
    let explain = label("This item asks for your master password before it can be used.", &["cs-gate-body"]);
    explain.set_wrap(true);
    body.append(&explain);
    body.append(&entry);
    body.append(&error);
    body.append(&buttons);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    let dialog =
        adw::Dialog::builder().title("Master password required").content_width(380).child(&view).build();

    let (answers, answer) = async_channel::unbounded::<bool>();
    let answered = Rc::new(Cell::new(false));
    {
        let verify = verify.clone();
        entry.connect_changed(move |e| verify.set_sensitive(!e.text().is_empty()));
    }
    {
        let verify = verify.clone();
        entry.connect_activate(move |_| {
            if verify.is_sensitive() {
                verify.emit_clicked();
            }
        });
    }
    {
        let (dialog, answers, answered) = (dialog.clone(), answers.clone(), answered.clone());
        cancel.connect_clicked(move |_| {
            answered.set(true);
            let _ = answers.try_send(false);
            dialog.close();
        });
    }
    {
        let (answers, answered) = (answers.clone(), answered.clone());
        dialog.connect_closed(move |_| {
            if !answered.get() {
                let _ = answers.try_send(false);
            }
        });
    }
    {
        let (app, dialog, entry, error, id) =
            (app.clone(), dialog.clone(), entry.clone(), error.clone(), id.to_owned());
        verify.connect_clicked(move |verify| {
            let password = Secret::new(entry.text().as_str());
            entry.set_text("");
            verify.set_sensitive(false);
            entry.set_sensitive(false);
            let (app, dialog, entry, error, id, answers, answered) = (
                app.clone(),
                dialog.clone(),
                entry.clone(),
                error.clone(),
                id.clone(),
                answers.clone(),
                answered.clone(),
            );
            glib::spawn_future_local(async move {
                let result = app.request(Request::VerifyPassword { item_id: id, password }).await;
                entry.set_sensitive(true);
                match result {
                    Ok(_) => {
                        answered.set(true);
                        let _ = answers.try_send(true);
                        dialog.close();
                    }
                    Err(e) => {
                        let message = if e.code == ErrorCode::WrongPassword {
                            "That master password is incorrect."
                        } else {
                            &e.message
                        };
                        error.set_label(message);
                        error.set_visible(true);
                        entry.grab_focus();
                    }
                }
            });
        });
    }
    dialog.present(Some(parent));
    entry.grab_focus();
    Ok(answer.recv().await.unwrap_or(false))
}
