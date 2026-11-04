//! The main window: sidebar, item list and details, plus the setup,
//! sign-in and unlock pages.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use countersign_ipc::{
    CliState, ClipboardEvent, ClipboardState, Field, ItemKind, ItemSummary, Request, Status, VaultSnapshot,
    VaultState,
};
use gtk::{gdk, gio, glib};

use super::detail::{Detail, launchable};
use super::gates::{Setup, SignIn, Unlock, confirm_sign_out, server_name};
use super::items::ItemList;
use super::{kind_icon, kind_plural, label, relative_time};
use crate::app::{App, View};
use crate::search::Searcher;

thread_local! {
    static WINDOW: RefCell<Option<Rc<MainWindow>>> = const { RefCell::new(None) };
}

/// Shows the main window, creating it the first time.
pub fn present(app: &Rc<App>) {
    get_or_create(app).window.present();
}

/// The main window, created hidden if it does not exist yet.
pub fn get_or_create(app: &Rc<App>) -> Rc<MainWindow> {
    WINDOW.with(|w| w.borrow().clone()).unwrap_or_else(|| {
        let window = MainWindow::new(app);
        WINDOW.with(|w| *w.borrow_mut() = Some(window.clone()));
        window
    })
}

pub fn current() -> Option<Rc<MainWindow>> {
    WINDOW.with(|w| w.borrow().clone())
}

/// Shows a short message in the main window, if it exists.
pub fn toast(message: &str) {
    if let Some(window) = current() {
        window.toast(message);
    }
}

