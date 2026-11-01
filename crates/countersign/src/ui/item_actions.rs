//! What you can do to the shown item: edit it, favorite it, attach files,
//! move it to an organization, and trash, restore or delete it.

use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{ItemKind, Request, Response};
use gtk::{gio, glib};
use serde_json::Value;

use super::detail::Detail;
use super::editor::{Editor, template};

impl Detail {
    pub(super) fn install_actions(self: &Rc<Self>) {
        let group = &self.action_group;
        let add = |name: &str, run: fn(&Rc<Detail>)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(me) = weak.upgrade() {
                    run(&me);
                }
            });
            group.add_action(&action);
        };
        add("favorite", |me| me.toggle_favorite());
        add("attach", |me| me.add_attachment());
        add("move", |me| me.move_to_organization());
        add("delete", |me| me.trash());
        add("restore", |me| me.restore());
        add("purge", |me| me.purge());
        self.actions.insert_action_group("detail", Some(group));

        let weak = Rc::downgrade(self);
        self.edit_button.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.edit();
            }
        });
        self.update_actions(None);
    }

    /// Enables what applies to `item`, or nothing without one.
    pub(super) fn update_actions(&self, item: Option<&Value>) {
        let deleted = item.is_some_and(|i| !i["deletedDate"].is_null());
        let live = item.is_some() && !deleted;
        let personal = item.is_some_and(|i| i["organizationId"].is_null());
        let has_orgs = !self.vault.borrow().organizations.is_empty();
        let enable = |name: &str, on: bool| {
            if let Some(action) = self.action_group.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on);
            }
        };
        enable("favorite", live);
        enable("attach", live);
        enable("move", live && personal && has_orgs);
        enable("delete", live);
        enable("restore", deleted);
        enable("purge", deleted);
        self.edit_button.set_sensitive(live);
        let favorite = item.is_some_and(|i| i["favorite"].as_bool() == Some(true));
        let menu = gio::Menu::new();
        let first = gio::Menu::new();
        first.append(
            Some(if favorite { "Remove from favorites" } else { "Add to favorites" }),
            Some("detail.favorite"),
        );
        first.append(Some("Add attachment…"), Some("detail.attach"));
        if has_orgs {
            first.append(Some("Move to organization…"), Some("detail.move"));
        }
        menu.append_section(None, &first);
        let last = gio::Menu::new();
        if deleted {
            last.append(Some("Restore"), Some("detail.restore"));
            last.append(Some("Delete forever…"), Some("detail.purge"));
        } else {
            last.append(Some("Move to trash"), Some("detail.delete"));
        }
        menu.append_section(None, &last);
        self.menu_button.set_menu_model(Some(&menu));
    }

    fn parent(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn current_id(&self) -> Option<String> {
        self.current.borrow().clone()
    }

    /// Opens the editor for the shown item.
    pub fn edit(self: &Rc<Self>) {
        let Some(id) = self.current_id() else { return };
        let me = self.clone();
        glib::spawn_future_local(async move {
            let result =
                me.app.with_reprompt(&me.parent(), &id, || Request::ItemForEdit { id: id.clone() }).await;
            match result {
                // Only if the item is still the one shown, and no other
                // editor opened meanwhile.
                Ok(Some(Response::Json { value }))
                    if me.current_id().as_deref() == Some(id.as_str()) && !me.is_editing() =>
                {
                    me.open_editor(value, false)
                }
                Ok(_) => {}
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    /// Opens the editor for a new item.
    pub fn create(self: &Rc<Self>, kind: ItemKind, folder_id: Option<String>) {
        self.open_editor(template(kind, folder_id), true);
    }

    fn open_editor(self: &Rc<Self>, item: Value, creating: bool) {
        let weak = Rc::downgrade(self);
        let previous = self.current_id();
        let on_done: Rc<dyn Fn(Option<String>)> = Rc::new(move |saved: Option<String>| {
            let Some(me) = weak.upgrade() else { return };
            me.close_editor();
            let shown = saved.or_else(|| previous.clone());
            me.reload(shown.as_deref());
            // Bring the list back to the item; a new one once it is listed.
            if let (Some(window), Some(id)) = (super::window::current(), shown) {
                window.select_when_listed(&id);
            }
        });
        let vault = self.vault.borrow().clone();
        let editor = Editor::new(&self.app, item, &vault, creating, on_done);
        self.close_editor();
        // A detail load still running must not render over the editor.
        self.invalidate();
        self.root.add_named(&editor.root, Some("edit"));
        self.root.set_visible_child_name("edit");
        // The editor has its own Cancel and Save.
        self.actions.set_visible(false);
        if let Some(window) = super::window::current() {
            window.set_editing(true);
        }
        editor.focus();
        *self.editor.borrow_mut() = Some(editor);
    }

    /// Leaves the editor, discarding unsaved changes.
    pub(super) fn close_editor(&self) {
        let Some(editor) = self.editor.borrow_mut().take() else { return };
        self.root.remove(&editor.root);
        self.actions.set_visible(true);
        if let Some(window) = super::window::current() {
            window.set_editing(false);
        }
    }

    fn request(self: &Rc<Self>, request: Request, done: &'static str) {
        let me = self.clone();
        glib::spawn_future_local(async move {
            match me.app.request(request).await {
                Ok(_) => (me.notify)(done),
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    /// Done in the daemon, so the item's secrets never reach the app.
    fn toggle_favorite(self: &Rc<Self>) {
        let Some(id) = self.current_id() else { return };
        let favorite = !self.vault.borrow().items.iter().any(|i| i.id == id && i.favorite);
        let me = self.clone();
        glib::spawn_future_local(async move {
            match me.app.request(Request::SetFavorite { id: id.clone(), favorite }).await {
                Ok(_) => {
                    (me.notify)(if favorite { "Added to favorites" } else { "Removed from favorites" });
                    me.reload(Some(&id));
                }
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    fn trash(self: &Rc<Self>) {
        if let Some(id) = self.current_id() {
            self.request(Request::DeleteItem { id, permanent: false }, "Moved to the trash");
        }
    }

    fn restore(self: &Rc<Self>) {
        if let Some(id) = self.current_id() {
            self.request(Request::RestoreItem { id }, "Restored");
        }
    }

    fn purge(self: &Rc<Self>) {
        let Some(id) = self.current_id() else { return };
        let dialog = adw::AlertDialog::builder()
            .heading("Delete forever?")
            .body("The item and its attachments are removed from your vault on every device. This cannot be undone.")
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete forever")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let me = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                me.request(Request::DeleteItem { id: id.clone(), permanent: true }, "Deleted forever");
            }
        });
        dialog.present(Some(&self.parent()));
    }

    fn add_attachment(self: &Rc<Self>) {
        let Some(id) = self.current_id() else { return };
        let window = self.root.root().and_downcast::<gtk::Window>();
        let chooser = gtk::FileDialog::builder().title("Attach a file").build();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = chooser.open_future(window.as_ref()).await else { return };
            let Some(path) = file.path() else { return };
            match me.app.request(Request::AddAttachment { item_id: id.clone(), path }).await {
                Ok(_) => {
                    (me.notify)("Attachment added");
                    me.reload(Some(&id));
                }
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    pub(super) fn delete_attachment(self: &Rc<Self>, item_id: String, attachment_id: String, file: String) {
        let dialog = adw::AlertDialog::builder()
            .heading("Delete this attachment?")
            .body(format!("{file} is removed from the item on every device."))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let me = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            let (me, item_id, attachment_id) = (me.clone(), item_id.clone(), attachment_id.clone());
            glib::spawn_future_local(async move {
                match me
                    .app
                    .request(Request::DeleteAttachment { item_id: item_id.clone(), attachment_id })
                    .await
                {
                    Ok(_) => {
                        (me.notify)("Attachment deleted");
                        me.reload(Some(&item_id));
                    }
                    Err(e) => (me.notify)(&e.message),
                }
            });
        });
        dialog.present(Some(&self.parent()));
    }

    fn move_to_organization(self: &Rc<Self>) {
        let Some(id) = self.current_id() else { return };
        let vault = self.vault.borrow().clone();
        if vault.organizations.is_empty() {
            return;
        }
        let names: Vec<&str> = vault.organizations.iter().map(|o| o.name.as_str()).collect();
        let organization =
            adw::ComboRow::builder().title("Organization").model(&gtk::StringList::new(&names)).build();
        let collections = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let checks: Rc<std::cell::RefCell<Vec<(gtk::CheckButton, String)>>> = Rc::default();
        let fill = {
            let (vault, collections, checks) = (vault.clone(), collections.clone(), checks.clone());
            move |index: u32| {
                while let Some(row) = collections.row_at_index(0) {
                    collections.remove(&row);
                }
                checks.borrow_mut().clear();
                let Some(org) = vault.organizations.get(index as usize) else { return };
                for collection in vault.collections.iter().filter(|c| c.organization_id == org.id) {
                    let check = gtk::CheckButton::new();
                    let row = adw::ActionRow::builder()
                        .title(collection.name.as_str())
                        .activatable_widget(&check)
                        .build();
                    row.add_prefix(&check);
                    checks.borrow_mut().push((check, collection.id.clone()));
                    collections.append(&row);
                }
            }
        };
        fill(0);
        {
            let fill = fill.clone();
            organization.connect_selected_notify(move |row| fill(row.selected()));
        }
        let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let org_list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        org_list.append(&organization);
        list.append(&org_list);
        list.append(&collections);
        let dialog = adw::AlertDialog::builder()
            .heading("Move to an organization")
            .body("The organization will own the item; this cannot be undone from Countersign. Choose at least one collection.")
            .extra_child(&list)
            .close_response("cancel")
            .default_response("move")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("move", "Move")]);
        dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        let me = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "move" {
                return;
            }
            let Some(org) = vault.organizations.get(organization.selected() as usize) else { return };
            let chosen: Vec<String> = checks
                .borrow()
                .iter()
                .filter(|(check, _)| check.is_active())
                .map(|(_, id)| id.clone())
                .collect();
            me.request(
                Request::MoveItem { id: id.clone(), organization_id: org.id.clone(), collection_ids: chosen },
                "Moved to the organization",
            );
        });
        dialog.present(Some(&self.parent()));
    }
}
