//! The keyboard shortcuts dialog (Ctrl+?) and the About dialog.

use adw::prelude::*;

use super::keycap;

/// Each group's shortcuts: keys, with alternatives separated by " or ", and
/// what they do.
const GROUPS: [(&str, &[(&str, &str)]); 5] = [
    (
        "Vault",
        &[
            ("Ctrl+F or /", "Search"),
            ("↑ or ↓", "Move through the results"),
            ("Enter", "Copy the password, card number or note"),
            ("Ctrl+C", "Copy the password"),
            ("Ctrl+B", "Copy the username"),
            ("Ctrl+T", "Copy the verification code"),
            ("Ctrl+U", "Open the website"),
            ("Ctrl+N", "New login"),
            ("Ctrl+R", "Sync"),
        ],
    ),
    ("Editing", &[("Ctrl+E", "Edit the item"), ("Ctrl+S", "Save"), ("Esc", "Cancel")]),
    ("Tools", &[("Ctrl+G", "Password generator"), ("Ctrl+Shift+S", "Send")]),
    (
        "Quick access",
        &[
            ("Enter", "Copy the password, card number or note"),
            ("Shift+Enter", "Copy the username or card security code"),
            ("Ctrl+T", "Copy the verification code"),
            ("Ctrl+U", "Open the website"),
            ("Ctrl+O or Ctrl+Enter", "Show the item in the main window"),
            ("Esc", "Clear the search, then close"),
        ],
    ),
    (
        "Countersign",
        &[
            ("Ctrl+L", "Lock"),
            ("Ctrl+,", "Settings"),
            ("Ctrl+?", "Keyboard shortcuts"),
            ("Ctrl+Q", "Quit, which locks the vault"),
        ],
    ),
];

/// The keys for one shortcut as keycaps, with "or" between alternatives.
fn keys(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_valign(gtk::Align::Center);
    for (i, alternative) in text.split(" or ").enumerate() {
        if i > 0 {
            row.append(&super::label("or", &["cs-quick-hint"]));
        }
        row.append(&keycap(alternative));
    }
    row
}

pub fn present_shortcuts(parent: Option<&gtk::Widget>) {
    let page = adw::PreferencesPage::new();
    for (title, shortcuts) in GROUPS {
        let group = adw::PreferencesGroup::builder().title(title).build();
        for (key, action) in shortcuts {
            let row = adw::ActionRow::builder().title(*action).build();
            row.add_suffix(&keys(key));
            group.add(&row);
        }
        page.add(&group);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title("Keyboard shortcuts")
        .content_width(520)
        .content_height(640)
        .child(&view)
        .build();
    dialog.present(parent);
}

pub fn present_about(parent: Option<&gtk::Widget>) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Countersign")
        // Inside the app the mascot is always the theme-colored symbolic icon.
        .application_icon(super::SENTRY_ICON)
        .version(crate::VERSION)
        .developer_name("BVisagie")
        .website(env!("CARGO_PKG_REPOSITORY"))
        .issue_url(concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"))
        .comments(
            "A keyboard-first Bitwarden app for Omarchy, built on the official Bitwarden CLI. \
             An independent project, not affiliated with or endorsed by Bitwarden, Inc.",
        )
        .license_type(gtk::License::Gpl30)
        .build();
    dialog.present(parent);
}