/// Shows a short message with a button, such as Undo, in the main window.
pub fn toast_with_action(message: &str, button: &str, run: impl Fn() + 'static) {
    if let Some(window) = current() {
        let toast = adw::Toast::builder().title(message).button_label(button).build();
        toast.connect_button_clicked(move |_| run());
        window.toasts.add_toast(toast);
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Filter {
    All,
    Favorites,
    Kind(ItemKind),
    Folder(Option<String>),
    Collection(String),
    Trash,
    /// Not a filter: the generator page.
    Generator,
    /// Not a filter: the Send page.
    Sends,
}

impl Filter {
    fn matches(&self, item: &ItemSummary) -> bool {
        match self {
            Filter::All | Filter::Trash | Filter::Generator | Filter::Sends => true,
            Filter::Favorites => item.favorite,
            Filter::Kind(kind) => item.kind == *kind,
            Filter::Folder(folder) => item.folder_id == *folder,
            Filter::Collection(id) => item.collection_ids.contains(id),
        }
    }
}

/// The field Enter copies for an item.
pub fn primary_field(item: &ItemSummary) -> Option<Field> {
    match item.kind {
        ItemKind::Login if item.has_password => Some(Field::Password),
        ItemKind::Login if item.username.is_some() => Some(Field::Username),
        ItemKind::Login => None,
        ItemKind::Card => Some(Field::CardNumber),
        ItemKind::SecureNote => Some(Field::Notes),
        ItemKind::SshKey => Some(Field::SshPublicKey),
        ItemKind::Identity => None,
    }
}

/// Shows what was copied and when it will be cleared.
pub struct Receipt {
    pub root: gtk::Revealer,
    title: gtk::Label,
    detail: gtk::Label,
    progress: gtk::ProgressBar,
    skip: gtk::Button,
    timer: RefCell<Option<glib::SourceId>>,
}

impl Receipt {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let title = label("", &["cs-receipt-title"]);
        let detail = label("", &["cs-receipt-detail"]);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.append(&title);
        text.append(&detail);
        let skip = gtk::Button::builder().label("Keep password").css_classes(["flat"]).visible(false).build();
        skip.set_valign(gtk::Align::Center);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::from_icon_name("edit-copy-symbolic");
        icon.add_css_class("cs-badge");
        row.append(&icon);
        row.append(&text);
        row.append(&skip);
        let progress = gtk::ProgressBar::new();
        progress.set_margin_top(6);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.add_css_class("cs-receipt");
        column.append(&row);
        column.append(&progress);
        let root = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .child(&column)
            .reveal_child(false)
            .build();
        let receipt = Rc::new(Receipt { root, title, detail, progress, skip, timer: RefCell::default() });
        {
            let (app, me) = (app.clone(), Rc::downgrade(&receipt));
            receipt.skip.connect_clicked(move |button| {
                app.fire(Request::SkipFollowUp);
                button.set_visible(false);
                if let Some(me) = me.upgrade() {
                    me.detail.set_label("Keeping the password");
                }
            });
        }
        receipt
    }

    fn stop(&self) {
        if let Some(source) = self.timer.borrow_mut().take() {
            source.remove();
        }
    }

    pub fn show(self: &Rc<Self>, event: &ClipboardEvent) {
        self.stop();
        self.root.set_reveal_child(true);
        self.title.set_label(&event.label);
        self.title.remove_css_class("cs-error");
        match event.state {
            ClipboardState::Copied | ClipboardState::FollowUpCopied => {
                let total = event.clears_in.unwrap_or(30).max(1);
                let item = event.item_name.clone();
                let follow = event.follow_up_in;
                self.skip.set_visible(follow.is_some());
                let start = std::time::Instant::now();
                let me = Rc::downgrade(self);
                let tick = move || {
                    let Some(me) = me.upgrade() else { return glib::ControlFlow::Break };
                    let elapsed = start.elapsed().as_secs_f64();
                    let left = (f64::from(total) - elapsed).max(0.0);
                    me.progress.set_fraction(left / f64::from(total));
                    let mut parts = Vec::new();
                    if let Some(item) = &item {
                        parts.push(item.clone());
                    }
                    if let Some(follow) = follow {
                        let code_in = (f64::from(follow) - elapsed).ceil();
                        if code_in > 0.0 && me.skip.is_visible() {
                            parts.push(format!("code replaces it in {code_in:.0}s"));
                        }
                    }
                    parts.push(format!("clears in {:.0}s", left.ceil()));
                    me.detail.set_label(&parts.join(" · "));
                    if left <= 0.0 {
                        me.timer.borrow_mut().take();
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                };
                tick();
                let source = glib::timeout_add_local(Duration::from_millis(250), tick);
                *self.timer.borrow_mut() = Some(source);
            }
            ClipboardState::Cleared => {
                self.skip.set_visible(false);
                self.progress.set_fraction(0.0);
                self.detail.set_label("Nothing of yours is left on the clipboard");
                self.hide_after(Duration::from_secs(2));
            }
            ClipboardState::Failed => {
                self.skip.set_visible(false);
                self.title.add_css_class("cs-error");
                self.detail.set_label(event.item_name.as_deref().unwrap_or(""));
                self.hide_after(Duration::from_secs(5));
            }
        }
    }

    fn hide_after(self: &Rc<Self>, delay: Duration) {
        let me = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(delay, move || {
            if let Some(me) = me.upgrade() {
                me.timer.borrow_mut().take();
                me.root.set_reveal_child(false);
            }
        });
        *self.timer.borrow_mut() = Some(source);
    }
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    app: Rc<App>,
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    loading_text: gtk::Label,
    unlock: Rc<Unlock>,
    sign_in: Rc<SignIn>,
    setup: Rc<Setup>,
    sidebar: gtk::ListBox,
    /// The generator and Send, pinned below the sidebar's filters.
    tools: gtk::ListBox,
    /// The main menu's folder actions, listed while a folder is shown.
    folder_menu_section: gio::Menu,
    filters: RefCell<Vec<Filter>>,
    filter: RefCell<Filter>,
    pub search: gtk::SearchEntry,
    pub list: ItemList,
    list_stack: gtk::Stack,
    list_meta: gtk::Label,
    empty: adw::StatusPage,
    search_all: gtk::Button,
    pub detail: Rc<Detail>,
    detail_page: adw::NavigationPage,
    pub nav: adw::NavigationSplitView,
    pub split: adw::OverlaySplitView,
    content: gtk::Stack,
    generator: Rc<super::generator::Generator>,
    generator_receipt: Rc<Receipt>,
    sends: Rc<super::sends::Sends>,
    sends_receipt: Rc<Receipt>,
    items: RefCell<Vec<Rc<ItemSummary>>>,
    /// An item to select once it appears, after creating it.
    pending_select: RefCell<Option<String>>,
    trash: RefCell<Vec<Rc<ItemSummary>>>,
    searcher: RefCell<Searcher>,
    account_email: gtk::Label,
    account_server: gtk::Label,
    dot: gtk::Box,
    sync_label: gtk::Label,
    receipt: Rc<Receipt>,
}

impl MainWindow {
    fn new(app: &Rc<App>) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .title("Countersign")
            .default_width(1180)
            .default_height(760)
            .width_request(360)
            .height_request(420)
            .build();

        // Sidebar.
        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("cs-nav");
        sidebar.set_selection_mode(gtk::SelectionMode::Single);
        let sidebar_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&sidebar)
            .build();
        let brand = label("COUNTERSIGN", &["cs-brand"]);
        let sidebar_header = adw::HeaderBar::builder()
            .title_widget(&brand)
            .show_end_title_buttons(false)
            .show_start_title_buttons(false)
            .build();
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("cs-dot");
        dot.set_valign(gtk::Align::Center);
        let account_email = label("", &["cs-account-email"]);
        account_email.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let account_server = label("", &["cs-account-server"]);
        let sync_label = label("", &["cs-account-server"]);
        let account_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        account_text.set_hexpand(true);
        account_text.append(&account_email);
        account_text.append(&account_server);
        account_text.append(&sync_label);
        let lock_button = super::icon_button("system-lock-screen-symbolic", "Lock (Ctrl+L)");
        lock_button.set_action_name(Some("win.lock"));
        let account = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        account.add_css_class("cs-account");
        account.append(&dot);
        account.append(&account_text);
        account.append(&lock_button);
        // The tools stay pinned above the account, however long the
        // folder list grows.
        let tools = gtk::ListBox::new();
        tools.add_css_class("cs-nav");
        tools.add_css_class("cs-tools");
        tools.set_selection_mode(gtk::SelectionMode::Single);
        for (_, icon, text) in TOOLS {
            tools.append(&nav_row(icon, text, None));
        }
        let sidebar_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar_column.append(&sidebar_scroll);
        sidebar_column.append(&tools);
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_header);
        sidebar_view.set_content(Some(&sidebar_column));
        sidebar_view.add_bottom_bar(&account);
        sidebar_view.add_css_class("cs-sidebar");

        // Item list.
        let search = gtk::SearchEntry::builder().placeholder_text("Search vault").hexpand(true).build();
        search.add_css_class("cs-search");
        search.update_property(&[gtk::accessible::Property::Label("Search vault")]);
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Show sidebar")
            .css_classes(["flat"])
            .visible(false)
            .build();
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Quick access"), Some("app.quick"));
        section.append(Some("Sync now"), Some("win.sync"));
        menu.append_section(None, &section);
        let section = gio::Menu::new();
        section.append(Some("Password generator"), Some("win.generator"));
        section.append(Some("Send"), Some("win.sends"));
        section.append(Some("Settings"), Some("app.settings"));
        section.append(Some("Lock"), Some("win.lock"));
        section.append(Some("Sign out…"), Some("win.sign-out"));
        menu.append_section(None, &section);
        let folder_menu_section = gio::Menu::new();
        menu.append_section(None, &folder_menu_section);
        let section = gio::Menu::new();
        section.append(Some("Keyboard shortcuts"), Some("app.shortcuts"));
        section.append(Some("About Countersign"), Some("app.about"));
        menu.append_section(None, &section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Menu")
            .primary(true)
            .build();
        let list_header =
            adw::HeaderBar::builder().title_widget(&search).show_end_title_buttons(false).build();
        list_header.pack_start(&sidebar_toggle);
        list_header.pack_end(&menu_button);
        let new_menu = gio::Menu::new();
        for (label, kind) in
            [("New login", "login"), ("New card", "card"), ("New identity", "identity"), ("New note", "note")]
        {
            new_menu.append(Some(label), Some(&format!("win.new-item::{kind}")));
        }
        new_menu.append(Some("New folder…"), Some("win.new-folder"));
        let new_button = gtk::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .menu_model(&new_menu)
            .tooltip_text("New item (Ctrl+N)")
            .build();
        list_header.pack_end(&new_button);
        let list = ItemList::new(false);
        let list_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list.view)
            .build();
        let list_meta = label("", &["cs-list-meta"]);
        let list_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list_column.append(&list_meta);
        list_column.append(&list_scroll);
        // Search covers the filter the sidebar shows; this widens it.
        let search_all = gtk::Button::builder()
            .label("Search all items")
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("edit-find-symbolic")
            .title("No matches")
            .child(&search_all)
            .build();
        empty.add_css_class("compact");
        let list_stack = gtk::Stack::new();
        list_stack.add_named(&list_column, Some("list"));
        list_stack.add_named(&empty, Some("empty"));
        let receipt = Receipt::new(app);
        let list_view = adw::ToolbarView::new();
        list_view.add_top_bar(&list_header);
        list_view.set_content(Some(&list_stack));
        list_view.add_bottom_bar(&receipt.root);
        let list_page = adw::NavigationPage::builder().title("Items").child(&list_view).build();

        // Details.
        let toasts = adw::ToastOverlay::new();
        let notify: Rc<dyn Fn(&str)> = {
            let toasts = toasts.clone();
            Rc::new(move |message: &str| toasts.add_toast(adw::Toast::new(message)))
        };
        let detail = Detail::new(app, notify.clone());
        let detail_view = adw::ToolbarView::new();
        detail_view.add_top_bar(&detail.header);
        detail_view.set_content(Some(&detail.root));
        let detail_page = adw::NavigationPage::builder().title("Details").child(&detail_view).build();

        let nav = adw::NavigationSplitView::builder()
            .sidebar(&list_page)
            .content(&detail_page)
            .min_sidebar_width(300.0)
            .max_sidebar_width(400.0)
            .sidebar_width_fraction(0.4)
            .build();
        let generator_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Show sidebar")
            .css_classes(["flat"])
            .build();
        // The generator page shows clipboard receipts too; the vault's sits
        // in the hidden list page while the generator is shown.
        let generator_receipt = Receipt::new(app);
        let generator = super::generator::Generator::new(
            app,
            notify.clone(),
            Some(generator_toggle.upcast_ref()),
            generator_receipt.root.upcast_ref(),
        );
        let sends_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Show sidebar")
            .css_classes(["flat"])
            .build();
        let sends_receipt = Receipt::new(app);
        let sends = super::sends::Sends::new(
            app,
            notify.clone(),
            Some(sends_toggle.upcast_ref()),
            sends_receipt.root.upcast_ref(),
        );
        let content = gtk::Stack::new();
        content.add_named(&nav, Some("vault"));
        content.add_named(&generator.root, Some("generator"));
        content.add_named(&sends.root, Some("sends"));
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar_view)
            .content(&content)
            .min_sidebar_width(210.0)
            .max_sidebar_width(250.0)
            .build();
        for toggle in [&sidebar_toggle, &generator_toggle, &sends_toggle] {
            toggle.bind_property("active", &split, "show-sidebar").bidirectional().build();
            split.bind_property("collapsed", toggle, "visible").sync_create().build();
        }

        let wide =
            adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 980sp").expect("condition"));
        wide.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(wide);
        let narrow =
            adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 620sp").expect("condition"));
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(&nav, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(narrow);

        // Pages.
        let unlock = Unlock::new(app, false);
        let sign_in = SignIn::new(app);
        let setup = Setup::new(app);
        let loading = gtk::Box::new(gtk::Orientation::Vertical, 12);
        loading.set_valign(gtk::Align::Center);
        loading.set_halign(gtk::Align::Center);
        loading.append(&super::gates::mark());
        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        loading.append(&spinner);
        let loading_text = label("", &["cs-gate-body"]);
        loading_text.set_halign(gtk::Align::Center);
        loading.append(&loading_text);
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        let gate = |child: &gtk::Widget| {
            let view = adw::ToolbarView::new();
            view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
            view.set_content(Some(child));
            view
        };
        stack.add_named(&gate(loading.upcast_ref()), Some("loading"));
        stack.add_named(&gate(&setup.root), Some("setup"));
        stack.add_named(&gate(&sign_in.root), Some("signin"));
        stack.add_named(&gate(&unlock.root), Some("locked"));
        stack.add_named(&split, Some("vault"));
        stack.add_css_class("cs-root");
        toasts.set_child(Some(&stack));
        window.set_content(Some(&toasts));

        let me = Rc::new(MainWindow {
            window,
            app: app.clone(),
            stack,
            toasts,
            loading_text,
            unlock,
            sign_in,
            setup,
            sidebar,
            tools,
            folder_menu_section,
            filters: RefCell::default(),
            filter: RefCell::new(Filter::All),
            search,
            list,
            list_stack,
            list_meta,
            empty,
            search_all,
            detail,
            detail_page,
            nav,
            split,
            content,
            generator,
            generator_receipt,
            sends,
            sends_receipt,
            items: RefCell::default(),
            pending_select: RefCell::default(),
            trash: RefCell::default(),
            searcher: RefCell::default(),
            account_email,
            account_server,
            dot,
            sync_label,
            receipt,
        });
        me.connect();
        app.add_view(Rc::downgrade(&(me.clone() as Rc<dyn View>)));
        me.status_changed(&app.status.borrow().clone());
        let vault = app.vault.borrow().clone();
        me.vault_changed(&vault);
        me
    }

    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let action = |name: &str, run: fn(&Rc<MainWindow>)| {
            let weak = weak.clone();
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| {
                if let Some(me) = weak.upgrade() {
                    run(&me);
                }
            });
            self.window.add_action(&action);
        };
        action("lock", |me| me.app.fire(Request::Lock));
        action("sync", |me| {
            let app = me.app.clone();
            let me = me.clone();
            glib::spawn_future_local(async move {
                match app.request(Request::Sync).await {
                    Ok(_) => me.toast("Vault synced"),
                    Err(e) => me.toast(&e.message),
                }
            });
        });
        action("sign-out", |me| confirm_sign_out(&me.app, me.window.upcast_ref(), None));
        {
            let weak = weak.clone();
            let new_item = gio::SimpleAction::new("new-item", Some(glib::VariantTy::STRING));
            new_item.connect_activate(move |_, parameter| {
                let Some(me) = weak.upgrade() else { return };
                let kind = match parameter.and_then(|p| p.str()) {
                    Some("card") => ItemKind::Card,
                    Some("identity") => ItemKind::Identity,
                    Some("note") => ItemKind::SecureNote,
                    _ => ItemKind::Login,
                };
                me.new_item(kind);
            });
            self.window.add_action(&new_item);
        }
        action("new-folder", |me| me.ask_folder_name(None));
        action("rename-folder", |me| {
            if let Some(folder) = me.shown_folder() {
                me.ask_folder_name(Some(folder));
            }
        });
        action("delete-folder", |me| {
            if let Some(folder) = me.shown_folder() {
                me.delete_folder(folder);
            }
        });
        action("edit", |me| {
            if me.content.visible_child_name().as_deref() == Some("vault")
                && me.window.visible_dialog().is_none()
                && !me.detail.is_editing()
                && me.detail.can_edit()
            {
                me.detail.edit();
                if me.nav.is_collapsed() {
                    me.nav.set_show_content(true);
                }
            }
        });
        action("generator", |me| me.show_vault_filter(Filter::Generator));
        action("sends", |me| me.show_vault_filter(Filter::Sends));
        action("focus-search", |me| {
            // Searching means the vault: leave the generator for it first.
            if me.content.visible_child_name().as_deref() != Some("vault") {
                me.show_vault_filter(Filter::All);
            }
            me.search.grab_focus();
            me.search.select_region(0, -1);
        });
        self.app.gtk.set_accels_for_action("win.lock", &["<Control>l"]);
        self.app.gtk.set_accels_for_action("win.sync", &["<Control>r"]);
        self.app.gtk.set_accels_for_action("win.focus-search", &["<Control>f"]);
        self.app.gtk.set_accels_for_action("win.generator", &["<Control>g"]);
        self.app.gtk.set_accels_for_action("win.sends", &["<Control><Shift>s"]);
        self.app.gtk.set_accels_for_action("win.new-item::login", &["<Control>n"]);
        self.app.gtk.set_accels_for_action("win.edit", &["<Control>e"]);
        {
            // Keep "Synced N minutes ago" current.
            let weak = weak.clone();
            glib::timeout_add_seconds_local(60, move || {
                let Some(me) = weak.upgrade() else { return glib::ControlFlow::Break };
                let status = me.app.status.borrow().clone();
                me.update_sync_label(&status);
                glib::ControlFlow::Continue
            });
        }

        {
            // Right-click a folder, or press the menu key on it, to rename or
            // delete it.
            let weak = weak.clone();
            let right = gtk::GestureClick::new();
            right.set_button(gdk::BUTTON_SECONDARY);
            right.connect_pressed(move |_, _, _, y| {
                let Some(me) = weak.upgrade() else { return };
                if let Some(row) = me.sidebar.row_at_y(y as i32) {
                    me.folder_menu(&row);
                }
            });
            self.sidebar.add_controller(right);
        }
        {
            let weak = weak.clone();
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(me) = weak.upgrade() else { return glib::Propagation::Proceed };
                let menu_key = key == gdk::Key::Menu
                    || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK));
                let focused = gtk::prelude::GtkWindowExt::focus(&me.window)
                    .and_then(|w| w.ancestor(gtk::ListBoxRow::static_type()))
                    .and_downcast::<gtk::ListBoxRow>();
                match focused.or_else(|| me.sidebar.selected_row()) {
                    Some(row) if menu_key => {
                        me.folder_menu(&row);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            self.sidebar.add_controller(keys);
        }
        {
            let weak = weak.clone();
            self.sidebar.connect_row_selected(move |_, row| {
                let (Some(me), Some(row)) = (weak.upgrade(), row) else { return };
                let filter = me.filters.borrow().get(row.index() as usize).cloned();
                if let Some(filter) = filter {
                    me.tools.unselect_all();
                    me.apply_filter(filter);
                }
            });
        }
        {
            let weak = weak.clone();
            self.tools.connect_row_selected(move |_, row| {
                let (Some(me), Some(row)) = (weak.upgrade(), row) else { return };
                if let Some((filter, _, _)) = TOOLS.get(row.index() as usize) {
                    me.sidebar.unselect_all();
                    me.apply_filter(filter.clone());
                }
            });
        }
        {
            let weak = weak.clone();
            self.search_all.connect_clicked(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.show_vault_filter(Filter::All);
                    me.search.grab_focus();
                    me.search.set_position(-1);
                }
            });
        }
        {
            let weak = weak.clone();
            self.search.connect_search_changed(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.refresh_list();
                    me.app.activity();
                }
            });
        }
        {
            let weak = weak.clone();
            self.search.connect_activate(move |_| {
                if let Some(me) = weak.upgrade() {
                    me.copy_primary();
                }
            });
        }
        {
            let weak = weak.clone();
            self.search.connect_stop_search(move |entry| {
                if entry.text().is_empty() {
                    if let Some(me) = weak.upgrade() {
                        me.list.view.grab_focus();
                    }
                } else {
                    entry.set_text("");
                }
            });
        }
        {
            let weak = weak.clone();
            self.list.selection.connect_selected_item_notify(move |_| {
                let Some(me) = weak.upgrade() else { return };
                if me.detail.is_editing() {
                    return;
                }
                let selected = me.list.selected();
                me.detail.show(selected.as_ref().map(|i| i.id.as_str()));
                if let Some(item) = selected {
                    me.detail_page.set_title(&item.name);
                }
            });
        }
        {
            let weak = weak.clone();
            self.list.view.connect_activate(move |_, _| {
                let Some(me) = weak.upgrade() else { return };
                if me.nav.is_collapsed() {
                    me.nav.set_show_content(true);
                } else {
                    me.copy_primary();
                }
            });
        }
        self.search.set_key_capture_widget(Some(&self.list.view));

        // Arrows in the search field move through the list; Ctrl shortcuts
        // copy fields of the selected item.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let weak = weak.clone();
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(me) = weak.upgrade() else { return glib::Propagation::Proceed };
                // Leave keys to an open dialog, such as the re-prompt.
                if me.stack.visible_child_name().as_deref() != Some("vault")
                    || me.content.visible_child_name().as_deref() != Some("vault")
                    || me.window.visible_dialog().is_some()
                {
                    return glib::Propagation::Proceed;
                }
                let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                let in_search = me.search.has_focus() || me.search.focus_child().is_some();
                // The editor takes only Esc and Ctrl+S: its item may not be
                // the list's selection. Esc in the search still clears it.
                if me.detail.is_editing() {
                    me.app.activity();
                    match key {
                        gdk::Key::Escape if !ctrl && !in_search && !focus_in_popover(&me.window) => {
                            me.detail.cancel_edit();
                            return glib::Propagation::Stop;
                        }
                        gdk::Key::s | gdk::Key::S if ctrl => {
                            me.detail.save_edit();
                            return glib::Propagation::Stop;
                        }
                        _ => return glib::Propagation::Proceed,
                    }
                }
                me.app.activity();
                match key {
                    gdk::Key::Down | gdk::Key::Up if in_search && !ctrl => {
                        me.list.step(if key == gdk::Key::Down { 1 } else { -1 });
                        glib::Propagation::Stop
                    }
                    gdk::Key::Page_Down | gdk::Key::Page_Up if in_search => {
                        me.list.step(if key == gdk::Key::Page_Down { 10 } else { -10 });
                        glib::Propagation::Stop
                    }
                    gdk::Key::slash if !in_search && !focus_in_editable(&me.window) => {
                        me.search.grab_focus();
                        glib::Propagation::Stop
                    }
                    gdk::Key::c | gdk::Key::C if ctrl && !has_text_selection(&me.window) => {
                        me.copy_selected(Field::Password)
                    }
                    gdk::Key::b | gdk::Key::B if ctrl => me.copy_selected(Field::Username),
                    gdk::Key::t | gdk::Key::T if ctrl => me.copy_selected(Field::Totp),
                    gdk::Key::u | gdk::Key::U if ctrl => {
                        me.open_selected();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        self.window.add_controller(keys);
        let clicks = gtk::GestureClick::new();
        clicks.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let weak = weak.clone();
            clicks.connect_pressed(move |_, _, _, _| {
                if let Some(me) = weak.upgrade() {
                    me.app.activity();
                }
            });
        }
        self.window.add_controller(clicks);

        {
            let app = self.app.clone();
            self.window.connect_is_active_notify(move |window| {
                if window.is_active() {
                    app.gtk.withdraw_notification(crate::app::CLIPBOARD_NOTIFICATION);
                }
            });
        }
        {
            let app = self.app.clone();
            self.window.connect_close_request(move |window| {
                window.set_visible(false);
                if !app.settings.borrow().close_to_background {
                    // Quit like Ctrl+Q does, which locks the vault first.
                    app.gtk.activate_action("quit", None);
                }
                glib::Propagation::Stop
            });
        }
    }

    /// Opens Rename and Delete for the folder on `row`, if it is one.
    fn folder_menu(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let filter = self.filters.borrow().get(row.index() as usize).cloned();
        let Some(Filter::Folder(Some(id))) = filter else { return };
        let Some(folder) = self.app.vault.borrow().folders.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let popover = gtk::Popover::new();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for (label, rename) in [("Rename…", true), ("Delete…", false)] {
            let button = gtk::Button::builder().label(label).css_classes(["flat"]).build();
            let (me, folder, popover) = (self.clone(), folder.clone(), popover.clone());
            button.connect_clicked(move |_| {
                popover.popdown();
                if rename {
                    me.ask_folder_name(Some(folder.clone()))
                } else {
                    me.delete_folder(folder.clone())
                }
            });
            column.append(&button);
        }
        popover.set_child(Some(&column));
        popover.set_parent(row);
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }

    /// The folder the sidebar shows, if it shows one.
    fn shown_folder(&self) -> Option<countersign_ipc::Folder> {
        let Filter::Folder(Some(id)) = &*self.filter.borrow() else { return None };
        self.app.vault.borrow().folders.iter().find(|f| f.id == *id).cloned()
    }

    /// Lists Rename and Delete in the main menu while a folder is shown.
    fn update_folder_menu(&self) {
        self.folder_menu_section.remove_all();
        if self.shown_folder().is_some() {
            self.folder_menu_section.append(Some("Rename folder…"), Some("win.rename-folder"));
            self.folder_menu_section.append(Some("Delete folder…"), Some("win.delete-folder"));
        }
    }

    /// Shows what a sidebar row stands for: a vault filter or a tool page.
    fn apply_filter(&self, filter: Filter) {
        // The selection, tool pages included, is what the sidebar restores
        // after a sync rebuilds it.
        let page = match filter {
            Filter::Generator => "generator",
            Filter::Sends => "sends",
            _ => "vault",
        };
        *self.filter.borrow_mut() = filter;
        self.update_folder_menu();
        self.content.set_visible_child_name(page);
        match page {
            "generator" => self.generator.ensure_value(),
            "sends" => {}
            _ => self.refresh_list(),
        }
        if self.split.is_collapsed() {
            self.split.set_show_sidebar(false);
        }
    }

    fn copy_selected(self: &Rc<Self>, field: Field) -> glib::Propagation {
        let Some(item) = self.list.selected() else { return glib::Propagation::Proceed };
        let field = match (&field, item.kind) {
            (Field::Password, ItemKind::Card) => Field::CardNumber,
            (Field::Username, ItemKind::Card) => Field::CardholderName,
            (Field::Totp, ItemKind::Card) => Field::CardCode,
            _ => field,
        };
        self.copy(&item.id, field);
        glib::Propagation::Stop
    }

    fn copy_primary(self: &Rc<Self>) {
        let Some(item) = self.list.selected() else { return };
        match primary_field(&item) {
            Some(field) => self.copy(&item.id, field),
            None => {
                if self.nav.is_collapsed() {
                    self.nav.set_show_content(true);
                }
            }
        }
    }

    fn copy(self: &Rc<Self>, id: &str, field: Field) {
        let (me, id) = (self.clone(), id.to_owned());
        glib::spawn_future_local(async move {
            if let Err(e) = me.app.copy(me.window.upcast_ref(), &id, field).await {
                me.toast(&e.message);
            }
        });
    }

    fn open_selected(&self) {
        let Some(target) =
            self.list.selected().and_then(|i| i.first_uri.clone()).and_then(|u| launchable(&u))
        else {
            return;
        };
        gtk::UriLauncher::new(&target).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
    }

    fn rebuild_sidebar(&self, vault: &VaultSnapshot) {
        let selected = self.filter.borrow().clone();
        while let Some(row) = self.sidebar.row_at_index(0) {
            self.sidebar.remove(&row);
        }
        let mut filters = Vec::new();
        let items = &vault.items;
        let count = |f: &Filter| items.iter().filter(|i| f.matches(i)).count();
        let add_row =
            |filters: &mut Vec<Filter>, filter: Filter, icon: &str, text: &str, count: Option<usize>| {
                self.sidebar.append(&nav_row(icon, text, count));
                filters.push(filter);
            };
        let add_title = |text: &str| {
            let title = label(text, &["cs-section-title"]);
            let row = gtk::ListBoxRow::builder().child(&title).selectable(false).activatable(false).build();
            self.sidebar.append(&row);
        };
        add_row(&mut filters, Filter::All, "view-grid-symbolic", "All items", Some(items.len()));
        add_row(
            &mut filters,
            Filter::Favorites,
            "starred-symbolic",
            "Favorites",
            Some(count(&Filter::Favorites)),
        );
        for kind in
            [ItemKind::Login, ItemKind::Card, ItemKind::Identity, ItemKind::SecureNote, ItemKind::SshKey]
        {
            let n = count(&Filter::Kind(kind));
            if n > 0 || kind == ItemKind::Login {
                add_row(&mut filters, Filter::Kind(kind), kind_icon(kind), kind_plural(kind), Some(n));
            }
        }
        // Titles occupy rows too; keep the filter list aligned with indices.
        let push_title = |filters: &mut Vec<Filter>, text: &str| {
            add_title(text);
            filters.push(Filter::All);
        };
        if !vault.folders.is_empty() {
            push_title(&mut filters, "FOLDERS");
            for folder in &vault.folders {
                let f = Filter::Folder(Some(folder.id.clone()));
                let n = count(&f);
                add_row(&mut filters, f, "folder-symbolic", &folder.name, Some(n));
            }
            let none = Filter::Folder(None);
            let n = count(&none);
            if n > 0 {
                add_row(&mut filters, none, "folder-symbolic", "No folder", Some(n));
            }
        }
        // Collections, under the organization that owns them.
        for organization in &vault.organizations {
            let owned: Vec<_> =
                vault.collections.iter().filter(|c| c.organization_id == organization.id).collect();
            if owned.is_empty() {
                continue;
            }
            push_title(&mut filters, &organization.name.to_uppercase());
            for collection in owned {
                let f = Filter::Collection(collection.id.clone());
                let n = count(&f);
                add_row(&mut filters, f, "user-bookmarks-symbolic", &collection.name, Some(n));
            }
        }
        let gap = gtk::ListBoxRow::builder()
            .child(&gtk::Separator::new(gtk::Orientation::Horizontal))
            .selectable(false)
            .activatable(false)
            .css_classes(["cs-nav-gap"])
            .build();
        self.sidebar.append(&gap);
        filters.push(Filter::All);
        add_row(&mut filters, Filter::Trash, "user-trash-symbolic", "Trash", Some(vault.trash.len()));
        *self.filters.borrow_mut() = filters;
        self.show_vault_filter(selected);
    }

    fn refresh_list(&self) {
        let filter = self.filter.borrow().clone();
        let query = self.search.text();
        let recent = self.app.recent.borrow().clone();
        let (shown, total) = {
            let source = if filter == Filter::Trash { self.trash.borrow() } else { self.items.borrow() };
            let candidates: Vec<&Rc<ItemSummary>> = source.iter().filter(|i| filter.matches(i)).collect();
            let refs: Vec<&ItemSummary> = candidates.iter().map(|r| r.as_ref()).collect();
            let order =
                self.searcher.borrow_mut().rank(&refs, &query, if query.is_empty() { &[] } else { &recent });
            let shown: Vec<Rc<ItemSummary>> = order.iter().map(|&i| candidates[i].clone()).collect();
            (shown, candidates.len())
        };
        self.list.set_items(&shown);
        self.list_meta.set_label(&if query.is_empty() {
            format!("{total} {}", if total == 1 { "item" } else { "items" })
        } else {
            format!("{} of {total} match “{}”", shown.len(), query.trim())
        });
        if shown.is_empty() {
            let narrowed = !query.is_empty() && filter != Filter::All;
            self.empty.set_title(if query.is_empty() { "Nothing here yet" } else { "No matches" });
            self.empty.set_description(Some(if query.is_empty() {
                "Items you add in this category appear here."
            } else if narrowed {
                "Try fewer letters, or search all items."
            } else {
                "Try fewer letters."
            }));
            self.search_all.set_visible(narrowed);
            self.list_stack.set_visible_child_name("empty");
        } else {
            self.list_stack.set_visible_child_name("list");
        }
        // An edit in progress keeps the pane until it is saved or
        // cancelled; locking closes it (see `status_changed`).
        if self.detail.is_editing() {
            return;
        }
        match self.list.selected().filter(|_| !shown.is_empty()) {
            Some(item) => {
                self.detail.show(Some(&item.id));
                self.detail_page.set_title(&item.name);
            }
            None => self.detail.show(None),
        }
    }
}

