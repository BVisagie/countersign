//! Quick access: a launcher-style overlay for finding an item and copying
//! from it without leaving what you were doing.
//!
//! Bind it in Hyprland, for example
//! `bindd = SUPER SHIFT, P, Passwords, exec, countersign --quick`.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use countersign_ipc::{
    CliState, ClipboardEvent, Field, ItemKind, ItemSummary, Status, VaultSnapshot, VaultState,
};
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::detail::launchable;
use super::gates::Unlock;
use super::items::ItemList;
use super::window::primary_field;
use super::{keycap, label};
use crate::app::{App, View};
use crate::search::Searcher;

const WIDTH: i32 = 680;
const HEIGHT: i32 = 500;

thread_local! {
    static QUICK: RefCell<Option<Rc<Quick>>> = const { RefCell::new(None) };
}

pub fn toggle(app: &Rc<App>) {
    let quick = get_or_create(app);
    if quick.window.is_visible() { quick.hide() } else { quick.show() }
}

pub fn get_or_create(app: &Rc<App>) -> Rc<Quick> {
    QUICK.with(|q| q.borrow().clone()).unwrap_or_else(|| {
        let quick = Quick::new(app);
        QUICK.with(|q| *q.borrow_mut() = Some(quick.clone()));
        quick
    })
}

pub struct Quick {
    pub window: adw::Window,
    pub panel: gtk::Box,
    app: Rc<App>,
    stack: gtk::Stack,
    pub search: gtk::Text,
    pub list: ItemList,
    unlock: Rc<Unlock>,
    other_text: gtk::Label,
    count: gtk::Label,
    items: RefCell<Vec<Rc<ItemSummary>>>,
    searcher: RefCell<Searcher>,
}

