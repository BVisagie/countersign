//! Follows the active Omarchy theme.
//!
//! Omarchy writes the current palette to `colors.toml` in its state folder
//! and replaces the folder when you switch themes. Countersign maps that
//! palette onto libadwaita's color variables, matches Hyprland's corner
//! rounding, and reloads when the theme changes. Without Omarchy it keeps
//! libadwaita's own look.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const BASE_CSS: &str = include_str!("style.css");

thread_local! {
    /// Colors for digits and symbols in generated values: the theme's blue
    /// and orange, or libadwaita's.
    static HIGHLIGHTS: RefCell<(String, String)> = RefCell::new(("#3584e4".into(), "#e66100".into()));
}

/// The current colors for digits and for symbols.
pub fn highlights() -> (String, String) {
    HIGHLIGHTS.with(|h| h.borrow().clone())
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn parse(text: &str) -> Option<Rgb> {
        let hex = text.trim().strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
    }

    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    fn alpha(self, a: f32) -> String {
        format!("rgba({},{},{},{a:.3})", self.0, self.1, self.2)
    }

    fn mix(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

/// The colors Countersign uses from an Omarchy theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub dark: bool,
    bg: Rgb,
    bg_dark: Rgb,
    bg_darker: Rgb,
    bg_lighter: Rgb,
    fg: Rgb,
    fg_dim: Rgb,
    muted: Rgb,
    accent: Rgb,
    selection: Rgb,
    selection_fg: Rgb,
    border: Rgb,
    red: Rgb,
    green: Rgb,
    yellow: Rgb,
    tones: [Rgb; 6],
}

impl Palette {
    pub fn from_toml(text: &str) -> Option<Palette> {
        let table: toml::Table = toml::from_str(text).ok()?;
        let get = |key: &str| table.get(key).and_then(|v| v.as_str()).and_then(Rgb::parse);
        let bg = get("background")?;
        let fg = get("foreground")?;
        let accent = get("accent").or_else(|| get("blue"))?;
        let dark = table.get("mode").and_then(|v| v.as_str()) != Some("light");
        let toward = if dark { Rgb(0, 0, 0) } else { Rgb(255, 255, 255) };
        let away = if dark { Rgb(255, 255, 255) } else { Rgb(0, 0, 0) };
        let bg_dark = get("dark_background").unwrap_or_else(|| bg.mix(toward, 0.12));
        Some(Palette {
            dark,
            bg,
            bg_dark,
            bg_darker: get("darker_background").unwrap_or_else(|| bg.mix(toward, 0.24)),
            bg_lighter: get("lighter_background").unwrap_or_else(|| bg.mix(away, 0.08)),
            fg,
            fg_dim: get("dark_foreground").unwrap_or_else(|| fg.mix(bg, 0.25)),
            muted: get("muted").unwrap_or_else(|| fg.mix(bg, 0.45)),
            accent,
            selection: get("selection").unwrap_or_else(|| accent.mix(bg, 0.55)),
            selection_fg: get("selection_foreground").unwrap_or(fg),
            border: get("hyprland_active_border").unwrap_or(accent),
            red: get("red").unwrap_or(Rgb(0xe0, 0x6c, 0x75)),
            green: get("green").unwrap_or(Rgb(0x98, 0xc3, 0x79)),
            yellow: get("yellow").unwrap_or(Rgb(0xe5, 0xc0, 0x7b)),
            tones: [
                get("blue").unwrap_or(accent),
                get("cyan").unwrap_or(accent),
                get("green").unwrap_or(accent),
                get("magenta").unwrap_or(accent),
                get("orange").or_else(|| get("yellow")).unwrap_or(accent),
                get("red").unwrap_or(accent),
            ],
        })
    }

    /// CSS that maps the palette onto libadwaita and Countersign's own
    /// variables.
    pub fn css(&self, radius: u32) -> String {
        let ink = if self.dark { self.bg_darker } else { Rgb(255, 255, 255) };
        let mut css = format!(
            ":root {{
  --window-bg-color: {bg}; --window-fg-color: {fg};
  --view-bg-color: {bg_dark}; --view-fg-color: {fg};
  --headerbar-bg-color: {bg_darker}; --headerbar-fg-color: {fg};
  --headerbar-backdrop-color: {bg_darker}; --headerbar-border-color: {line};
  --headerbar-shade-color: {line}; --headerbar-darker-shade-color: {line};
  --sidebar-bg-color: {bg_darker}; --sidebar-fg-color: {fg}; --sidebar-backdrop-color: {bg_darker};
  --sidebar-border-color: {line}; --sidebar-shade-color: {line};
  --secondary-sidebar-bg-color: {bg_dark}; --secondary-sidebar-fg-color: {fg};
  --secondary-sidebar-backdrop-color: {bg_dark}; --secondary-sidebar-border-color: {line};
  --secondary-sidebar-shade-color: {line};
  --card-bg-color: {card}; --card-fg-color: {fg}; --card-shade-color: {line};
  --popover-bg-color: {bg_lighter}; --popover-fg-color: {fg}; --popover-shade-color: {shadow};
  --dialog-bg-color: {bg}; --dialog-fg-color: {fg};
  --overview-bg-color: {bg_darker}; --overview-fg-color: {fg};
  --thumbnail-bg-color: {bg_dark}; --thumbnail-fg-color: {fg};
  --accent-bg-color: {accent}; --accent-fg-color: {ink}; --accent-color: {accent};
  --destructive-bg-color: {red}; --destructive-fg-color: {ink}; --destructive-color: {red};
  --success-bg-color: {green}; --success-fg-color: {ink}; --success-color: {green};
  --warning-bg-color: {yellow}; --warning-fg-color: {ink}; --warning-color: {yellow};
  --error-bg-color: {red}; --error-fg-color: {ink}; --error-color: {red};
  --border-color: {line}; --shade-color: {shadow}; --scrollbar-outline-color: transparent;
  --window-radius: {radius}px;
  --cs-radius: {radius}px; --cs-radius-small: {small}px;
  --cs-bg-darker: {bg_darker}; --cs-bg-lighter: {bg_lighter};
  --cs-dim: {fg_dim}; --cs-muted: {muted}; --cs-line: {line}; --cs-line-strong: {line_strong};
  --cs-hover: {hover}; --cs-selection: {selection}; --cs-selection-fg: {selection_fg};
  --cs-focus: {accent}; --cs-frame: {frame};
}}
window, popover, dialog, tooltip {{ font-family: monospace; }}
",
            bg = self.bg.hex(),
            fg = self.fg.hex(),
            bg_dark = self.bg_dark.hex(),
            bg_darker = self.bg_darker.hex(),
            bg_lighter = self.bg_lighter.hex(),
            fg_dim = self.fg_dim.hex(),
            muted = self.muted.hex(),
            accent = self.accent.hex(),
            ink = ink.hex(),
            red = self.red.hex(),
            green = self.green.hex(),
            yellow = self.yellow.hex(),
            selection = self.selection.hex(),
            selection_fg = self.selection_fg.hex(),
            frame = self.border.hex(),
            card = self.fg.alpha(0.035),
            line = self.fg.alpha(0.12),
            line_strong = self.fg.alpha(0.28),
            hover = self.fg.alpha(0.07),
            shadow = Rgb(0, 0, 0).alpha(if self.dark { 0.45 } else { 0.18 }),
            small = radius.min(6),
        );
        for (i, tone) in self.tones.iter().enumerate() {
            css.push_str(&format!(
                ".cs-tone-{i} {{ background-color: {}; color: {}; }}\n",
                tone.alpha(0.22),
                tone.mix(self.fg, 0.15).hex()
            ));
        }
        css
    }
}