impl MainWindow {
    /// Selects a vault filter in the sidebar, which also shows the vault.
    /// Opens the editor for a new item, in the folder being browsed.
    fn new_item(self: &Rc<Self>, kind: ItemKind) {
        if self.content.visible_child_name().as_deref() != Some("vault") {
            self.show_vault_filter(Filter::All);
        }
        let folder = match &*self.filter.borrow() {
            Filter::Folder(folder) => folder.clone(),
            _ => None,
        };
        self.detail.create(kind, folder);
        if self.nav.is_collapsed() {
            self.nav.set_show_content(true);
        }
    }

    /// Selects `id` now if the vault lists it, or as soon as it does.
    pub fn select_when_listed(&self, id: &str) {
        if self.items.borrow().iter().any(|i| i.id == id) {
            self.list.select_by_id(id);
        } else {
            *self.pending_select.borrow_mut() = Some(id.to_owned());
        }
    }

    /// While the editor is open the list stays put: a sync that rebuilds
    /// it must not move the selection and close the editor.
    pub fn set_editing(&self, editing: bool) {
        self.list.view.set_sensitive(!editing);
    }

    /// Asks for a folder name: a new folder, or a rename of `folder`.
    fn ask_folder_name(self: &Rc<Self>, folder: Option<countersign_ipc::Folder>) {
        let name = adw::EntryRow::builder().title("Folder name").build();
        if let Some(f) = &folder {
            name.set_text(&f.name);
        }
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.append(&name);
        let renaming = folder.is_some();
        let dialog = adw::AlertDialog::builder()
            .heading(if renaming { "Rename folder" } else { "New folder" })
            .extra_child(&list)
            .close_response("cancel")
            .default_response("save")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("save", if renaming { "Rename" } else { "Create" })]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("save", !name.text().trim().is_empty());
        {
            let dialog = dialog.clone();
            name.connect_changed(move |row| {
                dialog.set_response_enabled("save", !row.text().trim().is_empty())
            });
        }
        name.set_activates_default(true);
        let field = name.clone();
        let me = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "save" {
                return;
            }
            let text = name.text().trim().to_owned();
            if text.is_empty() {
                return;
            }
            let request = match &folder {
                Some(f) => Request::RenameFolder { id: f.id.clone(), name: text },
                None => Request::CreateFolder { name: text },
            };
            let me = me.clone();
            glib::spawn_future_local(async move {
                if let Err(e) = me.app.request(request).await {
                    me.toast(&e.message);
                }
            });
        });
        dialog.present(Some(&self.window));
        field.grab_focus();
    }

    fn delete_folder(self: &Rc<Self>, folder: countersign_ipc::Folder) {
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Delete “{}”?", folder.name))
            .body("Its items are kept and move to No folder.")
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
            let (me, id) = (me.clone(), folder.id.clone());
            glib::spawn_future_local(async move {
                match me.app.request(Request::DeleteFolder { id }).await {
                    Ok(_) => me.show_vault_filter(Filter::All),
                    Err(e) => me.toast(&e.message),
                }
            });
        });
        dialog.present(Some(&self.window));
    }

    /// Selects the sidebar row for `filter`, which shows it. A filter no
    /// longer listed, such as a deleted folder, falls back to all items.
    fn show_vault_filter(&self, filter: Filter) {
        if let Some(index) = TOOLS.iter().position(|(f, _, _)| *f == filter) {
            if let Some(row) = self.tools.row_at_index(index as i32) {
                self.tools.select_row(Some(&row));
            }
            return;
        }
        let index = self.filters.borrow().iter().position(|f| *f == filter).unwrap_or(0);
        if let Some(row) = self.sidebar.row_at_index(index as i32) {
            self.sidebar.select_row(Some(&row));
        }
    }

    /// Updates only the sync time, without touching any other page.
    fn update_sync_label(&self, status: &Status) {
        self.sync_label.set_label(&match (status.syncing, &status.last_sync) {
            (true, _) => "Syncing…".to_owned(),
            (false, Some(when)) => format!("Synced {}", relative_time(when)),
            (false, None) => String::new(),
        });
    }
}

