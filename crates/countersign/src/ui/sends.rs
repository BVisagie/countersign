//! Send: share text or a file through Bitwarden, end-to-end encrypted.
//!
//! The list never holds a Send's text or link (the daemon strips them; see
//! `redact_send`). The editor fetches the text when you edit a Send, and the
//! daemon copies links.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{ErrorCode, Request, Response};
use gtk::{gio, glib};
use serde_json::{Value, json};

use super::{icon_button, label};
use crate::app::App;

/// How long a new Send lives, in hours. Bitwarden allows at most 31 days.
const LIFETIMES: [(&str, i64); 7] = [
    ("1 hour", 1),
    ("1 day", 24),
    ("2 days", 48),
    ("3 days", 72),
    ("7 days", 168),
    ("14 days", 336),
    ("30 days", 720),
];
const DEFAULT_LIFETIME: u32 = 4;

pub struct Sends {
    pub root: gtk::Widget,
    app: Rc<App>,
    notify: Rc<dyn Fn(&str)>,
    list: gtk::ListBox,
    pages: gtk::Stack,
    spinner: adw::Spinner,
    sends: RefCell<Vec<Value>>,
    /// The list may be out of date: load it when the page is next shown.
    stale: Cell<bool>,
    loading: Cell<bool>,
    generation: Cell<u64>,
    editor: RefCell<Option<adw::Dialog>>,
}

