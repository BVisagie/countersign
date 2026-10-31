//! The list of vault items, shared by the main window and quick access.

use std::rc::Rc;

use countersign_ipc::{ItemKind, ItemSummary};
use gtk::prelude::*;
use gtk::{gio, glib};

use super::{label, set_monogram};

#[derive(Clone)]
pub struct ItemList {
    pub view: gtk::ListView,
    store: gio::ListStore,
    pub selection: gtk::SingleSelection,
}

struct Row {
    tile: gtk::Label,
    name: gtk::Label,
    subtitle: gtk::Label,
    badges: gtk::Box,
    hint: Option<gtk::Label>,
}

impl Row {
    /// Finds the widgets built in `connect_setup`.
    fn find(list_item: &gtk::ListItem) -> Option<Row> {
        let row = list_item.child().and_downcast::<gtk::Box>()?;
        let tile = row.first_child().and_downcast::<gtk::Label>()?;
        let text = tile.next_sibling().and_downcast::<gtk::Box>()?;
        let name = text.first_child().and_downcast::<gtk::Label>()?;
        let subtitle = name.next_sibling().and_downcast::<gtk::Label>()?;
        let badges = text.next_sibling().and_downcast::<gtk::Box>()?;
        let hint = badges.next_sibling().and_downcast::<gtk::Label>();
        Some(Row { tile, name, subtitle, badges, hint })
    }
}

impl ItemList {
    /// `hints` shows the Enter action on the selected row, for quick access.
    pub fn new(hints: bool) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::SingleSelection::new(Some(store.clone()));
        selection.set_autoselect(true);
        selection.set_can_unselect(false);
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let tile = gtk::Label::new(None);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            let name = label("", &["cs-item-name"]);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let subtitle = label("", &["cs-item-subtitle"]);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&name);
            text.append(&subtitle);
            let badges = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            badges.set_valign(gtk::Align::Center);
            let hint = label("", &["cs-quick-action"]);
            hint.set_visible(false);
            row.append(&tile);
            row.append(&text);
            row.append(&badges);
            if hints {
                list_item.property_expression("selected").bind(&hint, "visible", gtk::Widget::NONE);
                row.append(&hint);
            }
            list_item.set_child(Some(&row));
        });
        factory.connect_bind(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else { return };
            let Some(boxed) = list_item.item().and_downcast::<glib::BoxedAnyObject>() else { return };
            let Some(row) = Row::find(list_item) else { return };
            let item = boxed.borrow::<Rc<ItemSummary>>();
            set_monogram(&row.tile, &item.name, false);
            row.name.set_label(&item.name);
            let subtitle = item.subtitle.clone().or_else(|| item.hosts.first().cloned()).unwrap_or_default();
            row.subtitle.set_label(&subtitle);
            row.subtitle.set_visible(!subtitle.is_empty());
            super::clear(&row.badges);
            let badge = |icon: &str, tip: &str, extra: Option<&str>| {
                let image = gtk::Image::from_icon_name(icon);
                image.add_css_class("cs-badge");
                if let Some(extra) = extra {
                    image.add_css_class(extra);
                }
                image.set_tooltip_text(Some(tip));
                image.set_pixel_size(14);
                image
            };
            if item.reprompt {
                row.badges.append(&badge("changes-prevent-symbolic", "Asks for the master password", None));
            }
            if item.has_passkey {
                row.badges.append(&badge("auth-fingerprint-symbolic", "Has a passkey", None));
            }
            if item.has_totp {
                row.badges.append(&badge("document-open-recent-symbolic", "Has a verification code", None));
            }
            if item.favorite {
                row.badges.append(&badge("starred-symbolic", "Favorite", Some("favorite")));
            }
            let action = match item.kind {
                ItemKind::Login if item.has_password => "↵ password",
                ItemKind::Login => "↵ username",
                ItemKind::Card => "↵ number",
                ItemKind::SecureNote => "↵ note",
                ItemKind::SshKey => "↵ public key",
                ItemKind::Identity => "↵ open",
            };
            if let Some(hint) = &row.hint {
                hint.set_label(action);
            }
            let tooltip = format!("{} · {}", item.name, item.kind.label());
            row.name.set_tooltip_text(Some(&tooltip));
        });
        let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        view.add_css_class("cs-items");
        view.set_single_click_activate(false);
        ItemList { view, store, selection }
    }

    /// Replaces the items, keeping the selected item selected if it remains.
    pub fn set_items(&self, items: &[Rc<ItemSummary>]) {
        let previous = self.selected().map(|i| i.id.clone());
        let objects: Vec<glib::BoxedAnyObject> =
            items.iter().map(|i| glib::BoxedAnyObject::new(i.clone())).collect();
        self.store.splice(0, self.store.n_items(), &objects);
        let index = previous.and_then(|id| items.iter().position(|i| i.id == id)).unwrap_or(0);
        if !items.is_empty() {
            self.selection.set_selected(index as u32);
        }
    }

    pub fn len(&self) -> u32 {
        self.store.n_items()
    }

    pub fn selected(&self) -> Option<Rc<ItemSummary>> {
        self.selection
            .selected_item()
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|b| b.borrow::<Rc<ItemSummary>>().clone())
    }

    pub fn select_by_id(&self, id: &str) {
        for i in 0..self.store.n_items() {
            if let Some(boxed) = self.store.item(i).and_downcast::<glib::BoxedAnyObject>()
                && boxed.borrow::<Rc<ItemSummary>>().id == id
            {
                self.selection.set_selected(i);
                self.view.scroll_to(i, gtk::ListScrollFlags::NONE, None);
                return;
            }
        }
    }

    /// Moves the selection by `delta`, staying in range.
    pub fn step(&self, delta: i32) {
        let n = self.store.n_items();
        if n == 0 {
            return;
        }
        let current = self.selection.selected();
        let current = if current == gtk::INVALID_LIST_POSITION { 0 } else { current };
        let next = (i64::from(current) + i64::from(delta)).clamp(0, i64::from(n) - 1) as u32;
        self.selection.set_selected(next);
        self.view.scroll_to(next, gtk::ListScrollFlags::NONE, None);
    }
}