/// The tool pages pinned below the sidebar's filters.
const TOOLS: [(Filter, &str, &str); 2] = [
    (Filter::Generator, "dialog-password-symbolic", "Generator"),
    (Filter::Sends, "send-to-symbolic", "Send"),
];

/// A sidebar row: icon, name and an optional count.
fn nav_row(icon: &str, text: &str, count: Option<usize>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("cs-nav-icon");
    row.append(&image);
    let name = label(text, &[]);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&name);
    if let Some(count) = count {
        row.append(&label(&count.to_string(), &["cs-count"]));
    }
    row
}

/// The window actions that act on the open vault.
const VAULT_ACTIONS: [&str; 11] = [
    "lock",
    "sync",
    "sign-out",
    "new-item",
    "new-folder",
    "rename-folder",
    "delete-folder",
    "edit",
    "generator",
    "sends",
    "focus-search",
];

/// The page the main window shows for `status`, given the page it shows
/// now. While the vault opens, an unlock form on screen stays, busy; any
/// other page gives way to "Opening your vault…". A sign-out shows
/// "Signing out…" until the CLI answers.
fn page_for(status: &Status, opening: bool, signing_out: bool, showing: Option<&str>) -> &'static str {
    match (&status.cli, status.vault) {
        (CliState::Checking, _) => "loading",
        (CliState::Ready { .. }, _) if signing_out => "loading",
        (CliState::Ready { .. }, _) if opening => {
            if showing == Some("locked") {
                "locked"
            } else {
                "loading"
            }
        }
        (CliState::Ready { .. }, VaultState::Unlocked) => "vault",
        (CliState::Ready { .. }, VaultState::Locked) => "locked",
        (CliState::Ready { .. }, VaultState::LoggedOut) => "signin",
        (CliState::Ready { .. }, VaultState::Unlocking | VaultState::Unknown) => "loading",
        _ => "setup",
    }
}

