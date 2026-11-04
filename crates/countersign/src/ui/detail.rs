//! The selected item's details.
//!
//! Items arrive with their secrets removed. A secret is fetched only when
//! you reveal it, and hidden again after a minute or when the item changes.
//! Copying happens in the daemon, so copied values never reach this process.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use countersign_ipc::{ErrorCode, Field, ItemKind, Request, Response, VaultSnapshot};
use gtk::prelude::*;
use gtk::{gio, glib};
use serde_json::Value;

use super::{clear, icon_button, label, monogram, relative_time, short_date};
use crate::app::App;

const MASK: &str = "••••••••••••";
const REVEAL_FOR: Duration = Duration::from_secs(60);

pub struct Detail {
    pub root: gtk::Stack,
    /// The pane's header bar: edit and menu buttons, or while editing the
    /// editor's Cancel and Save.
    pub header: adw::HeaderBar,
    pub(super) actions: gtk::Box,
    pub(super) header_start: gtk::Box,
    pub(super) header_end: gtk::Box,
    pub(super) header_title: gtk::Label,
    content: gtk::Box,
    pub(super) app: Rc<App>,
    pub(super) notify: Rc<dyn Fn(&str)>,
    pub(super) current: RefCell<Option<String>>,
    /// Bumped on every render so stale async results are dropped.
    generation: Cell<u64>,
    timers: RefCell<Vec<glib::SourceId>>,
    pub(super) vault: RefCell<Rc<VaultSnapshot>>,
    pub(super) edit_button: gtk::Button,
    pub(super) menu_button: gtk::MenuButton,
    pub(super) action_group: gio::SimpleActionGroup,
    pub(super) editor: RefCell<Option<Rc<super::editor::Editor>>>,
}