/// The folder holding the active Omarchy theme, if there is one.
fn theme_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    [state.join("omarchy/current/theme"), config.join("omarchy/current/theme")]
        .into_iter()
        .find(|d| d.join("colors.toml").is_file())
}

/// Hyprland's window rounding, so Countersign's corners match.
fn hyprland_rounding() -> Option<u32> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let path = PathBuf::from(runtime).join("hypr").join(instance).join(".socket.sock");
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(300))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(300))).ok()?;
    stream.write_all(b"j/getoption decoration:rounding").ok()?;
    let mut buf = Vec::new();
    stream.take(64 << 10).read_to_end(&mut buf).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&buf).ok()?;
    value["int"].as_u64().map(|r| r.min(24) as u32)
}

fn current_css() -> (String, Option<bool>) {
    let mut css = BASE_CSS.to_owned();
    let palette = theme_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("colors.toml")).ok())
        .and_then(|text| Palette::from_toml(&text));
    let dark = palette.as_ref().map(|p| p.dark);
    let highlights = palette
        .as_ref()
        .map(|p| (p.tones[0].hex(), p.tones[4].hex()))
        .unwrap_or_else(|| ("#3584e4".into(), "#e66100".into()));
    HIGHLIGHTS.with(|h| *h.borrow_mut() = highlights);
    if let Some(palette) = palette {
        css.push_str(&palette.css(hyprland_rounding().unwrap_or(0)));
    }
    (css, dark)
}

/// Applies the theme to the display and keeps it current.
pub fn install() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let apply = {
        let provider = provider.clone();
        move || {
            let (css, dark) = current_css();
            provider.load_from_string(&css);
            let scheme = match dark {
                Some(true) => adw::ColorScheme::ForceDark,
                Some(false) => adw::ColorScheme::ForceLight,
                None => adw::ColorScheme::Default,
            };
            adw::StyleManager::default().set_color_scheme(scheme);
        }
    };
    apply();

    // Omarchy replaces the theme folder and `theme.name` on every switch.
    let Some(current) = theme_dir().and_then(|d| d.parent().map(PathBuf::from)) else { return };
    let Ok(monitor) =
        gio::File::for_path(&current).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
    else {
        return;
    };
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    monitor.connect_changed(move |_, _, _, _| {
        if let Some(source) = pending.borrow_mut().take() {
            source.remove();
        }
        let apply = apply.clone();
        let slot = pending.clone();
        let source = glib::timeout_add_local_once(Duration::from_millis(250), move || {
            slot.borrow_mut().take();
            apply();
        });
        *pending.borrow_mut() = Some(source);
    });
    // The monitor stops when dropped; keep it for the life of the app.
    std::mem::forget(monitor);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_omarchy_palettes_and_rejects_bad_colors() {
        let palette = Palette::from_toml(
            r##"mode = "dark"
accent = "#77baf3"
background = "#193e58"
foreground = "#eee2cc"
red = "#ff8e7c"
"##,
        )
        .unwrap();
        assert!(palette.dark);
        let css = palette.css(0);
        assert!(css.contains("--accent-bg-color: #77baf3"));
        assert!(css.contains("--cs-radius: 0px"));
        assert!(Palette::from_toml("background = \"#19}3e58\"\nforeground = \"#fff\"").is_none());
        assert!(Palette::from_toml("background = \"red; } * { x\"").is_none());
    }
}
