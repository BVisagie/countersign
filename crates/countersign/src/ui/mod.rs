//! Widgets.

pub mod detail;
pub mod editor;
pub mod gates;
pub mod generator;
pub mod item_actions;
pub mod items;
pub mod prefs;
pub mod quick;
pub mod reprompt;
pub mod sends;
pub mod window;

use countersign_ipc::ItemKind;
use gtk::prelude::*;

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(classes.to_vec()).build()
}

/// A square tile with the first letter of a name, tinted by the name.
pub fn monogram(name: &str, large: bool) -> gtk::Label {
    let tile = gtk::Label::new(None);
    set_monogram(&tile, name, large);
    tile
}

pub fn set_monogram(tile: &gtk::Label, name: &str, large: bool) {
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect::<String>());
    tile.set_label(letter.as_deref().unwrap_or("•"));
    let tone = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))) % 6;
    let mut classes = vec!["cs-monogram".to_owned(), format!("cs-tone-{tone}")];
    if large {
        classes.push("large".into());
    }
    tile.set_css_classes(&classes.iter().map(String::as_str).collect::<Vec<_>>());
    tile.set_valign(gtk::Align::Center);
    tile.set_halign(gtk::Align::Center);
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("flat");
    button.set_valign(gtk::Align::Center);
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

pub fn keycap(text: &str) -> gtk::Label {
    let key = gtk::Label::new(Some(text));
    key.add_css_class("cs-key");
    key.set_valign(gtk::Align::Center);
    key
}

pub fn kind_icon(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Login => "dialog-password-symbolic",
        ItemKind::Card => "auth-smartcard-symbolic",
        ItemKind::Identity => "contact-new-symbolic",
        ItemKind::SecureNote => "accessories-text-editor-symbolic",
        ItemKind::SshKey => "utilities-terminal-symbolic",
    }
}

pub fn kind_plural(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Login => "Logins",
        ItemKind::Card => "Cards",
        ItemKind::Identity => "Identities",
        ItemKind::SecureNote => "Notes",
        ItemKind::SshKey => "SSH keys",
    }
}

/// Removes every child of a box.
pub fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// A short, friendly time like "2 minutes ago" for an RFC 3339 timestamp.
pub fn relative_time(rfc3339: &str) -> String {
    let Ok(then) = gtk::glib::DateTime::from_iso8601(rfc3339, None) else { return String::new() };
    let Ok(now) = gtk::glib::DateTime::now_utc() else { return String::new() };
    let secs = now.difference(&then).as_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 { format!("1 {unit} ago") } else { format!("{n} {unit}s ago") }
}

/// A date like "14 Mar 2026" for an RFC 3339 timestamp.
pub fn short_date(rfc3339: &str) -> String {
    gtk::glib::DateTime::from_iso8601(rfc3339, None)
        .and_then(|d| d.to_local())
        .and_then(|d| d.format("%-d %b %Y"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
