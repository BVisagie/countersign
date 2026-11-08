//! Generates passwords and passphrases with the official CLI's generator.
//!
//! The Generator page and the editor's generator dialog share one panel and
//! the options last used. Copying goes through the daemon, so the value is
//! marked sensitive and cleared like any other copy.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use countersign_ipc::{GenerateOptions, Request, Response, Secret};
use gtk::glib;

use zeroize::Zeroizing;

use super::label;
use crate::app::App;

/// The Generator page.
pub struct Generator {
    pub root: gtk::Widget,
    panel: Rc<Panel>,
    copy: gtk::Button,
}

/// The generated value, a button that makes another, and the options.
struct Panel {
    root: gtk::Box,
    app: Rc<App>,
    value: gtk::Label,
    current: RefCell<Option<Secret>>,
    notify: Rc<dyn Fn(&str)>,
    controls: Controls,
    pending: RefCell<Option<glib::SourceId>>,
    generation: Cell<u64>,
    /// Set while the controls show stored options, so that is not a change.
    applying: Cell<bool>,
    remember: RefCell<Option<glib::SourceId>>,
}

struct Controls {
    mode: adw::ToggleGroup,
    password: adw::PreferencesGroup,
    passphrase: adw::PreferencesGroup,
    length: adw::SpinRow,
    uppercase: adw::SwitchRow,
    lowercase: adw::SwitchRow,
    number: adw::SwitchRow,
    special: adw::SwitchRow,
    min_number: adw::SpinRow,
    min_special: adw::SpinRow,
    avoid_ambiguous: adw::SwitchRow,
    words: adw::SpinRow,
    separator: adw::EntryRow,
    capitalize: adw::SwitchRow,
    include_number: adw::SwitchRow,
}

impl Controls {
    fn new() -> Self {
        let defaults = GenerateOptions::default();
        let mode = adw::ToggleGroup::new();
        mode.add(adw::Toggle::builder().name("password").label("Password").build());
        mode.add(adw::Toggle::builder().name("passphrase").label("Passphrase").build());
        mode.set_halign(gtk::Align::Center);

        let password = adw::PreferencesGroup::builder().title("Password").build();
        let length = spin("Length", 5.0, 128.0, defaults.length);
        let uppercase = switch("A–Z", defaults.uppercase);
        let lowercase = switch("a–z", defaults.lowercase);
        let number = switch("0–9", defaults.number);
        let special = switch("!@#$%^&*", defaults.special);
        let min_number = spin("Minimum numbers", 0.0, 9.0, defaults.min_number);
        let min_special = spin("Minimum special", 0.0, 9.0, defaults.min_special);
        let avoid_ambiguous = switch("Avoid ambiguous characters", defaults.avoid_ambiguous);
        let rows: [&gtk::Widget; 8] = [
            length.upcast_ref(),
            uppercase.upcast_ref(),
            lowercase.upcast_ref(),
            number.upcast_ref(),
            special.upcast_ref(),
            min_number.upcast_ref(),
            min_special.upcast_ref(),
            avoid_ambiguous.upcast_ref(),
        ];
        for row in rows {
            password.add(row);
        }
        let passphrase = adw::PreferencesGroup::builder().title("Passphrase").build();
        let words = spin("Words", 3.0, 20.0, defaults.words);
        let separator = adw::EntryRow::builder().title("Separator").build();
        separator.set_max_length(1);
        let capitalize = switch("Capitalize", defaults.capitalize);
        let include_number = switch("Include a number", defaults.include_number);
        passphrase.add(&words);
        passphrase.add(&separator);
        passphrase.add(&capitalize);
        passphrase.add(&include_number);

        let controls = Controls {
            mode,
            password,
            passphrase,
            length,
            uppercase,
            lowercase,
            number,
            special,
            min_number,
            min_special,
            avoid_ambiguous,
            words,
            separator,
            capitalize,
            include_number,
        };
        controls.apply(&defaults);
        controls
    }