impl Quick {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        // An Adwaita window, so dialogs such as the re-prompt open inside
        // the overlay instead of as a separate window it holds focus from.
        let window = adw::Window::builder()
            .application(&app.gtk)
            .title("Countersign")
            .default_width(WIDTH)
            .default_height(HEIGHT)
            .resizable(false)
            .decorated(false)
            .build();
        window.add_css_class("cs-quick-window");
        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("countersign"));
            window.set_layer(Layer::Overlay);
            window.set_keyboard_mode(KeyboardMode::Exclusive);
            window.set_anchor(Edge::Top, true);
            window.set_margin(Edge::Top, 140);
            window.set_exclusive_zone(-1);
        }

        // Search row.
        let icon = gtk::Image::from_icon_name("system-search-symbolic");
        icon.add_css_class("cs-badge");
        let search =
            gtk::Text::builder().placeholder_text("Search logins, cards, notes…").hexpand(true).build();
        search.update_property(&[gtk::accessible::Property::Label("Search vault")]);
        let count = label("", &["cs-quick-hint"]);
        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        search_row.add_css_class("cs-quick-search");
        search_row.append(&icon);
        search_row.append(&search);
        search_row.append(&count);

        let list = ItemList::new(true);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list.view)
            .build();
        scroll.set_margin_top(4);
        scroll.set_margin_bottom(4);
        let empty = adw::StatusPage::builder().icon_name("edit-find-symbolic").title("No matches").build();
        empty.add_css_class("compact");
        let results = gtk::Stack::new();
        results.add_named(&scroll, Some("list"));
        results.add_named(&empty, Some("empty"));

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.add_css_class("cs-quick-footer");
        for (key, text) in
            [("↵", "copy"), ("⇧↵", "username"), ("Ctrl T", "code"), ("Ctrl U", "open"), ("Ctrl O", "details")]
        {
            footer.append(&keycap(key));
            let hint = label(text, &["cs-quick-hint"]);
            hint.set_margin_end(8);
            footer.append(&hint);
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        footer.append(&keycap("Esc"));

        let vault_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vault_page.append(&search_row);
        vault_page.append(&results);
        vault_page.append(&footer);

        let unlock = Unlock::new(app, true);
        let other_text = label("", &["cs-gate-body"]);
        other_text.set_wrap(true);
        other_text.set_justify(gtk::Justification::Center);
        let open = gtk::Button::builder()
            .label("Open Countersign")
            .css_classes(["pill", "suggested-action"])
            .build();
        open.set_halign(gtk::Align::Center);
        let other = gtk::Box::new(gtk::Orientation::Vertical, 14);
        other.set_valign(gtk::Align::Center);
        other.set_margin_start(32);
        other.set_margin_end(32);
        other.append(&other_text);
        other.append(&open);

        let stack =
            gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).vexpand(true).build();
        stack.add_named(&vault_page, Some("vault"));
        stack.add_named(&unlock.root, Some("locked"));
        stack.add_named(&other, Some("other"));

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("cs-quick");
        panel.set_size_request(WIDTH, HEIGHT);
        panel.set_overflow(gtk::Overflow::Hidden);
        panel.append(&stack);
        // Room for the shadow.
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.set_margin_start(24);
        outer.set_margin_end(24);
        outer.set_margin_bottom(36);
        outer.append(&panel);
        window.set_content(Some(&outer));

        let quick = Rc::new(Quick {
            window,
            panel,
            app: app.clone(),
            stack,
            search,
            list,
            unlock,
            other_text,
            count,
            items: RefCell::default(),
            searcher: RefCell::default(),
        });
        {
            let results = results.clone();
            let list = quick.list.clone();
            quick.list.selection.connect_items_changed(move |_, _, _, _| {
                results.set_visible_child_name(if list.len() == 0 { "empty" } else { "list" });
            });
        }
        quick.connect(&open);
        app.add_view(Rc::downgrade(&(quick.clone() as Rc<dyn View>)));
        quick.status_changed(&app.status.borrow().clone());
        let vault = app.vault.borrow().clone();
        quick.vault_changed(&vault);
        quick
    }

    fn connect(self: &Rc<Self>, open: &gtk::Button) {
        let weak = Rc::downgrade(self);
        {
            let weak = weak.clone();
            self.search.connect_changed(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.refresh();
                    me.app.activity();
                }
            });
        }
        {
            let weak = weak.clone();
            self.list.view.connect_activate(move |_, _| {
                if let Some(me) = weak.upgrade() {
                    me.copy_primary(false);
                }
            });
        }
        {
            let weak = weak.clone();
            open.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.hide();
                    me.app.show_main();
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(me) = weak.upgrade() else { return glib::Propagation::Proceed };
            if me.window.visible_dialog().is_some() {
                return glib::Propagation::Proceed;
            }
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            if key == gdk::Key::Escape {
                if me.stack.visible_child_name().as_deref() == Some("vault") && !me.search.text().is_empty() {
                    me.search.set_text("");
                } else {
                    me.hide();
                }
                return glib::Propagation::Stop;
            }
            if me.stack.visible_child_name().as_deref() != Some("vault") {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::Down | gdk::Key::Up => me.list.step(if key == gdk::Key::Down { 1 } else { -1 }),
                gdk::Key::Page_Down | gdk::Key::Page_Up => {
                    me.list.step(if key == gdk::Key::Page_Down { 8 } else { -8 })
                }
                gdk::Key::n | gdk::Key::j if ctrl => me.list.step(1),
                gdk::Key::p | gdk::Key::k if ctrl => me.list.step(-1),
                gdk::Key::Return | gdk::Key::KP_Enter if ctrl => me.open_details(),
                gdk::Key::Return | gdk::Key::KP_Enter => me.copy_primary(shift),
                gdk::Key::c | gdk::Key::C if ctrl && me.search.selection_bounds().is_none() => {
                    me.copy_field(Field::Password)
                }
                gdk::Key::b | gdk::Key::B if ctrl => me.copy_field(Field::Username),
                gdk::Key::t | gdk::Key::T if ctrl => me.copy_field(Field::Totp),
                gdk::Key::u | gdk::Key::U if ctrl => me.open_website(),
                gdk::Key::o | gdk::Key::O if ctrl => me.open_details(),
                gdk::Key::l | gdk::Key::L if ctrl => me.app.fire(countersign_ipc::Request::Lock),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
        let clicks = gtk::GestureClick::new();
        clicks.set_propagation_phase(gtk::PropagationPhase::Capture);
        let app = self.app.clone();
        clicks.connect_pressed(move |_, _, _, _| app.activity());
        self.window.add_controller(clicks);
    }

    pub fn show(self: &Rc<Self>) {
        let start = std::time::Instant::now();
        let frame_clock = self.window.frame_clock();
        if let Some(clock) = frame_clock {
            let handler: std::rc::Rc<std::cell::Cell<Option<glib::SignalHandlerId>>> = Default::default();
            let slot = handler.clone();
            let id = clock.connect_after_paint(move |clock| {
                crate::app::timing(&format!("quick access painted in {:?}", start.elapsed()));
                if let Some(id) = slot.take() {
                    clock.disconnect(id);
                }
            });
            handler.set(Some(id));
        }
        {
            let mapped = self.window.connect_map(move |_| {
                crate::app::timing(&format!("quick access mapped in {:?}", start.elapsed()));
            });
            let window = self.window.clone();
            glib::idle_add_local_once(move || window.disconnect(mapped));
        }
        self.search.set_text("");
        self.refresh();
        self.list.step(-(self.list.len() as i32));
        self.window.present();
        self.focus();
        self.app.activity();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
        self.search.set_text("");
    }

    fn focus(&self) {
        match self.stack.visible_child_name().as_deref() {
            Some("vault") => {
                self.search.grab_focus();
            }
            Some("locked") => self.unlock.focus(),
            _ => {}
        }
    }

    fn refresh(&self) {
        let items = self.items.borrow();
        let refs: Vec<&ItemSummary> = items.iter().map(|r| r.as_ref()).collect();
        let query = self.search.text();
        let recent = self.app.recent.borrow().clone();
        let order = self.searcher.borrow_mut().rank(&refs, &query, &recent);
        let shown: Vec<Rc<ItemSummary>> = order.iter().take(200).map(|&i| items[i].clone()).collect();
        drop(items);
        self.count.set_label(&if query.trim().is_empty() {
            String::new()
        } else {
            format!("{} {}", order.len(), if order.len() == 1 { "match" } else { "matches" })
        });
        self.list.set_items(&shown);
    }

    fn copy_primary(self: &Rc<Self>, alternate: bool) {
        let Some(item) = self.list.selected() else { return };
        let field = if alternate {
            match item.kind {
                ItemKind::Login => Some(Field::Username),
                ItemKind::Card => Some(Field::CardCode),
                _ => primary_field(&item),
            }
        } else {
            primary_field(&item)
        };
        match field {
            Some(field) => self.copy(&item, field),
            None => self.open_details(),
        }
    }

    fn copy_field(self: &Rc<Self>, field: Field) {
        let Some(item) = self.list.selected() else { return };
        let field = match (&field, item.kind) {
            (Field::Password, ItemKind::Card) => Field::CardNumber,
            (Field::Username, ItemKind::Card) => Field::CardholderName,
            (Field::Totp, ItemKind::Card) => Field::CardCode,
            _ => field,
        };
        self.copy(&item, field);
    }

    fn copy(self: &Rc<Self>, item: &ItemSummary, field: Field) {
        let (me, id) = (self.clone(), item.id.clone());
        glib::spawn_future_local(async move {
            match me.app.copy(me.panel.upcast_ref(), &id, field).await {
                Ok(_) => me.hide(),
                Err(e) => me.count.set_label(&e.message),
            }
        });
    }

    fn open_website(&self) {
        if let Some(target) =
            self.list.selected().and_then(|i| i.first_uri.clone()).and_then(|u| launchable(&u))
        {
            self.hide();
            gtk::UriLauncher::new(&target).launch(None::<&gtk::Window>, gtk::gio::Cancellable::NONE, |_| {});
        }
    }

    fn open_details(&self) {
        let selected = self.list.selected();
        self.hide();
        self.app.show_main();
        if let (Some(item), Some(window)) = (selected, super::window::current()) {
            window.search.set_text("");
            window.list.select_by_id(&item.id);
        }
    }
}