impl Sends {
    pub fn new(
        app: &Rc<App>,
        notify: Rc<dyn Fn(&str)>,
        header_start: Option<&gtk::Widget>,
        receipt: &gtk::Widget,
    ) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let intro = label(
            "Share text or a file with anyone. It is end-to-end encrypted and deletes itself on the date you choose.",
            &["cs-empty-body"],
        );
        intro.set_wrap(true);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 14);
        column.add_css_class("cs-detail");
        column.append(&intro);
        column.append(&list);
        let clamp = adw::Clamp::builder().maximum_size(720).child(&column).build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();

        let first = gtk::Button::builder()
            .label("New text Send")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .action_name("sends.new-text")
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("send-to-symbolic")
            .title("No Sends")
            .description("Share text or a file with anyone, end-to-end encrypted. A Send deletes itself on the date you choose.")
            .child(&first)
            .build();
        let waiting = adw::Spinner::new();
        waiting.set_size_request(32, 32);
        waiting.set_halign(gtk::Align::Center);
        waiting.set_valign(gtk::Align::Center);
        let pages = gtk::Stack::new();
        pages.add_named(&waiting, Some("loading"));
        pages.add_named(&empty, Some("empty"));
        pages.add_named(&scroll, Some("list"));

        let menu = gio::Menu::new();
        menu.append(Some("Text Send"), Some("sends.new-text"));
        menu.append(Some("File Send"), Some("sends.new-file"));
        let new_button = gtk::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New Send")
            .menu_model(&menu)
            .build();
        let spinner = adw::Spinner::new();
        spinner.set_visible(false);
        spinner.set_tooltip_text(Some("Loading Sends"));
        let header = adw::HeaderBar::builder().title_widget(&label("Send", &["title"])).build();
        if let Some(start) = header_start {
            header.pack_start(start);
        }
        header.pack_end(&new_button);
        header.pack_end(&spinner);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&pages));
        view.add_bottom_bar(receipt);

        let sends = Rc::new(Sends {
            root: view.clone().upcast(),
            app: app.clone(),
            notify,
            list,
            pages,
            spinner,
            sends: RefCell::default(),
            stale: Cell::new(true),
            loading: Cell::new(false),
            generation: Cell::new(0),
            editor: RefCell::default(),
        });
        let group = gio::SimpleActionGroup::new();
        for (name, file) in [("new-text", false), ("new-file", true)] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(&sends);
            action.connect_activate(move |_, _| {
                if let Some(me) = weak.upgrade() {
                    me.open_editor(None, file);
                }
            });
            group.add_action(&action);
        }
        view.insert_action_group("sends", Some(&group));
        let weak = Rc::downgrade(&sends);
        view.connect_map(move |_| {
            if let Some(me) = weak.upgrade()
                && me.stale.get()
            {
                me.load();
            }
        });
        sends
    }

    /// Sends may have changed, for example after a sync.
    pub fn vault_changed(self: &Rc<Self>) {
        self.stale.set(true);
        if self.root.is_mapped() {
            self.load();
        }
    }

    /// Forgets every Send and closes the editor, for example when the vault
    /// locks.
    pub fn clear(self: &Rc<Self>) {
        self.generation.set(self.generation.get() + 1);
        self.loading.set(false);
        self.spinner.set_visible(false);
        self.stale.set(true);
        self.sends.borrow_mut().clear();
        if let Some(dialog) = self.editor.borrow_mut().take() {
            dialog.force_close();
        }
        self.render();
    }

    /// Moves focus into the page.
    pub fn focus(&self) {
        self.list.grab_focus();
    }

    fn load(self: &Rc<Self>) {
        self.stale.set(false);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.loading.set(true);
        self.spinner.set_visible(true);
        self.render();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let result = me.app.request(Request::Sends).await;
            if me.generation.get() != generation {
                return;
            }
            me.loading.set(false);
            me.spinner.set_visible(false);
            match result {
                Ok(Response::List { values }) => *me.sends.borrow_mut() = values,
                Ok(_) => {}
                Err(e) => {
                    me.stale.set(true);
                    if e.code != ErrorCode::Locked {
                        (me.notify)(&e.message);
                    }
                }
            }
            me.render();
        });
    }

    fn render(self: &Rc<Self>) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut sends = self.sends.borrow().clone();
        sends.sort_by_key(|s| s["name"].as_str().unwrap_or_default().to_lowercase());
        let now = glib::DateTime::now_utc().ok();
        for send in &sends {
            self.list.append(&self.row(send, now.as_ref()));
        }
        self.pages.set_visible_child_name(match (sends.is_empty(), self.loading.get()) {
            (false, _) => "list",
            (true, true) => "loading",
            (true, false) => "empty",
        });
    }

    fn row(self: &Rc<Self>, send: &Value, now: Option<&glib::DateTime>) -> adw::ActionRow {
        let id = send["id"].as_str().unwrap_or_default().to_owned();
        let name = send["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Untitled").to_owned();
        let row = adw::ActionRow::builder()
            .title(name.as_str())
            .subtitle(now.map(|now| summary(send, now)).unwrap_or_default())
            .use_markup(false)
            .activatable(true)
            .build();
        let file = send["type"].as_i64() == Some(1);
        let icon = gtk::Image::from_icon_name(if file {
            "x-office-document-symbolic"
        } else {
            "text-x-generic-symbolic"
        });
        icon.add_css_class("cs-nav-icon");
        row.add_prefix(&icon);
        if send["passwordSet"].as_bool() == Some(true) {
            let lock = gtk::Image::from_icon_name("system-lock-screen-symbolic");
            lock.set_tooltip_text(Some("Opens with a password"));
            lock.add_css_class("cs-badge");
            row.add_suffix(&lock);
        }
        let copy = icon_button("insert-link-symbolic", "Copy link");
        {
            let (weak, id) = (Rc::downgrade(self), id.clone());
            copy.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.copy_link(&id);
                }
            });
        }
        row.add_suffix(&copy);

        let group = gio::SimpleActionGroup::new();
        let add = |action: &str, run: fn(&Rc<Sends>, &str, &str)| {
            let simple = gio::SimpleAction::new(action, None);
            let (weak, id, name) = (Rc::downgrade(self), id.clone(), name.clone());
            simple.connect_activate(move |_, _| {
                if let Some(me) = weak.upgrade() {
                    run(&me, &id, &name);
                }
            });
            group.add_action(&simple);
        };
        add("edit", |me, id, _| me.edit(id));
        add("remove-password", |me, id, _| me.remove_password(id));
        add("delete", |me, id, name| me.delete(id, name));
        row.insert_action_group("send", Some(&group));
        let menu = gio::Menu::new();
        menu.append(Some("Edit…"), Some("send.edit"));
        if send["passwordSet"].as_bool() == Some(true) {
            menu.append(Some("Remove password"), Some("send.remove-password"));
        }
        let last = gio::Menu::new();
        last.append(Some("Delete…"), Some("send.delete"));
        menu.append_section(None, &last);
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .menu_model(&menu)
            .build();
        row.add_suffix(&more);
        {
            let weak = Rc::downgrade(self);
            row.connect_activated(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.edit(&id);
                }
            });
        }
        row
    }

    /// Puts a created or edited Send in the list.
    fn upsert(self: &Rc<Self>, send: Value) {
        {
            let mut sends = self.sends.borrow_mut();
            match sends.iter_mut().find(|s| s["id"] == send["id"]) {
                Some(slot) => *slot = send,
                None => sends.push(send),
            }
        }
        self.changed();
    }

    /// A load still running began before this change; start it again so
    /// its older answer does not replace the list.
    fn changed(self: &Rc<Self>) {
        if self.loading.get() { self.load() } else { self.render() }
    }

    fn copy_link(self: &Rc<Self>, id: &str) {
        let (me, id) = (self.clone(), id.to_owned());
        glib::spawn_future_local(async move {
            // The receipt bar shows the copy and its countdown.
            if let Err(e) = me.app.request(Request::CopySendLink { id }).await {
                (me.notify)(&e.message);
            }
        });
    }

    fn edit(self: &Rc<Self>, id: &str) {
        let (me, id) = (self.clone(), id.to_owned());
        glib::spawn_future_local(async move {
            match me.app.request(Request::SendForEdit { id }).await {
                Ok(Response::Json { value }) => {
                    let file = value["type"].as_i64() == Some(1);
                    me.open_editor(Some(value), file);
                }
                Ok(_) => {}
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    fn remove_password(self: &Rc<Self>, id: &str) {
        let (me, id) = (self.clone(), id.to_owned());
        glib::spawn_future_local(async move {
            match me.app.request(Request::RemoveSendPassword { id }).await {
                Ok(Response::Json { value }) => {
                    me.upsert(value);
                    (me.notify)("Password removed");
                }
                Ok(_) => {}
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    fn delete(self: &Rc<Self>, id: &str, name: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Delete “{name}”?"))
            .body("Its link stops working for everyone. This cannot be undone.")
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let (me, id) = (self.clone(), id.to_owned());
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            let (me, id) = (me.clone(), id.clone());
            glib::spawn_future_local(async move {
                match me.app.request(Request::DeleteSend { id: id.clone() }).await {
                    Ok(_) => {
                        me.sends.borrow_mut().retain(|s| s["id"] != id.as_str());
                        me.changed();
                        (me.notify)("Send deleted");
                    }
                    Err(e) => (me.notify)(&e.message),
                }
            });
        });
        dialog.present(Some(&self.root));
    }

    /// The editor for a new Send, or for `existing` as `SendForEdit`
    /// returned it.
    fn open_editor(self: &Rc<Self>, existing: Option<Value>, file: bool) {
        let creating = existing.is_none();
        let original = existing.unwrap_or_else(|| template(file));

        let name = adw::EntryRow::builder()
            .title("Name")
            .text(original["name"].as_str().unwrap_or_default())
            .build();
        let general = adw::PreferencesGroup::new();
        general.add(&name);

        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(10)
            .bottom_margin(10)
            .left_margin(12)
            .right_margin(12)
            .build();
        text.update_property(&[gtk::accessible::Property::Label("Text to share")]);
        text.buffer().set_text(original.pointer("/text/text").and_then(Value::as_str).unwrap_or_default());
        let text_frame = gtk::ScrolledWindow::builder()
            .min_content_height(140)
            .max_content_height(320)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["card"])
            .child(&text)
            .build();
        let text_group = adw::PreferencesGroup::builder().title("Text").build();
        text_group.add(&text_frame);
        let hidden = adw::SwitchRow::builder()
            .title("Hide the text until opened")
            .subtitle("Recipients click to show it")
            .active(original.pointer("/text/hidden").and_then(Value::as_bool).unwrap_or(false))
            .build();

        let chosen: Rc<RefCell<Option<PathBuf>>> = Rc::default();
        let file_row = adw::ActionRow::builder()
            .title("File")
            .subtitle(original.pointer("/file/fileName").and_then(Value::as_str).unwrap_or("None chosen"))
            .subtitle_selectable(false)
            .use_markup(false)
            .build();
        let file_group = adw::PreferencesGroup::builder()
            .description(if creating { "At most 500 MB. File Sends need Premium." } else { "" })
            .build();
        file_group.add(&file_row);

        let lifetime = adw::ComboRow::builder().title("Delete after").build();
        let mut choices: Vec<String> = Vec::new();
        if !creating {
            let current =
                original["deletionDate"].as_str().and_then(local_date).unwrap_or_else(|| "unchanged".into());
            choices.push(format!("Keep ({current})"));
        }
        choices.extend(LIFETIMES.iter().map(|(label, _)| (*label).to_owned()));
        let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
        lifetime.set_model(Some(&gtk::StringList::new(&refs)));
        lifetime.set_selected(if creating { DEFAULT_LIFETIME } else { 0 });
        let max_views = adw::SpinRow::with_range(0.0, 1000.0, 1.0);
        max_views.set_title("Maximum views");
        max_views.set_subtitle("0 means no limit");
        max_views.set_value(original["maxAccessCount"].as_f64().unwrap_or(0.0));
        // The CLI keeps no password it is not given: an edit without one
        // removes it. So a protected Send needs its password typed again,
        // and turning the switch off is how you remove it.
        let has_password = original["passwordSet"].as_bool() == Some(true);
        let protect = adw::SwitchRow::builder()
            .title("Require a password")
            .subtitle(if has_password {
                "Type the password again to keep it, or a new one"
            } else {
                "Recipients need it to open the Send"
            })
            .active(has_password)
            .build();
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        protect.bind_property("active", &password, "visible").sync_create().build();
        // A Send for chosen email addresses cannot also have a password.
        if original["emails"].as_array().is_some_and(|e| !e.is_empty()) {
            protect.set_active(false);
            protect.set_visible(false);
        }
        let hide_email = adw::SwitchRow::builder()
            .title("Hide my email address")
            .active(original["hideEmail"].as_bool().unwrap_or(false))
            .build();
        let disabled = adw::SwitchRow::builder()
            .title("Disable this Send")
            .subtitle("No one can open it")
            .active(original["disabled"].as_bool().unwrap_or(false))
            .build();
        let notes = adw::EntryRow::builder()
            .title("Private notes")
            .text(original["notes"].as_str().unwrap_or_default())
            .build();
        let options = adw::PreferencesGroup::builder().title("Options").build();
        if !file {
            options.add(&hidden);
        }
        options.add(&lifetime);
        options.add(&max_views);
        options.add(&protect);
        options.add(&password);
        options.add(&hide_email);
        options.add(&disabled);
        options.add(&notes);

        let page = adw::PreferencesPage::new();
        page.add(&general);
        if file {
            page.add(&file_group);
        } else {
            page.add(&text_group);
        }
        page.add(&options);

        let banner = adw::Banner::new("");
        let cancel = gtk::Button::with_label("Cancel");
        let save = gtk::Button::builder()
            .label(if creating { "Create" } else { "Save" })
            .css_classes(["suggested-action"])
            .build();
        let header =
            adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
        header.pack_start(&cancel);
        header.pack_end(&save);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.add_top_bar(&banner);
        view.set_content(Some(&page));
        let title = match (creating, file) {
            (true, false) => "New text Send",
            (true, true) => "New file Send",
            (false, _) => "Edit Send",
        };
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(520)
            .content_height(680)
            .child(&view)
            .default_widget(&save)
            .build();

        // Saving needs a name, the text or file to share, and a password
        // while one is required.
        let validate: Rc<dyn Fn()> = Rc::new({
            let (name, text, chosen, protect, password, save) =
                (name.clone(), text.clone(), chosen.clone(), protect.clone(), password.clone(), save.clone());
            move || {
                let named = !name.text().trim().is_empty();
                let content = if file {
                    !creating || chosen.borrow().is_some()
                } else {
                    text.buffer().char_count() > 0
                };
                let locked = !protect.is_active() || !password.text().is_empty();
                save.set_sensitive(named && content && locked);
            }
        });
        validate();
        {
            let validate = validate.clone();
            name.connect_changed(move |_| validate());
        }
        {
            let validate = validate.clone();
            text.buffer().connect_changed(move |_| validate());
        }
        {
            let validate = validate.clone();
            password.connect_changed(move |_| validate());
        }
        {
            let validate = validate.clone();
            protect.connect_active_notify(move |_| validate());
        }
        if file && creating {
            let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
            file_row.add_suffix(&choose);
            file_row.set_activatable_widget(Some(&choose));
            let (chosen, file_row, name, validate) =
                (chosen.clone(), file_row.clone(), name.clone(), validate.clone());
            choose.connect_clicked(move |button| {
                let window = button.root().and_downcast::<gtk::Window>();
                let chooser = gtk::FileDialog::builder().title("Choose a file to send").build();
                let (chosen, file_row, name, validate) =
                    (chosen.clone(), file_row.clone(), name.clone(), validate.clone());
                glib::spawn_future_local(async move {
                    let Ok(picked) = chooser.open_future(window.as_ref()).await else { return };
                    let Some(path) = picked.path() else { return };
                    let shown =
                        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    file_row.set_subtitle(&shown);
                    if name.text().is_empty() {
                        name.set_text(&shown);
                    }
                    *chosen.borrow_mut() = Some(path);
                    validate();
                });
            });
        }

        {
            let dialog = dialog.clone();
            cancel.connect_clicked(move |_| {
                dialog.close();
            });
        }
        {
            // Leave nothing typed behind once the editor closes.
            let (me, text, password) = (Rc::downgrade(self), text.clone(), password.clone());
            dialog.connect_closed(move |dialog| {
                text.buffer().set_text("");
                password.set_text("");
                if let Some(me) = me.upgrade() {
                    let mut editor = me.editor.borrow_mut();
                    if editor.as_ref() == Some(dialog) {
                        editor.take();
                    }
                }
            });
        }
        let form = Form {
            name: name.clone(),
            text,
            hidden,
            lifetime,
            max_views,
            protect,
            password,
            hide_email,
            disabled,
            notes,
        };
        {
            let (me, dialog, banner) = (self.clone(), dialog.clone(), banner.clone());
            save.connect_clicked(move |button| {
                let send = form.apply(&original, file, creating);
                let request = if creating {
                    Request::CreateSend { send, file: chosen.borrow().clone() }
                } else {
                    let id = original["id"].as_str().unwrap_or_default().to_owned();
                    Request::EditSend { id, send }
                };
                button.set_sensitive(false);
                button.set_label(if creating { "Creating…" } else { "Saving…" });
                let (me, dialog, banner, button) =
                    (me.clone(), dialog.clone(), banner.clone(), button.clone());
                glib::spawn_future_local(async move {
                    match me.app.request(request).await {
                        Ok(Response::Json { value }) => {
                            let id = value["id"].as_str().map(str::to_owned);
                            me.upsert(value);
                            dialog.close();
                            match id {
                                // A new Send is made to be shared: copy its link.
                                Some(id) if creating => me.copy_link(&id),
                                _ => (me.notify)("Send saved"),
                            }
                        }
                        Ok(_) => {
                            dialog.close();
                        }
                        Err(e) => {
                            banner.set_title(&e.message);
                            banner.set_revealed(true);
                            button.set_sensitive(true);
                            button.set_label(if creating { "Create" } else { "Save" });
                        }
                    }
                });
            });
        }
        if let Some(previous) = self.editor.borrow_mut().replace(dialog.clone()) {
            previous.force_close();
        }
        dialog.present(Some(&self.root));
        name.grab_focus();
    }
}

struct Form {
    name: adw::EntryRow,
    text: gtk::TextView,
    hidden: adw::SwitchRow,
    lifetime: adw::ComboRow,
    max_views: adw::SpinRow,
    protect: adw::SwitchRow,
    password: adw::PasswordEntryRow,
    hide_email: adw::SwitchRow,
    disabled: adw::SwitchRow,
    notes: adw::EntryRow,
}

impl Form {
    /// The Send to save: `original` with the form applied, so fields the
    /// form does not show, such as the key, keep their values.
    fn apply(&self, original: &Value, file: bool, creating: bool) -> Value {
        let mut send = original.clone();
        send["name"] = Value::from(self.name.text().trim());
        let notes = self.notes.text();
        send["notes"] = if notes.trim().is_empty() { Value::Null } else { Value::from(notes.as_str()) };
        if !file {
            let buffer = self.text.buffer();
            let (start, end) = buffer.bounds();
            send["text"] = json!({ "text": buffer.text(&start, &end, false).as_str(), "hidden": self.hidden.is_active() });
        }
        // Editing offers "Keep" first; creating offers only lifetimes.
        let offset = if creating { 0 } else { 1 };
        let choice = self.lifetime.selected() as usize;
        if let Some((_, hours)) = choice.checked_sub(offset).and_then(|i| LIFETIMES.get(i))
            && let Some(date) = from_now(*hours)
        {
            send["deletionDate"] = Value::from(date);
        }
        let views = self.max_views.value() as u64;
        send["maxAccessCount"] = if views == 0 { Value::Null } else { Value::from(views) };
        let password = self.password.text();
        if self.protect.is_active() && !password.is_empty() {
            send["password"] = Value::from(password.as_str());
        } else {
            // Saving without a password removes any the Send had.
            send["password"] = Value::Null;
        }
        send["hideEmail"] = Value::from(self.hide_email.is_active());
        send["disabled"] = Value::from(self.disabled.is_active());
        send
    }
}

/// A new Send in the shape `bw send template` gives.
fn template(file: bool) -> Value {
    json!({
        "name": "",
        "notes": null,
        "type": if file { 1 } else { 0 },
        "text": if file { Value::Null } else { json!({ "text": "", "hidden": false }) },
        "file": null,
        "maxAccessCount": null,
        "deletionDate": null,
        "expirationDate": null,
        "password": null,
        "disabled": false,
        "hideEmail": false,
    })
}

/// `hours` from now, in the form the CLI expects.
fn from_now(hours: i64) -> Option<String> {
    let date = glib::DateTime::now_utc().ok()?.add_hours(hours as i32).ok()?;
    date.format("%Y-%m-%dT%H:%M:%S.000Z").ok().map(Into::into)
}

/// A CLI date as local time, such as "8 Nov 2026, 14:00".
fn local_date(iso: &str) -> Option<String> {
    let date = glib::DateTime::from_iso8601(iso, None).ok()?.to_local().ok()?;
    date.format("%-d %b %Y, %H:%M").ok().map(Into::into)
}

/// One line about a Send: its kind, views, and when it stops working.
fn summary(send: &Value, now: &glib::DateTime) -> String {
    let mut parts = Vec::new();
    if send["disabled"].as_bool() == Some(true) {
        parts.push("Disabled".to_owned());
    }
    if send["type"].as_i64() == Some(1) {
        let name = send.pointer("/file/fileName").and_then(Value::as_str).unwrap_or("File");
        match send.pointer("/file/sizeName").and_then(Value::as_str) {
            Some(size) => parts.push(format!("{name} ({size})")),
            None => parts.push(name.to_owned()),
        }
    } else {
        parts.push("Text".to_owned());
    }
    let views = send["accessCount"].as_u64().unwrap_or(0);
    match send["maxAccessCount"].as_u64() {
        Some(max) => parts.push(format!("{views} of {max} views")),
        None if views > 0 => parts.push(format!("{views} {}", if views == 1 { "view" } else { "views" })),
        None => {}
    }
    let left = |key: &str| {
        let date = glib::DateTime::from_iso8601(send[key].as_str()?, None).ok()?;
        Some(date.difference(now).as_seconds())
    };
    match (left("expirationDate"), left("deletionDate")) {
        (Some(expires), _) if expires <= 0 => parts.push("Expired".into()),
        (Some(expires), Some(deletes)) if expires < deletes => {
            parts.push(format!("Expires in {}", span(expires)))
        }
        (_, Some(deletes)) => parts.push(format!("Deletes in {}", span(deletes))),
        _ => {}
    }
    parts.join(" · ")
}

fn span(seconds: i64) -> String {
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    match seconds {
        ..3600 => "under an hour".into(),
        // Nearest, so a Send made for 2 days does not read "1 day" a
        // second later.
        3600..84600 => plural((seconds + 1800) / 3600, "hour"),
        _ => plural((seconds + 43200) / 86400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> glib::DateTime {
        glib::DateTime::from_iso8601("2026-11-01T12:00:00Z", None).unwrap()
    }

    #[test]
    fn summaries_say_kind_views_and_lifetime() {
        let text = json!({ "type": 0, "accessCount": 2, "deletionDate": "2026-11-08T12:00:00.000Z" });
        assert_eq!(summary(&text, &now()), "Text · 2 views · Deletes in 7 days");
        let file = json!({
            "type": 1, "disabled": true, "accessCount": 0, "maxAccessCount": 3,
            "file": { "fileName": "plan.pdf", "sizeName": "1.2 MB" },
            "deletionDate": "2026-11-01T15:20:00.000Z"
        });
        assert_eq!(
            summary(&file, &now()),
            "Disabled · plan.pdf (1.2 MB) · 0 of 3 views · Deletes in 3 hours"
        );
        let expired = json!({ "type": 0, "expirationDate": "2026-10-31T00:00:00.000Z", "deletionDate": "2026-11-05T00:00:00.000Z" });
        assert_eq!(summary(&expired, &now()), "Text · Expired");
        let almost = json!({ "type": 0, "deletionDate": "2026-11-03T11:59:00.000Z" });
        assert_eq!(summary(&almost, &now()), "Text · Deletes in 2 days");
        let expiring = json!({ "type": 0, "expirationDate": "2026-11-01T12:30:00.000Z", "deletionDate": "2026-11-05T00:00:00.000Z" });
        assert_eq!(summary(&expiring, &now()), "Text · Expires in under an hour");
    }

    #[test]
    fn templates_match_the_cli() {
        assert_eq!(template(false)["text"]["text"], "");
        assert_eq!(template(true)["type"], 1);
        assert!(template(true)["text"].is_null());
    }

    #[test]
    fn dates_are_in_the_cli_format() {
        let date = from_now(24).unwrap();
        assert!(date.ends_with(".000Z") && date.len() == 24, "{date}");
    }
}