    fn options(&self) -> GenerateOptions {
        GenerateOptions {
            passphrase: self.mode.active_name().as_deref() == Some("passphrase"),
            length: self.length.value() as u32,
            uppercase: self.uppercase.is_active(),
            lowercase: self.lowercase.is_active(),
            number: self.number.is_active(),
            special: self.special.is_active(),
            min_number: self.min_number.value() as u32,
            min_special: self.min_special.value() as u32,
            avoid_ambiguous: self.avoid_ambiguous.is_active(),
            words: self.words.value() as u32,
            separator: self.separator.text().chars().take(1).collect(),
            capitalize: self.capitalize.is_active(),
            include_number: self.include_number.is_active(),
        }
    }

    fn apply(&self, options: &GenerateOptions) {
        self.mode.set_active_name(Some(if options.passphrase { "passphrase" } else { "password" }));
        self.length.set_value(f64::from(options.length));
        self.uppercase.set_active(options.uppercase);
        self.lowercase.set_active(options.lowercase);
        self.number.set_active(options.number);
        self.special.set_active(options.special);
        self.min_number.set_value(f64::from(options.min_number));
        self.min_special.set_value(f64::from(options.min_special));
        self.avoid_ambiguous.set_active(options.avoid_ambiguous);
        self.words.set_value(f64::from(options.words));
        self.separator.set_text(&options.separator);
        self.capitalize.set_active(options.capitalize);
        self.include_number.set_active(options.include_number);
    }
}

fn spin(title: &str, min: f64, max: f64, value: u32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(title);
    row.set_value(f64::from(value));
    row
}

/// Titles such as `!@#$%^&*` are plain text, not markup.
fn switch(title: &str, active: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder().title(title).use_markup(false).active(active).build()
}

/// Colors digits and symbols, as Bitwarden does, so similar characters are
/// easier to tell apart. The markup holds the secret, so it is wiped when
/// dropped.
pub(super) fn markup(value: &str) -> Zeroizing<String> {
    let (digit, symbol) = crate::theme::highlights();
    let mut out = Zeroizing::new(String::with_capacity(value.len() * 2));
    for c in value.chars() {
        let escaped = glib::markup_escape_text(&c.to_string());
        if c.is_ascii_digit() {
            out.push_str(&format!("<span foreground=\"{digit}\">{escaped}</span>"));
        } else if !c.is_alphanumeric() {
            out.push_str(&format!("<span foreground=\"{symbol}\">{escaped}</span>"));
        } else {
            out.push_str(&escaped);
        }
    }
    out
}