fn focus_in_editable(window: &adw::ApplicationWindow) -> bool {
    gtk::prelude::GtkWindowExt::focus(window).is_some_and(|w| w.is::<gtk::Text>() || w.is::<gtk::Entry>())
}

/// Whether a popover, such as a drop-down's list, has the focus; Esc
/// closes it rather than the editor.
fn focus_in_popover(window: &adw::ApplicationWindow) -> bool {
    gtk::prelude::GtkWindowExt::focus(window)
        .is_some_and(|w| w.ancestor(gtk::Popover::static_type()).is_some())
}

fn has_text_selection(window: &adw::ApplicationWindow) -> bool {
    let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else { return false };
    if let Some(editable) = focus.dynamic_cast_ref::<gtk::Editable>() {
        return editable.selection_bounds().is_some();
    }
    if let Some(label) = focus.dynamic_cast_ref::<gtk::Label>() {
        return label.selection_bounds().is_some();
    }
    false
}

impl View for MainWindow {
    fn status_changed(&self, status: &Status) {
        let (opening, signing_out) = (self.app.opening(), self.app.signing_out());
        let previous = self.stack.visible_child_name();
        let page = page_for(status, opening, signing_out, previous.as_deref());
        self.stack.set_visible_child_name(page);
        self.loading_text.set_label(if signing_out {
            "Signing out…"
        } else if opening {
            "Opening your vault…"
        } else {
            ""
        });
        // The vault's shortcuts and menus act only on the vault page.
        for name in VAULT_ACTIONS {
            if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(page == "vault");
            }
        }
        self.unlock.set_status(status, opening);
        self.sign_in.set_status(status);
        self.setup.set_status(status);
        if page == "locked" && previous.as_deref() != Some("locked") {
            self.unlock.focus();
        }
        if page == "vault" && previous.as_deref() != Some("vault") {
            // Focus what is shown: the search, unless a tool page is.
            match self.content.visible_child_name().as_deref() {
                Some("generator") => self.generator.focus(),
                Some("sends") => self.sends.focus(),
                _ => {
                    self.search.grab_focus();
                }
            }
        }
        let account = status.account.as_ref();
        self.account_email.set_label(account.and_then(|a| a.email.as_deref()).unwrap_or("Not signed in"));
        self.account_server.set_label(&server_name(account.and_then(|a| a.server_url.as_deref())));
        if status.vault != VaultState::Unlocked {
            self.generator.clear();
            self.sends.clear();
            // Drop an open editor and the secrets in its fields.
            if self.detail.is_editing() {
                self.detail.reload(None);
            }
        }
        self.update_sync_label(status);
        self.dot.set_css_classes(&match (status.vault, status.syncing) {
            (_, true) => vec!["cs-dot", "syncing"],
            (VaultState::Unlocked, _) => vec!["cs-dot"],
            _ => vec!["cs-dot", "locked"],
        });
    }

    fn vault_changed(&self, vault: &Rc<VaultSnapshot>) {
        *self.items.borrow_mut() = vault.items.iter().cloned().map(Rc::new).collect();
        *self.trash.borrow_mut() = vault.trash.iter().cloned().map(Rc::new).collect();
        self.detail.set_vault(vault);
        self.rebuild_sidebar(vault);
        self.refresh_list();
        let pending = self.pending_select.borrow_mut().take();
        if let Some(id) = pending {
            if vault.items.iter().any(|i| i.id == id) {
                self.search.set_text("");
                self.list.select_by_id(&id);
            } else {
                *self.pending_select.borrow_mut() = Some(id);
            }
        }
        self.sends.vault_changed();
        // A sync must not throw away an edit in progress.
        if !self.detail.is_editing()
            && let Some(id) = self.detail.current()
        {
            self.detail.reload(Some(&id));
        }
    }

    fn clipboard(&self, event: &ClipboardEvent) {
        self.receipt.show(event);
        self.generator_receipt.show(event);
        self.sends_receipt.show(event);
    }
}