impl View for Quick {
    fn status_changed(&self, status: &Status) {
        // As in the main window: an unlock form on screen stays, busy,
        // while the vault opens, and a sign-out shows its wait.
        let (opening, signing_out) = (self.app.opening(), self.app.signing_out());
        let showing = self.stack.visible_child_name();
        let page = match (&status.cli, status.vault) {
            (CliState::Ready { .. }, _) if signing_out => "other",
            (CliState::Ready { .. }, _) if opening => {
                if showing.as_deref() == Some("locked") {
                    "locked"
                } else {
                    "other"
                }
            }
            (CliState::Ready { .. }, VaultState::Unlocked) => "vault",
            (CliState::Ready { .. }, VaultState::Locked) => "locked",
            _ => "other",
        };
        self.other_text.set_label(match status.vault {
            _ if signing_out => "Signing out…",
            _ if opening => "Opening your vault…",
            VaultState::LoggedOut => "Sign in to Bitwarden in Countersign first.",
            _ => "Countersign is getting ready.",
        });
        let changed = showing.as_deref() != Some(page);
        self.stack.set_visible_child_name(page);
        self.unlock.set_status(status, opening);
        if changed && self.window.is_visible() {
            self.focus();
        }
    }

    fn vault_changed(&self, vault: &Rc<VaultSnapshot>) {
        *self.items.borrow_mut() = vault.items.iter().cloned().map(Rc::new).collect();
        self.refresh();
    }

    fn clipboard(&self, _event: &ClipboardEvent) {}
}
