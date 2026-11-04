//! Creating and editing items.
//!
//! Editing starts from the complete item the CLI returned and changes only
//! the fields this form shows, so anything Countersign does not edit, such
//! as passkeys, password history or linked fields, is sent back unchanged.

use std::cell::{Cell, RefCell};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{GenerateOptions, ItemKind, Request, Response, VaultSnapshot};
use gtk::glib;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::app::App;

const CARD_BRANDS: [&str; 11] = [
    "",
    "Visa",
    "Mastercard",
    "Amex",
    "Discover",
    "Diners Club",
    "JCB",
    "Maestro",
    "UnionPay",
    "RuPay",
    "Other",
];
const TITLES: [&str; 6] = ["", "Mr", "Mrs", "Ms", "Mx", "Dr"];
const IDENTITY_FIELDS: [(&str, &str, bool); 17] = [
    ("firstName", "First name", false),
    ("middleName", "Middle name", false),
    ("lastName", "Last name", false),
    ("username", "Username", false),
    ("company", "Company", false),
    ("email", "Email", false),
    ("phone", "Phone", false),
    ("ssn", "Social security number", true),
    ("passportNumber", "Passport number", true),
    ("licenseNumber", "License number", true),
    ("address1", "Address", false),
    ("address2", "Address 2", false),
    ("address3", "Address 3", false),
    ("city", "City", false),
    ("state", "State or province", false),
    ("postalCode", "Postal code", false),
    ("country", "Country", false),
];

/// A new, empty item of `kind`.
pub fn template(kind: ItemKind, folder_id: Option<String>) -> Value {
    let mut item = json!({
        "type": kind.type_number(),
        "name": "",
        "notes": null,
        "favorite": false,
        "folderId": folder_id,
        "organizationId": null,
        "reprompt": 0,
        "fields": [],
    });
    match kind {
        ItemKind::Login => {
            item["login"] = json!({"username": null, "password": null, "totp": null, "uris": []})
        }
        ItemKind::SecureNote => item["secureNote"] = json!({"type": 0}),
        ItemKind::Card => {
            item["card"] = json!({"cardholderName": null, "brand": null, "number": null,
                                  "expMonth": null, "expYear": null, "code": null})
        }
        ItemKind::Identity => {
            let mut identity = serde_json::Map::new();
            identity.insert("title".into(), Value::Null);
            for (key, _, _) in IDENTITY_FIELDS {
                identity.insert(key.into(), Value::Null);
            }
            item["identity"] = Value::Object(identity);
        }
        ItemKind::SshKey => {}
    }
    item
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// The text as typed, or JSON null when empty. For values whose spaces may
/// matter, such as notes and custom fields.
fn keep(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { Value::from(s) }
}

/// A trimmed string, or JSON null when empty.
fn opt(s: &str) -> Value {
    let s = s.trim();
    if s.is_empty() { Value::Null } else { Value::from(s) }
}

fn entry(title: &str, value: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).text(value).build()
}

fn secret(title: &str, value: &str) -> adw::PasswordEntryRow {
    adw::PasswordEntryRow::builder().title(title).text(value).build()
}

fn combo(title: &str, options: &[&str], selected: &str) -> adw::ComboRow {
    let model = gtk::StringList::new(options);
    let index = options.iter().position(|o| o.eq_ignore_ascii_case(selected)).unwrap_or(0);
    adw::ComboRow::builder().title(title).model(&model).selected(index as u32).build()
}

fn combo_value(row: &adw::ComboRow, options: &[&str]) -> Value {
    opt(options.get(row.selected() as usize).copied().unwrap_or_default())
}

/// A custom field row pair and where it came from.
struct CustomField {
    original: Option<usize>,
    kind: i64,
    name: adw::EntryRow,
    value: FieldValue,
}

enum FieldValue {
    Text(adw::EntryRow),
    Hidden(adw::PasswordEntryRow),
    Boolean(adw::SwitchRow),
    /// Linked fields point at another field; they are kept as they are.
    Linked,
}