#[cfg(test)]
mod tests {
    use countersign_ipc::{CliState, Status, VaultState};

    use super::page_for;

    fn status(vault: VaultState) -> Status {
        Status {
            cli: CliState::Ready { version: "2026.10.1".into(), managed: true },
            vault,
            account: None,
            server_url: None,
            last_sync: None,
            syncing: false,
        }
    }

    #[test]
    fn an_unlock_keeps_its_form_and_a_sign_in_shows_the_vault_opening() {
        use VaultState::*;
        // Unlocking from the form: the form stays until the vault can show.
        for vault in [Locked, Unlocking, Unlocked] {
            assert_eq!(page_for(&status(vault), true, false, Some("locked")), "locked");
        }
        // Signing in: the sign-in page gives way, and nothing comes between.
        assert_eq!(page_for(&status(LoggedOut), true, false, Some("signin")), "loading");
        for vault in [Locked, Unlocking, Unlocked] {
            assert_eq!(page_for(&status(vault), true, false, Some("loading")), "loading");
        }
        assert_eq!(page_for(&status(Unlocked), false, false, Some("loading")), "vault");
        assert_eq!(page_for(&status(Unlocked), false, false, Some("locked")), "vault");
        // A failed open lands on the form.
        assert_eq!(page_for(&status(Locked), false, false, Some("loading")), "locked");
        assert_eq!(page_for(&status(Locked), false, false, Some("locked")), "locked");
        assert_eq!(page_for(&status(LoggedOut), false, false, Some("loading")), "signin");
    }

    #[test]
    fn a_sign_out_shows_its_wait_instead_of_the_unlock_form() {
        use VaultState::*;
        // The daemon reports the vault locked before the CLI has signed out.
        for (vault, showing) in
            [(Unlocked, "vault"), (Locked, "vault"), (Locked, "locked"), (LoggedOut, "loading")]
        {
            assert_eq!(page_for(&status(vault), false, true, Some(showing)), "loading");
        }
        // Once it answers: signed out, or still locked if it failed.
        assert_eq!(page_for(&status(LoggedOut), false, false, Some("loading")), "signin");
        assert_eq!(page_for(&status(Locked), false, false, Some("loading")), "locked");
    }

    #[test]
    fn the_cli_comes_first() {
        let mut checking = status(VaultState::Unlocking);
        checking.cli = CliState::Checking;
        assert_eq!(page_for(&checking, true, false, Some("locked")), "loading");
        let mut missing = status(VaultState::Unlocking);
        missing.cli = CliState::Missing;
        assert_eq!(page_for(&missing, true, false, Some("locked")), "setup");
    }
}