impl Detail {
    pub fn new(app: &Rc<App>, notify: Rc<dyn Fn(&str)>) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("cs-detail");
        let clamp = adw::Clamp::builder().maximum_size(720).tightening_threshold(520).child(&content).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .vexpand(true)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name(super::SENTRY_AT_EASE_ICON)
            .title("No item selected")
            .description("Search or pick an item to see its details.")
            .build();
        empty.add_css_class("compact");
        let root = gtk::Stack::new();
        root.add_named(&empty, Some("empty"));
        root.add_named(&scroller, Some("item"));
        root.set_visible_child_name("empty");
        let edit_button = icon_button("document-edit-symbolic", "Edit (Ctrl+E)");
        let menu_button =
            gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("More").build();
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.append(&edit_button);
        actions.append(&menu_button);
        let header_start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let header_end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let header_title = label("", &["title"]);
        header_title.set_visible(false);
        let header = adw::HeaderBar::builder().title_widget(&header_title).build();
        header.pack_start(&header_start);
        header.pack_end(&actions);
        header.pack_end(&header_end);
        let detail = Rc::new(Detail {
            root,
            header,
            actions,
            header_start,
            header_end,
            header_title,
            content,
            app: app.clone(),
            notify,
            current: RefCell::new(None),
            generation: Cell::new(0),
            timers: RefCell::default(),
            vault: RefCell::default(),
            edit_button,
            menu_button,
            action_group: gio::SimpleActionGroup::new(),
            editor: RefCell::default(),
        });
        detail.install_actions();
        detail
    }

    pub fn set_vault(&self, vault: &Rc<VaultSnapshot>) {
        *self.vault.borrow_mut() = vault.clone();
    }

    /// Whether the editor is open; background refreshes leave it alone.
    pub fn is_editing(&self) -> bool {
        self.editor.borrow().is_some()
    }

    /// Drops the results of any load or reveal still running.
    pub(super) fn invalidate(&self) {
        self.generation.set(self.generation.get() + 1);
    }

    pub fn current(&self) -> Option<String> {
        self.current.borrow().clone()
    }

    fn stop_timers(&self) {
        for source in self.timers.borrow_mut().drain(..) {
            source.remove();
        }
    }

    /// Shows an item, or the empty state for `None`.
    pub fn show(self: &Rc<Self>, id: Option<&str>) {
        if id.is_some() && self.current.borrow().as_deref() == id {
            return;
        }
        self.reload(id);
    }

    /// Shows an item again even if it is already shown.
    pub fn reload(self: &Rc<Self>, id: Option<&str>) {
        self.stop_timers();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        *self.current.borrow_mut() = id.map(str::to_owned);
        // Drop the previous item at once, including any revealed values,
        // and any unsaved edit.
        clear(&self.content);
        self.close_editor();
        self.update_actions(None);
        let Some(id) = id.map(str::to_owned) else {
            self.root.set_visible_child_name("empty");
            return;
        };
        let me = self.clone();
        glib::spawn_future_local(async move {
            let result = me.app.request(Request::Item { id: id.clone() }).await;
            if me.generation.get() != generation {
                return;
            }
            match result {
                Ok(Response::Item { item, secret_fields }) => me.render(&id, &item, &secret_fields),
                Ok(_) => {}
                Err(e) if e.code == ErrorCode::NotFound => me.root.set_visible_child_name("empty"),
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    fn render(self: &Rc<Self>, id: &str, item: &Value, secret_fields: &[Field]) {
        clear(&self.content);
        self.root.set_visible_child_name("item");
        self.update_actions(Some(item));
        let kind = item["type"].as_i64().and_then(ItemKind::from_type).unwrap_or(ItemKind::SecureNote);
        let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        let name = text(&item["name"]).unwrap_or_else(|| "(no name)".into());
        let has = |f: &Field| secret_fields.contains(f);

        // Header.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        header.set_margin_bottom(6);
        header.append(&monogram(&name, true));
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 3);
        titles.set_valign(gtk::Align::Center);
        titles.set_hexpand(true);
        let title = label(&name, &["cs-detail-title"]);
        title.set_wrap(true);
        titles.append(&title);
        let first_uri = item["login"]["uris"][0]["uri"].as_str().unwrap_or_default();
        let subtitle =
            countersign_ipc::summarize(item).and_then(|s| s.hosts.first().cloned()).unwrap_or_else(|| {
                if first_uri.is_empty() { kind.label().to_owned() } else { first_uri.to_owned() }
            });
        titles.append(&label(&subtitle, &["cs-detail-subtitle"]));
        header.append(&titles);
        if item["favorite"].as_bool() == Some(true) {
            let star = gtk::Image::from_icon_name("starred-symbolic");
            star.add_css_class("cs-badge");
            star.add_css_class("favorite");
            star.set_tooltip_text(Some("Favorite"));
            star.set_valign(gtk::Align::Start);
            header.append(&star);
        }
        self.content.append(&header);

        // Chips: kind, folder, organization, protections.
        let chips = adw::WrapBox::builder().child_spacing(6).line_spacing(6).margin_bottom(4).build();
        let chip = |text: &str| {
            let l = label(text, &["cs-chip"]);
            l.set_halign(gtk::Align::Start);
            l
        };
        chips.append(&chip(kind.label()));
        let vault = self.vault.borrow().clone();
        if let Some(folder) = text(&item["folderId"]).and_then(|f| vault.folders.iter().find(|x| x.id == f)) {
            chips.append(&chip(&format!("Folder · {}", folder.name)));
        }
        if let Some(org) =
            text(&item["organizationId"]).and_then(|o| vault.organizations.iter().find(|x| x.id == o))
        {
            chips.append(&chip(&org.name));
        }
        if item["reprompt"].as_i64() == Some(1) {
            chips.append(&chip("Master password re-prompt"));
        }
        if item["login"]["fido2Credentials"].as_array().is_some_and(|p| !p.is_empty()) {
            chips.append(&chip("Passkey"));
        }
        self.content.append(&chips);
        if let Some(deleted) = text(&item["deletedDate"]) {
            let banner = adw::Banner::builder()
                .title(format!("In the trash since {}", short_date(&deleted)))
                .revealed(true)
                .build();
            banner.set_margin_top(6);
            self.content.append(&banner);
        }

        match kind {
            ItemKind::Login => {
                let group = self.group("Login");
                if let Some(user) = text(&item["login"]["username"]) {
                    group.append(&self.plain_field(id, "Username", &user, Some(Field::Username)));
                }
                if has(&Field::Password) {
                    group.append(&self.secret_field(id, "Password", Field::Password));
                }
                if has(&Field::Totp) {
                    group.append(&self.totp_field(id));
                }
                self.finish_group(group);
                if let Some(uris) = item["login"]["uris"].as_array().filter(|u| !u.is_empty()) {
                    let group = self.group("Websites");
                    for (index, uri) in uris.iter().enumerate() {
                        if let Some(uri) = text(&uri["uri"]) {
                            group.append(&self.uri_field(id, index, &uri));
                        }
                    }
                    self.finish_group(group);
                }
            }
            ItemKind::Card => {
                let card = &item["card"];
                let group = self.group("Card");
                if let Some(holder) = text(&card["cardholderName"]) {
                    group.append(&self.plain_field(
                        id,
                        "Cardholder name",
                        &holder,
                        Some(Field::CardholderName),
                    ));
                }
                if has(&Field::CardNumber) {
                    let brand = text(&card["brand"]).unwrap_or_else(|| "Number".into());
                    group.append(&self.secret_field(id, &format!("{brand} number"), Field::CardNumber));
                }
                if let Some(expiry) = Field::CardExpiry.read(item) {
                    group.append(&self.plain_field(id, "Expiration", &expiry, Some(Field::CardExpiry)));
                }
                if has(&Field::CardCode) {
                    group.append(&self.secret_field(id, "Security code", Field::CardCode));
                }
                self.finish_group(group);
            }
            ItemKind::Identity => {
                let identity = &item["identity"];
                let group = self.group("Identity");
                let full: Vec<String> = ["title", "firstName", "middleName", "lastName"]
                    .iter()
                    .filter_map(|k| text(&identity[*k]))
                    .collect();
                if !full.is_empty() {
                    group.append(&self.plain_field(id, "Name", &full.join(" "), None));
                }
                for key in ["username", "company", "email", "phone"] {
                    if let Some(value) = text(&identity[key]) {
                        let field = Field::Identity { name: key.into() };
                        group.append(&self.plain_field(id, &field.label(), &value, Some(field.clone())));
                    }
                }
                for key in ["ssn", "passportNumber", "licenseNumber"] {
                    let field = Field::Identity { name: key.into() };
                    if has(&field) {
                        group.append(&self.secret_field(id, &field.label(), field.clone()));
                    }
                }
                self.finish_group(group);
                let address: Vec<String> = ["address1", "address2", "address3"]
                    .iter()
                    .filter_map(|k| text(&identity[*k]))
                    .chain(
                        [
                            ["city", "state", "postalCode"]
                                .iter()
                                .filter_map(|k| text(&identity[*k]))
                                .collect::<Vec<_>>()
                                .join(", "),
                            text(&identity["country"]).unwrap_or_default(),
                        ]
                        .into_iter()
                        .filter(|s| !s.is_empty()),
                    )
                    .collect();
                if !address.is_empty() {
                    let group = self.group("Address");
                    group.append(&self.plain_field(id, "Address", &address.join("\n"), None));
                    self.finish_group(group);
                }
            }
            ItemKind::SshKey => {
                let key = &item["sshKey"];
                let group = self.group("SSH key");
                if let Some(public) = text(&key["publicKey"]) {
                    group.append(&self.plain_field(id, "Public key", &public, Some(Field::SshPublicKey)));
                }
                if let Some(fingerprint) = text(&key["keyFingerprint"]) {
                    group.append(&self.plain_field(
                        id,
                        "Fingerprint",
                        &fingerprint,
                        Some(Field::SshFingerprint),
                    ));
                }
                if has(&Field::SshPrivateKey) {
                    group.append(&self.secret_field(id, "Private key", Field::SshPrivateKey));
                }
                self.finish_group(group);
            }
            ItemKind::SecureNote => {}
        }

        if text(&item["notes"]).is_none() && has(&Field::Notes) {
            // The item requires the master password; notes stay hidden.
            let group = self.group("Notes");
            group.append(&self.secret_field(id, "Notes", Field::Notes));
            self.finish_group(group);
        }
        if let Some(notes) = text(&item["notes"]) {
            let group = self.group("Notes");
            let body = label(&notes, &["cs-field-value"]);
            body.set_wrap(true);
            body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("cs-notes");
            body.set_hexpand(true);
            row.append(&body);
            let copy = icon_button("edit-copy-symbolic", "Copy notes");
            self.connect_copy(&copy, id, Field::Notes);
            copy.set_valign(gtk::Align::Start);
            row.append(&copy);
            group.append(&row);
            self.finish_group(group);
        }

        if let Some(fields) = item["fields"].as_array().filter(|f| !f.is_empty()) {
            let group = self.group("Custom fields");
            for (index, field) in fields.iter().enumerate() {
                let name = text(&field["name"]).unwrap_or_else(|| "Field".into());
                let reference = Field::Custom { index };
                match field["type"].as_i64() {
                    _ if has(&reference) => group.append(&self.secret_field(id, &name, reference)),
                    Some(2) => {
                        let on = field["value"].as_str() == Some("true");
                        group.append(&self.plain_field(id, &name, if on { "Yes" } else { "No" }, None));
                    }
                    Some(3) => group.append(&self.plain_field(id, &name, "Linked to another field", None)),
                    _ => {
                        let value = text(&field["value"]).unwrap_or_default();
                        group.append(&self.plain_field(id, &name, &value, Some(reference)));
                    }
                }
            }
            self.finish_group(group);
        }

        if let Some(attachments) = item["attachments"].as_array().filter(|a| !a.is_empty()) {
            let group = self.group("Attachments");
            for attachment in attachments {
                let file = text(&attachment["fileName"]).unwrap_or_else(|| "attachment".into());
                let size = text(&attachment["sizeName"]).unwrap_or_default();
                let attachment_id = text(&attachment["id"]).unwrap_or_default();
                group.append(&self.attachment_field(id, &attachment_id, &file, &size));
            }
            self.finish_group(group);
        }

        let history = item["passwordHistory"].as_array().map_or(0, Vec::len);
        if history > 0 {
            let group = self.group("Password history");
            for index in 0..history {
                let used = text(&item["passwordHistory"][index]["lastUsedDate"])
                    .map(|d| format!("Used until {}", short_date(&d)))
                    .unwrap_or_else(|| "Old password".into());
                group.append(&self.secret_field(id, &used, Field::PasswordHistory { index }));
            }
            self.finish_group(group);
        }

        let mut meta = Vec::new();
        if let Some(updated) = text(&item["revisionDate"]) {
            meta.push(format!("Updated {}", relative_time(&updated)));
        }
        if let Some(changed) = text(&item["login"]["passwordRevisionDate"]) {
            meta.push(format!("Password changed {}", short_date(&changed)));
        }
        if let Some(created) = text(&item["creationDate"]) {
            meta.push(format!("Created {}", short_date(&created)));
        }
        let footer = label(&meta.join("  ·  "), &["cs-meta"]);
        footer.set_margin_top(18);
        footer.set_wrap(true);
        self.content.append(&footer);
    }

    fn group(&self, title: &str) -> gtk::Box {
        self.content.append(&label(&title.to_uppercase(), &["cs-group-title"]));
        let fields = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fields.add_css_class("cs-fields");
        fields
    }

    fn finish_group(&self, group: gtk::Box) {
        self.content.append(&group);
    }

    fn row(&self, title: &str, value: &gtk::Widget) -> (gtk::Box, gtk::Box) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("cs-field");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        text.set_hexpand(true);
        text.append(&label(&title.to_uppercase(), &["cs-field-label"]));
        text.append(value);
        row.append(&text);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        buttons.set_valign(gtk::Align::Center);
        row.append(&buttons);
        (row, buttons)
    }

    fn plain_field(self: &Rc<Self>, id: &str, title: &str, value: &str, copy: Option<Field>) -> gtk::Box {
        // Values are not selectable: copying goes through the daemon, which
        // marks it sensitive and clears it.
        let value_label = label(value, &["cs-field-value"]);
        value_label.set_wrap(true);
        value_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let (row, buttons) = self.row(title, value_label.upcast_ref());
        if let Some(field) = copy {
            let button = icon_button("edit-copy-symbolic", &format!("Copy {}", title.to_lowercase()));
            self.connect_copy(&button, id, field);
            buttons.append(&button);
        }
        row
    }

    fn secret_field(self: &Rc<Self>, id: &str, title: &str, field: Field) -> gtk::Box {
        let value = label(MASK, &["cs-field-value", "secret"]);
        value.set_wrap(true);
        value.set_wrap_mode(gtk::pango::WrapMode::Char);
        let (row, buttons) = self.row(title, value.upcast_ref());
        let reveal = icon_button("view-reveal-symbolic", &format!("Show {}", title.to_lowercase()));
        let copy = icon_button("edit-copy-symbolic", &format!("Copy {}", title.to_lowercase()));
        buttons.append(&reveal);
        buttons.append(&copy);
        self.connect_copy(&copy, id, field.clone());
        let me = Rc::downgrade(self);
        let id = id.to_owned();
        let shown = Rc::new(Cell::new(false));
        reveal.connect_clicked(move |button| {
            let Some(me) = me.upgrade() else { return };
            if shown.get() {
                value.set_label(MASK);
                value.set_selectable(false);
                value.add_css_class("secret");
                button.set_icon_name("view-reveal-symbolic");
                shown.set(false);
                return;
            }
            let (value, button, shown, field, id) =
                (value.clone(), button.clone(), shown.clone(), field.clone(), id.clone());
            let generation = me.generation.get();
            glib::spawn_future_local(async move {
                let result = me.app.reveal(button.upcast_ref(), &id, field).await;
                if me.generation.get() != generation {
                    return;
                }
                match result {
                    Ok(Some(secret)) => {
                        value.set_label(secret.expose());
                        value.remove_css_class("secret");
                        button.set_icon_name("view-conceal-symbolic");
                        shown.set(true);
                        let (value, button, shown) = (value.clone(), button.clone(), shown.clone());
                        // Not tracked in `timers`: a one-shot source is gone once
                        // it fires, and removing it again would panic. The
                        // generation check skips items no longer shown.
                        let weak = Rc::downgrade(&me);
                        glib::timeout_add_local_once(REVEAL_FOR, move || {
                            if weak.upgrade().is_none_or(|me| me.generation.get() != generation) {
                                return;
                            }
                            value.set_label(MASK);
                            value.set_selectable(false);
                            value.add_css_class("secret");
                            button.set_icon_name("view-reveal-symbolic");
                            shown.set(false);
                        });
                    }
                    Ok(None) => {}
                    Err(e) => (me.notify)(&e.message),
                }
            });
        });
        row
    }

    fn totp_field(self: &Rc<Self>, id: &str) -> gtk::Box {
        let code = label("••• •••", &["cs-field-value", "code"]);
        let (row, buttons) = self.row("Verification code", code.upcast_ref());
        let countdown = gtk::Label::new(None);
        countdown.add_css_class("cs-meta");
        let ring = gtk::LevelBar::builder().min_value(0.0).max_value(30.0).width_request(36).build();
        ring.add_css_class("cs-totp-ring");
        ring.set_valign(gtk::Align::Center);
        let copy = icon_button("edit-copy-symbolic", "Copy verification code");
        self.connect_copy(&copy, id, Field::Totp);
        buttons.append(&countdown);
        buttons.append(&ring);
        buttons.append(&copy);

        let me = Rc::downgrade(self);
        let id = id.to_owned();
        let generation = self.generation.get();
        let remaining = Rc::new(Cell::new(0u32));
        let fetch: Rc<dyn Fn()> = Rc::new({
            let (code, countdown, ring, remaining, copy) =
                (code.clone(), countdown.clone(), ring.clone(), remaining.clone(), copy.clone());
            move || {
                let Some(me) = me.upgrade() else { return };
                let (code, countdown, ring, remaining, copy, id) = (
                    code.clone(),
                    countdown.clone(),
                    ring.clone(),
                    remaining.clone(),
                    copy.clone(),
                    id.clone(),
                );
                glib::spawn_future_local(async move {
                    let result = me.app.request(Request::Totp { id }).await;
                    if me.generation.get() != generation {
                        return;
                    }
                    match result {
                        Ok(Response::Totp { totp }) => {
                            let digits = totp.code.expose();
                            let shown = if digits.len() == 6 {
                                format!("{} {}", &digits[..3], &digits[3..])
                            } else {
                                digits.to_owned()
                            };
                            code.set_label(&shown);
                            ring.set_max_value(f64::from(totp.period));
                            remaining.set(totp.remaining);
                            ring.set_value(f64::from(totp.remaining));
                            countdown.set_label(&format!("{}s", totp.remaining));
                        }
                        Err(e) if e.code == ErrorCode::Premium => {
                            code.set_label("Needs Bitwarden Premium");
                            code.remove_css_class("code");
                            code.add_css_class("cs-meta");
                            countdown.set_visible(false);
                            ring.set_visible(false);
                            copy.set_sensitive(false);
                        }
                        Err(e) if e.code == ErrorCode::RepromptRequired => {
                            code.set_label("Protected · use copy to unlock");
                            code.remove_css_class("code");
                            code.add_css_class("cs-meta");
                            countdown.set_visible(false);
                            ring.set_visible(false);
                        }
                        _ => {}
                    }
                });
            }
        });
        fetch();
        let tick_fetch = fetch.clone();
        let source = glib::timeout_add_seconds_local(1, move || {
            let left = remaining.get();
            if left <= 1 {
                remaining.set(0);
                tick_fetch();
            } else {
                remaining.set(left - 1);
                ring.set_value(f64::from(left - 1));
                countdown.set_label(&format!("{}s", left - 1));
            }
            glib::ControlFlow::Continue
        });
        self.timers.borrow_mut().push(source);
        row
    }

    fn uri_field(self: &Rc<Self>, id: &str, index: usize, uri: &str) -> gtk::Box {
        let value = label(uri, &["cs-field-value"]);
        value.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let (row, buttons) = self.row(if index == 0 { "Website" } else { "Also" }, value.upcast_ref());
        if let Some(target) = launchable(uri) {
            let open = icon_button("adw-external-link-symbolic", "Open website");
            open.connect_clicked(move |button| {
                let window = button.root().and_downcast::<gtk::Window>();
                gtk::UriLauncher::new(&target).launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            });
            buttons.append(&open);
        }
        let copy = icon_button("edit-copy-symbolic", "Copy website");
        self.connect_copy(&copy, id, Field::Uri { index });
        buttons.append(&copy);
        row
    }

    fn attachment_field(self: &Rc<Self>, id: &str, attachment_id: &str, file: &str, size: &str) -> gtk::Box {
        let value = label(file, &["cs-field-value"]);
        value.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let (row, buttons) = self.row(size, value.upcast_ref());
        let save = icon_button("document-save-symbolic", "Save attachment");
        buttons.append(&save);
        let delete = icon_button("user-trash-symbolic", "Delete attachment");
        buttons.append(&delete);
        {
            let weak = Rc::downgrade(self);
            let (item_id, attachment, name) = (id.to_owned(), attachment_id.to_owned(), file.to_owned());
            delete.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.delete_attachment(item_id.clone(), attachment.clone(), name.clone());
                }
            });
        }
        let me = Rc::downgrade(self);
        let (id, attachment_id, file) = (id.to_owned(), attachment_id.to_owned(), file.to_owned());
        save.connect_clicked(move |button| {
            let Some(me) = me.upgrade() else { return };
            let window = button.root().and_downcast::<gtk::Window>();
            let dialog =
                gtk::FileDialog::builder().title("Save attachment").initial_name(file.as_str()).build();
            let (button, id, attachment_id) = (button.clone(), id.clone(), attachment_id.clone());
            glib::spawn_future_local(async move {
                let Ok(target) = dialog.save_future(window.as_ref()).await else { return };
                let Some(path) = target.path() else { return };
                let result = me
                    .app
                    .with_reprompt(button.upcast_ref(), &id, || Request::SaveAttachment {
                        item_id: id.clone(),
                        attachment_id: attachment_id.clone(),
                        path: path.clone(),
                    })
                    .await;
                match result {
                    Ok(Some(_)) => (me.notify)("Attachment saved"),
                    Ok(None) => {}
                    Err(e) => (me.notify)(&e.message),
                }
            });
        });
        row
    }

    fn connect_copy(self: &Rc<Self>, button: &gtk::Button, id: &str, field: Field) {
        let me = Rc::downgrade(self);
        let id = id.to_owned();
        button.connect_clicked(move |button| {
            let Some(me) = me.upgrade() else { return };
            let (button, id, field) = (button.clone(), id.clone(), field.clone());
            glib::spawn_future_local(async move {
                if let Err(e) = me.app.copy(button.upcast_ref(), &id, field).await {
                    (me.notify)(&e.message);
                }
            });
        });
    }
}