impl Panel {
    /// `actions` sit beside the regenerate button.
    fn new(app: &Rc<App>, notify: Rc<dyn Fn(&str)>, actions: &[&gtk::Widget]) -> Rc<Self> {
        let value = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::Char)
            .justify(gtk::Justification::Center)
            .css_classes(["cs-generated"])
            .build();
        value.update_property(&[gtk::accessible::Property::Label("Generated value")]);
        let regenerate = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Generate another")
            .css_classes(["circular"])
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::Center);
        buttons.append(&regenerate);
        for action in actions {
            buttons.append(*action);
        }

        let controls = Controls::new();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.add_css_class("cs-detail");
        column.append(&value);
        column.append(&buttons);
        column.append(&controls.mode);
        column.append(&controls.password);
        column.append(&controls.passphrase);

        let panel = Rc::new(Panel {
            root: column,
            app: app.clone(),
            value,
            current: RefCell::default(),
            notify,
            controls,
            pending: RefCell::default(),
            generation: Cell::new(0),
            applying: Cell::new(false),
            remember: RefCell::default(),
        });
        panel.connect(&regenerate);
        panel.show_mode();
        panel
    }

    fn connect(self: &Rc<Self>, regenerate: &gtk::Button) {
        let c = &self.controls;
        let weak = Rc::downgrade(self);
        let changed = move || {
            if let Some(me) = weak.upgrade()
                && !me.applying.get()
            {
                me.show_mode();
                me.schedule();
                me.remember();
            }
        };
        let changed: Rc<dyn Fn()> = Rc::new(changed);
        {
            let changed = changed.clone();
            c.mode.connect_active_name_notify(move |_| changed());
        }
        for row in [&c.length, &c.min_number, &c.min_special, &c.words] {
            let changed = changed.clone();
            row.connect_value_notify(move |_| changed());
        }
        for row in [
            &c.uppercase,
            &c.lowercase,
            &c.number,
            &c.special,
            &c.avoid_ambiguous,
            &c.capitalize,
            &c.include_number,
        ] {
            let changed = changed.clone();
            row.connect_active_notify(move |_| changed());
        }
        {
            let changed = changed.clone();
            c.separator.connect_changed(move |_| changed());
        }
        let weak = Rc::downgrade(self);
        regenerate.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.generate();
            }
        });
    }

    fn passphrase(&self) -> bool {
        self.controls.options().passphrase
    }

    fn show_mode(&self) {
        let passphrase = self.passphrase();
        self.controls.password.set_visible(!passphrase);
        self.controls.passphrase.set_visible(passphrase);
    }

    /// Shows the options last used, and a value made with them. The other
    /// place the generator appears may have changed the options since.
    fn load(self: &Rc<Self>) {
        let stored = self.app.settings.borrow().generator.clone().normalized();
        // A change still waiting to be stored is newer than the stored one.
        let changed = self.remember.borrow().is_none() && stored != self.controls.options();
        if changed {
            self.applying.set(true);
            self.controls.apply(&stored);
            self.applying.set(false);
            self.show_mode();
        }
        if changed || self.current.borrow().is_none() {
            self.generate();
        }
    }

    /// Stores the options shortly after they stop changing.
    fn remember(self: &Rc<Self>) {
        if let Some(source) = self.remember.borrow_mut().take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            if let Some(me) = weak.upgrade() {
                me.remember.borrow_mut().take();
                me.store();
            }
        });
        *self.remember.borrow_mut() = Some(source);
    }

    /// Stores the options now if a change is still waiting.
    fn store_pending(&self) {
        let Some(source) = self.remember.borrow_mut().take() else { return };
        source.remove();
        self.store();
    }

    fn store(&self) {
        let current = self.app.settings.borrow().clone();
        let generator = self.controls.options();
        if generator != current.generator {
            super::prefs::save(&self.app, countersign_ipc::Settings { generator, ..current });
        }
    }

    fn schedule(self: &Rc<Self>) {
        if let Some(source) = self.pending.borrow_mut().take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if let Some(me) = weak.upgrade() {
                me.pending.borrow_mut().take();
                me.generate();
            }
        });
        *self.pending.borrow_mut() = Some(source);
    }

    fn generate(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let options = self.controls.options();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let result = me.app.request(Request::Generate { options }).await;
            if me.generation.get() != generation {
                return;
            }
            match result {
                Ok(Response::Secret { value }) => {
                    me.value.set_markup(&markup(value.expose()));
                    *me.current.borrow_mut() = Some(value);
                }
                Ok(_) => {}
                Err(e) => (me.notify)(&e.message),
            }
        });
    }

    /// Forgets the shown value.
    fn clear(&self) {
        // Drop any regeneration still waiting or in flight, so it cannot
        // bring a value back.
        self.generation.set(self.generation.get() + 1);
        if let Some(source) = self.pending.borrow_mut().take() {
            source.remove();
        }
        self.current.borrow_mut().take();
        self.value.set_label("");
    }
}

impl Generator {
    pub fn new(
        app: &Rc<App>,
        notify: Rc<dyn Fn(&str)>,
        header_start: Option<&gtk::Widget>,
        receipt: &gtk::Widget,
    ) -> Rc<Self> {
        let copy = gtk::Button::builder().label("Copy").css_classes(["suggested-action", "pill"]).build();
        let panel = Panel::new(app, notify, &[copy.upcast_ref()]);
        let clamp = adw::Clamp::builder().maximum_size(640).child(&panel.root).build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let header = adw::HeaderBar::builder().title_widget(&label("Generator", &["title"])).build();
        if let Some(start) = header_start {
            header.pack_start(start);
        }
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scroll));
        view.add_bottom_bar(receipt);

        let generator = Rc::new(Generator { root: view.upcast(), panel, copy: copy.clone() });
        let weak = Rc::downgrade(&generator);
        copy.connect_clicked(move |_| {
            let Some(me) = weak.upgrade() else { return };
            let Some(value) = me.panel.current.borrow().clone() else { return };
            let label = if me.panel.passphrase() { "Passphrase" } else { "Password" };
            let panel = me.panel.clone();
            glib::spawn_future_local(async move {
                match panel.app.request(Request::CopyValue { value, label: label.into() }).await {
                    // The receipt bar shows the copy and its countdown.
                    Ok(_) => {}
                    Err(e) => (panel.notify)(&e.message),
                }
            });
        });
        generator
    }

    /// Moves focus into the page.
    pub fn focus(&self) {
        self.copy.grab_focus();
    }

    /// Shows the options last used, with a value made with them: the first
    /// time the page is shown, after a lock, and after the editor's dialog
    /// changed them.
    pub fn ensure_value(&self) {
        self.panel.load();
    }

    /// Forgets the shown value, for example when the vault locks.
    pub fn clear(&self) {
        self.panel.clear();
    }
}