pub struct Editor {
    pub root: gtk::Widget,
    app: Rc<App>,
    original: Value,
    creating: bool,
    on_done: Rc<dyn Fn(Option<String>)>,
    name: adw::EntryRow,
    folder: adw::ComboRow,
    folder_ids: Vec<Option<String>>,
    favorite: adw::SwitchRow,
    reprompt: adw::SwitchRow,
    username: Option<adw::EntryRow>,
    password: Option<adw::PasswordEntryRow>,
    totp: Option<adw::PasswordEntryRow>,
    uris: RefCell<Vec<(Option<usize>, adw::EntryRow)>>,
    uri_group: Option<adw::PreferencesGroup>,
    card: Option<[gtk::Widget; 6]>,
    identity: Vec<(&'static str, gtk::Widget)>,
    title: Option<adw::ComboRow>,
    notes: gtk::TextView,
    fields: RefCell<Vec<CustomField>>,
    fields_group: adw::PreferencesGroup,
    /// The "Add …" rows, kept last in their groups.
    uri_add: RefCell<Option<adw::ButtonRow>>,
    field_add: RefCell<Option<adw::ButtonRow>>,
    error: gtk::Label,
    /// Cancel and Save, for the pane's header bar.
    pub cancel: gtk::Button,
    pub save: gtk::Button,
    pub heading: &'static str,
    /// A fingerprint of the form as it opened, to tell whether it changed.
    baseline: Cell<u64>,
}

impl Editor {
    pub fn new(
        app: &Rc<App>,
        item: Value,
        vault: &VaultSnapshot,
        creating: bool,
        on_done: Rc<dyn Fn(Option<String>)>,
    ) -> Rc<Self> {
        let kind = item["type"].as_i64().and_then(ItemKind::from_type).unwrap_or(ItemKind::SecureNote);
        let page = adw::PreferencesPage::new();

        // The item itself.
        let basics = adw::PreferencesGroup::new();
        let name = entry("Name", &text(&item["name"]));
        let mut folder_names = vec!["No folder".to_owned()];
        let mut folder_ids = vec![None];
        for folder in &vault.folders {
            folder_names.push(folder.name.clone());
            folder_ids.push(Some(folder.id.clone()));
        }
        let current_folder = item["folderId"].as_str().map(str::to_owned);
        let names: Vec<&str> = folder_names.iter().map(String::as_str).collect();
        let folder = adw::ComboRow::builder().title("Folder").model(&gtk::StringList::new(&names)).build();
        folder.set_selected(folder_ids.iter().position(|f| *f == current_folder).unwrap_or(0) as u32);
        let favorite = adw::SwitchRow::builder()
            .title("Favorite")
            .active(item["favorite"].as_bool() == Some(true))
            .build();
        let reprompt = adw::SwitchRow::builder()
            .title("Ask for the master password")
            .subtitle("Before this item is shown in full, copied or edited")
            .active(item["reprompt"].as_i64() == Some(1))
            .build();
        basics.add(&name);
        basics.add(&folder);
        basics.add(&favorite);
        basics.add(&reprompt);
        page.add(&basics);

        let (mut username, mut password, mut totp, mut uri_group) = (None, None, None, None);
        let mut uris = Vec::new();
        let mut card = None;
        let mut identity = Vec::new();
        let mut title = None;
        match kind {
            ItemKind::Login => {
                let group = adw::PreferencesGroup::builder().title("Login").build();
                let user = entry("Username", &text(&item["login"]["username"]));
                let pass = secret("Password", &text(&item["login"]["password"]));
                let generate = super::icon_button("view-refresh-symbolic", "Generate a password");
                pass.add_suffix(&generate);
                {
                    // Weak: a row holding its own handler would never be
                    // freed, and its password with it.
                    let (app, pass) = (app.clone(), pass.downgrade());
                    generate.connect_clicked(move |_| {
                        let (app, pass) = (app.clone(), pass.clone());
                        glib::spawn_future_local(async move {
                            let options = GenerateOptions::default();
                            if let Ok(Response::Secret { value }) =
                                app.request(Request::Generate { options }).await
                                && let Some(pass) = pass.upgrade()
                            {
                                pass.set_text(value.expose());
                            }
                        });
                    });
                }
                let key = secret("Authenticator key (TOTP)", &text(&item["login"]["totp"]));
                group.add(&user);
                group.add(&pass);
                group.add(&key);
                page.add(&group);
                let websites = adw::PreferencesGroup::builder().title("Websites").build();
                for (index, uri) in item["login"]["uris"].as_array().into_iter().flatten().enumerate() {
                    uris.push((Some(index), entry("Website", &text(&uri["uri"]))));
                }
                page.add(&websites);
                username = Some(user);
                password = Some(pass);
                totp = Some(key);
                uri_group = Some(websites);
            }
            ItemKind::Card => {
                let c = &item["card"];
                let group = adw::PreferencesGroup::builder().title("Card").build();
                let holder = entry("Cardholder name", &text(&c["cardholderName"]));
                let brand = combo("Brand", &CARD_BRANDS, &text(&c["brand"]));
                let number = secret("Number", &text(&c["number"]));
                let months: Vec<String> =
                    std::iter::once(String::new()).chain((1..=12).map(|m| format!("{m:02}"))).collect();
                let month_names: Vec<&str> = months.iter().map(String::as_str).collect();
                let month = adw::ComboRow::builder()
                    .title("Expiration month")
                    .model(&gtk::StringList::new(&month_names))
                    .build();
                let stored =
                    text(&c["expMonth"]).parse::<usize>().ok().filter(|m| (1..=12).contains(m)).unwrap_or(0);
                month.set_selected(stored as u32);
                let year = entry("Expiration year", &text(&c["expYear"]));
                let code = secret("Security code", &text(&c["code"]));
                for row in [
                    holder.upcast_ref::<gtk::Widget>(),
                    brand.upcast_ref(),
                    number.upcast_ref(),
                    month.upcast_ref(),
                    year.upcast_ref(),
                    code.upcast_ref(),
                ] {
                    group.add(row);
                }
                page.add(&group);
                card = Some([
                    holder.upcast(),
                    brand.upcast(),
                    number.upcast(),
                    month.upcast(),
                    year.upcast(),
                    code.upcast(),
                ]);
            }
            ItemKind::Identity => {
                let i = &item["identity"];
                let group = adw::PreferencesGroup::builder().title("Identity").build();
                let t = combo("Title", &TITLES, &text(&i["title"]));
                group.add(&t);
                for (key, label, hidden) in IDENTITY_FIELDS {
                    let row: gtk::Widget = if hidden {
                        secret(label, &text(&i[key])).upcast()
                    } else {
                        entry(label, &text(&i[key])).upcast()
                    };
                    group.add(&row);
                    identity.push((key, row));
                }
                page.add(&group);
                title = Some(t);
            }
            ItemKind::SshKey => {
                // Bitwarden derives the public key and fingerprint from the
                // private key, which the CLI cannot do; the key stays as it is.
                let k = &item["sshKey"];
                let group = adw::PreferencesGroup::builder()
                    .title("SSH key")
                    .description("The key itself cannot be changed.")
                    .build();
                for (label, value) in [("Public key", &k["publicKey"]), ("Fingerprint", &k["keyFingerprint"])]
                {
                    let row = adw::ActionRow::builder()
                        .title(label)
                        .subtitle(text(value))
                        .subtitle_selectable(true)
                        .use_markup(false)
                        .build();
                    row.add_css_class("property");
                    group.add(&row);
                }
                page.add(&group);
            }
            ItemKind::SecureNote => {}
        }

        let notes_group = adw::PreferencesGroup::builder().title("Notes").build();
        let notes = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(10)
            .bottom_margin(10)
            .left_margin(12)
            .right_margin(12)
            .build();
        notes.buffer().set_text(item["notes"].as_str().unwrap_or_default());
        notes.update_property(&[gtk::accessible::Property::Label("Notes")]);
        let notes_frame = gtk::ScrolledWindow::builder()
            .child(&notes)
            .min_content_height(if kind == ItemKind::SecureNote { 220 } else { 110 })
            .css_classes(["card"])
            .build();
        notes_group.add(&notes_frame);
        page.add(&notes_group);

        let fields_group = adw::PreferencesGroup::builder().title("Custom fields").build();
        page.add(&fields_group);

        let error = super::label("", &["cs-error"]);
        error.set_visible(false);
        error.set_wrap(true);
        let cancel = gtk::Button::with_label("Cancel");
        let save = gtk::Button::builder()
            .label(if creating { "Create" } else { "Save" })
            .css_classes(["suggested-action"])
            .build();
        cancel.set_tooltip_text(Some("Cancel (Esc)"));
        save.set_tooltip_text(Some("Save (Ctrl+S)"));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&error);
        column.append(&page);