/// A web address safe to hand to the browser, adding `https://` when the
/// item stores a bare host. Other schemes are not opened.
pub fn launchable(uri: &str) -> Option<String> {
    let uri = uri.trim();
    if uri.starts_with("https://") || uri.starts_with("http://") {
        return Some(uri.to_owned());
    }
    if uri.is_empty() || uri.contains("://") || uri.contains(char::is_whitespace) {
        return None;
    }
    // A bare host may carry a port, but no other colon: `javascript:` and
    // `mailto:` are not hosts.
    let host = uri.split('/').next().unwrap_or_default();
    let plain = match host.split_once(':') {
        None => true,
        Some((_, port)) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
    };
    plain.then(|| format!("https://{uri}"))
}

#[cfg(test)]
mod tests {
    use super::launchable;

    #[test]
    fn only_web_addresses_open() {
        assert_eq!(launchable("https://github.com").as_deref(), Some("https://github.com"));
        assert_eq!(launchable("github.com").as_deref(), Some("https://github.com"));
        assert_eq!(launchable("javascript:alert(1)"), None);
        assert_eq!(launchable("localhost:8080/x").as_deref(), Some("https://localhost:8080/x"));
        assert_eq!(launchable("file:///etc/passwd"), None);
        assert_eq!(launchable("androidapp://com.x"), None);
    }
}