/// Opens the generator in a dialog over `parent`, for the editor's password
/// field. **Use** hands the shown value to `on_use`. Option changes are
/// remembered as on the Generator page. The dialog keeps no value once it
/// closes.
pub fn choose(app: &Rc<App>, parent: &gtk::Widget, on_use: impl Fn(Secret) + 'static) -> adw::Dialog {
    let toasts = adw::ToastOverlay::new();
    let notify: Rc<dyn Fn(&str)> = Rc::new({
        let toasts = toasts.downgrade();
        move |message: &str| {
            if let Some(toasts) = toasts.upgrade() {
                toasts.add_toast(adw::Toast::new(message));
            }
        }
    });
    let panel = Panel::new(app, notify, &[]);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&panel.root)
        .build();
    toasts.set_child(Some(&scroll));

    let cancel = gtk::Button::with_label("Cancel");
    let accept = gtk::Button::builder().css_classes(["suggested-action"]).build();
    let header =
        adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&accept);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&toasts));
    let dialog = adw::Dialog::builder()
        .title("Generator")
        .content_width(460)
        .child(&view)
        .default_widget(&accept)
        // Enter means Use, not Cancel.
        .focus_widget(&accept)
        .build();

    let name_accept = {
        let (panel, accept) = (Rc::downgrade(&panel), accept.downgrade());
        move || {
            if let (Some(panel), Some(accept)) = (panel.upgrade(), accept.upgrade()) {
                accept.set_label(if panel.passphrase() { "Use passphrase" } else { "Use password" });
            }
        }
    };
    name_accept();
    panel.controls.mode.connect_active_name_notify(move |_| name_accept());
    {
        let dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    {
        let (panel, dialog) = (Rc::downgrade(&panel), dialog.downgrade());
        accept.connect_clicked(move |_| {
            let (Some(panel), Some(dialog)) = (panel.upgrade(), dialog.upgrade()) else { return };
            // Nothing to use until the first value arrives.
            let Some(value) = panel.current.borrow_mut().take() else { return };
            on_use(value);
            dialog.close();
        });
    }
    // The dialog's `closed` handler holds the panel, so it lives as long as
    // the dialog, and lets it go once the dialog closes.
    let held = RefCell::new(Some(panel.clone()));
    dialog.connect_closed(move |_| {
        if let Some(panel) = held.borrow_mut().take() {
            panel.store_pending();
            panel.clear();
        }
    });
    panel.load();
    dialog.present(Some(parent));
    dialog
}

#[cfg(test)]
mod tests {
    use super::{Controls, markup};
    use countersign_ipc::GenerateOptions;

    #[test]
    fn markup_escapes_and_colors() {
        let m = markup("a1<&");
        assert!(m.starts_with('a'));
        assert!(m.contains(">1</span>"));
        assert!(m.contains("&lt;") && m.contains("&amp;"));
        assert!(!m.contains("<&"));
    }

    /// The controls show stored options as they are, so the page and the
    /// dialog read back what the other stored.
    #[test]
    #[ignore = "needs a display"]
    fn controls_show_the_options_they_are_given() {
        gtk::init().expect("a display");
        adw::init().expect("libadwaita");
        let controls = Controls::new();
        assert_eq!(controls.options(), GenerateOptions::default());
        let options = GenerateOptions {
            passphrase: true,
            length: 64,
            uppercase: false,
            lowercase: true,
            number: false,
            special: true,
            min_number: 3,
            min_special: 4,
            avoid_ambiguous: true,
            words: 7,
            separator: "_".into(),
            capitalize: false,
            include_number: false,
        };
        controls.apply(&options);
        assert_eq!(controls.options(), options);
    }
}