        let editor = Rc::new(Editor {
            root: column.upcast(),
            app: app.clone(),
            original: item.clone(),
            creating,
            on_done,
            name,
            folder,
            folder_ids,
            favorite,
            reprompt,
            username,
            password,
            totp,
            uris: RefCell::default(),
            uri_group,
            card,
            identity,
            title,
            notes,
            fields: RefCell::default(),
            fields_group,
            uri_add: RefCell::default(),
            field_add: RefCell::default(),
            error,
            cancel: cancel.clone(),
            save: save.clone(),
            heading: if creating { "New item" } else { "Edit item" },
            baseline: Cell::new(0),
        });
        for (original, row) in uris {
            editor.add_uri(original, row);
        }
        if let Some(group) = &editor.uri_group {
            let add =
                adw::ButtonRow::builder().title("Add website").start_icon_name("list-add-symbolic").build();
            let weak = Rc::downgrade(&editor);
            add.connect_activated(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.add_uri(None, entry("Website", ""));
                }
            });
            group.add(&add);
            *editor.uri_add.borrow_mut() = Some(add);
        }
        for (index, field) in item["fields"].as_array().into_iter().flatten().enumerate() {
            editor.add_field(
                Some(index),
                field["type"].as_i64().unwrap_or(0),
                &text(&field["name"]),
                field["value"].as_str(),
            );
        }
        let add_field =
            adw::ButtonRow::builder().title("Add a field").start_icon_name("list-add-symbolic").build();
        {
            let weak = Rc::downgrade(&editor);
            add_field.connect_activated(move |row| {
                if let Some(me) = weak.upgrade() {
                    me.choose_field_kind(row.upcast_ref());
                }
            });
        }
        editor.fields_group.add(&add_field);
        *editor.field_add.borrow_mut() = Some(add_field);
        {
            let weak = Rc::downgrade(&editor);
            cancel.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.cancel();
                }
            });
        }
        {
            let weak = Rc::downgrade(&editor);
            save.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.save();
                }
            });
        }
        editor.baseline.set(editor.fingerprint());
        editor
    }

    /// A hash of the item the form would save. The serialized item holds
    /// secrets, so it is wiped once hashed.
    fn fingerprint(&self) -> u64 {
        let text = Zeroizing::new(serde_json::to_string(&self.build()).unwrap_or_default());
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Leaves the editor, first asking whether to discard any changes.
    pub fn cancel(self: &Rc<Self>) {
        if self.fingerprint() == self.baseline.get() {
            (self.on_done)(None);
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Discard changes?")
            .body(if self.creating { "This item has not been created." } else { "Your edits are not saved." })
            .close_response("keep")
            .default_response("keep")
            .build();
        dialog.add_responses(&[("keep", "Keep editing"), ("discard", "Discard")]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if response == "discard"
                && let Some(me) = weak.upgrade()
                // Locking may have closed the editor meanwhile.
                && me.root.parent().is_some()
            {
                (me.on_done)(None);
            }
        });
        dialog.present(Some(&self.root));
    }

    pub fn focus(&self) {
        self.name.grab_focus();
    }

    fn add_uri(self: &Rc<Self>, original: Option<usize>, row: adw::EntryRow) {
        let Some(group) = &self.uri_group else { return };
        let remove = super::icon_button("user-trash-symbolic", "Remove website");
        row.add_suffix(&remove);
        let add = self.uri_add.borrow().clone();
        if let Some(add) = &add {
            group.remove(add);
        }
        group.add(&row);
        if let Some(add) = &add {
            group.add(add);
        }
        self.uris.borrow_mut().push((original, row.clone()));
        let (weak, row_ref) = (Rc::downgrade(self), row.downgrade());
        remove.connect_clicked(move |_| {
            let (Some(me), Some(row)) = (weak.upgrade(), row_ref.upgrade()) else { return };
            if let Some(group) = &me.uri_group {
                group.remove(&row);
            }
            me.uris.borrow_mut().retain(|(_, r)| r != &row);
        });
    }

    fn choose_field_kind(self: &Rc<Self>, anchor: &gtk::Widget) {
        let popover = gtk::Popover::new();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for (kind, label) in [(0, "Text"), (1, "Hidden"), (2, "Checkbox")] {
            let button = gtk::Button::builder().label(label).css_classes(["flat"]).build();
            let (weak, popover) = (Rc::downgrade(self), popover.downgrade());
            button.connect_clicked(move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                if let Some(me) = weak.upgrade() {
                    me.add_field(None, kind, "", None);
                }
            });
            column.append(&button);
        }
        popover.set_child(Some(&column));
        popover.set_parent(anchor);
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    fn add_field(self: &Rc<Self>, original: Option<usize>, kind: i64, name: &str, value: Option<&str>) {
        let name_row = entry("Field name", name);
        let value_widget = match kind {
            1 => FieldValue::Hidden(secret("Value", value.unwrap_or_default())),
            2 => FieldValue::Boolean(
                adw::SwitchRow::builder().title("Checked").active(value == Some("true")).build(),
            ),
            3 => FieldValue::Linked,
            _ => FieldValue::Text(entry("Value", value.unwrap_or_default())),
        };
        let remove = super::icon_button("user-trash-symbolic", "Remove field");
        name_row.add_suffix(&remove);
        let add = self.field_add.borrow().clone();
        if let Some(add) = &add {
            self.fields_group.remove(add);
        }
        self.fields_group.add(&name_row);
        let value_row: Option<gtk::Widget> = match &value_widget {
            FieldValue::Text(r) => Some(r.clone().upcast()),
            FieldValue::Hidden(r) => Some(r.clone().upcast()),
            FieldValue::Boolean(r) => Some(r.clone().upcast()),
            FieldValue::Linked => {
                name_row.set_tooltip_text(Some("Linked to another field; kept as it is"));
                None
            }
        };
        if let Some(row) = &value_row {
            self.fields_group.add(row);
        }
        if let Some(add) = &add {
            self.fields_group.add(add);
        }
        self.fields.borrow_mut().push(CustomField {
            original,
            kind,
            name: name_row.clone(),
            value: value_widget,
        });
        // Weak rows: the name row holds this handler, and a hidden value
        // must not outlive the editor.
        let weak = Rc::downgrade(self);
        let name_ref = name_row.downgrade();
        let value_ref = value_row.map(|row| row.downgrade());
        remove.connect_clicked(move |_| {
            let (Some(me), Some(name)) = (weak.upgrade(), name_ref.upgrade()) else { return };
            me.fields_group.remove(&name);
            if let Some(row) = value_ref.as_ref().and_then(|r| r.upgrade()) {
                me.fields_group.remove(&row);
            }
            me.fields.borrow_mut().retain(|f| f.name != name);
        });
    }

    /// The item with the form's values applied to the original.
    fn build(&self) -> Value {
        let mut item = self.original.clone();
        item["name"] = Value::from(self.name.text().trim());
        item["folderId"] = self
            .folder_ids
            .get(self.folder.selected() as usize)
            .cloned()
            .flatten()
            .map_or(Value::Null, Value::from);
        item["favorite"] = Value::from(self.favorite.is_active());
        item["reprompt"] = Value::from(i64::from(self.reprompt.is_active()));
        let buffer = self.notes.buffer();
        item["notes"] = keep(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false));

        if let (Some(user), Some(pass), Some(key)) = (&self.username, &self.password, &self.totp) {
            item["login"]["username"] = opt(&user.text());
            // Passwords keep their spaces; only an empty one becomes null.
            let password = pass.text();
            item["login"]["password"] =
                if password.is_empty() { Value::Null } else { Value::from(password.as_str()) };
            item["login"]["totp"] = opt(&key.text());
            let old = self.original["login"]["uris"].as_array().cloned().unwrap_or_default();
            let uris: Vec<Value> = self
                .uris
                .borrow()
                .iter()
                .filter(|(_, row)| !row.text().trim().is_empty())
                .map(|(index, row)| {
                    let mut uri =
                        index.and_then(|i| old.get(i).cloned()).unwrap_or_else(|| json!({"match": null}));
                    uri["uri"] = Value::from(row.text().trim());
                    uri
                })
                .collect();
            item["login"]["uris"] = Value::Array(uris);
        }
        if let Some([holder, brand, number, month, year, code]) = &self.card {
            let entry_text = |w: &gtk::Widget| {
                w.downcast_ref::<gtk::Editable>().map(|e| e.text().to_string()).unwrap_or_default()
            };
            item["card"]["cardholderName"] = opt(&entry_text(holder));
            if let Some(brand) = brand.downcast_ref::<adw::ComboRow>() {
                item["card"]["brand"] = combo_value(brand, &CARD_BRANDS);
            }
            item["card"]["number"] = opt(&entry_text(number));
            if let Some(month) = month.downcast_ref::<adw::ComboRow>() {
                item["card"]["expMonth"] = if month.selected() == 0 {
                    Value::Null
                } else {
                    Value::from(month.selected().to_string())
                };
            }
            item["card"]["expYear"] = opt(&entry_text(year));
            item["card"]["code"] = opt(&entry_text(code));
        }
        if let Some(title) = &self.title {
            item["identity"]["title"] = combo_value(title, &TITLES);
        }
        for (key, row) in &self.identity {
            if let Some(editable) = row.downcast_ref::<gtk::Editable>() {
                item["identity"][*key] = opt(&editable.text());
            }
        }

        let old = self.original["fields"].as_array().cloned().unwrap_or_default();
        let fields: Vec<Value> = self
            .fields
            .borrow()
            .iter()
            .map(|field| {
                let mut out = field
                    .original
                    .and_then(|i| old.get(i).cloned())
                    .unwrap_or_else(|| json!({"linkedId": null}));
                out["name"] = opt(&field.name.text());
                out["type"] = Value::from(field.kind);
                match &field.value {
                    FieldValue::Text(row) => out["value"] = keep(&row.text()),
                    FieldValue::Hidden(row) => out["value"] = keep(&row.text()),
                    FieldValue::Boolean(row) => {
                        out["value"] = Value::from(if row.is_active() { "true" } else { "false" })
                    }
                    FieldValue::Linked => {}
                }
                out
            })
            .collect();
        item["fields"] = Value::Array(fields);
        item
    }

    pub fn save(self: &Rc<Self>) {
        if !self.save.is_sensitive() {
            return;
        }
        if self.name.text().trim().is_empty() {
            self.show_error("Give the item a name.");
            self.name.grab_focus();
            return;
        }
        let item = self.build();
        let request = if self.creating {
            Request::CreateItem { item }
        } else {
            let id = text(&self.original["id"]);
            Request::EditItem { id, item }
        };
        self.save.set_sensitive(false);
        let me = self.clone();
        glib::spawn_future_local(async move {
            let result = me.app.request(request).await;
            me.save.set_sensitive(true);
            // Cancelled or replaced while saving: another editor may be open.
            if me.root.parent().is_none() {
                return;
            }
            match result {
                Ok(Response::Item { item, .. }) => (me.on_done)(item["id"].as_str().map(str::to_owned)),
                Ok(_) => (me.on_done)(None),
                Err(e) => me.show_error(&e.message),
            }
        });
    }

    fn show_error(&self, message: &str) {
        self.error.set_label(message);
        self.error.set_visible(true);
        self.error.set_margin_top(10);
        self.error.set_margin_start(16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_their_sections() {
        assert!(template(ItemKind::Login, None)["login"]["uris"].is_array());
        assert_eq!(template(ItemKind::SecureNote, None)["secureNote"]["type"], 0);
        assert!(template(ItemKind::Card, None)["card"].is_object());
        assert!(template(ItemKind::Identity, Some("f".into()))["identity"]["passportNumber"].is_null());
        assert_eq!(template(ItemKind::Identity, Some("f".into()))["folderId"], "f");
    }

    #[test]
    fn empty_text_becomes_null() {
        assert_eq!(opt("  "), Value::Null);
        assert_eq!(opt(" a "), Value::from("a"));
        assert_eq!(keep(""), Value::Null);
        assert_eq!(keep(" token \n"), Value::from(" token \n"));
    }
}
