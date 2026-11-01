# Countersign

A fast, keyboard-first desktop app for Bitwarden on Omarchy and other
Hyprland desktops, written in Rust and built on the official Bitwarden CLI.

![The main window in Omarchy's Cumulus theme](docs/screenshots/main.png)

Countersign is an independent project, not affiliated with or endorsed by
Bitwarden, Inc. It never implements Bitwarden's encryption and never talks to
Bitwarden's servers itself. The official CLI does both; Countersign is the
interface and the desktop integration around it.

## What it does

- **Quick access.** A launcher-style overlay: type a few letters, press
  Enter, and the password is on the clipboard. Recent items come first.
- **The whole vault.** Logins, cards, identities, notes and SSH keys, with
  folders, collections, favorites and trash. Secrets stay masked until you
  reveal them. Verification codes count down live.
- **A clipboard that cleans up.** Copies clear themselves after 30 seconds
  and are marked so Omarchy's clipboard history skips them. After a password
  copy, the item's one-time code replaces it a few seconds later.
- **Locks when you do.** On request, after inactivity, when the screen locks
  and before the computer sleeps.
- **Looks like Omarchy.** Follows the active theme's palette and Hyprland's
  corner rounding, light or dark, and switches with the theme.

![Quick access searching for "git"](docs/screenshots/quick-search.png)

| Omarchy Flexoki Light | Omarchy Tokyo Night |
| --- | --- |
| ![A card in a light theme](docs/screenshots/light-card.png) | ![Search in Tokyo Night](docs/screenshots/tokyo-night-search.png) |

Screenshots use a fictional demo vault; see `scripts/screenshots.sh`.

## Status

Under active development; not released yet. See [the plan](docs/PLAN.md) for
what each milestone delivers and [M0 results](docs/M0-RESULTS.md) for the
measured performance.

## Requirements

- Omarchy, or another Hyprland desktop, with GTK 4.18+ and libadwaita 1.7+
- `gtk4-layer-shell` for quick access, and `vte4` for in-app sign-in
- The official Bitwarden CLI **2026.10.0 or newer**. Countersign can install
  the official release for you, verified against Bitwarden's published
  checksum.

## Install

```bash
scripts/install.sh
```

This builds `countersign` (the app) and `countersignd` (its background
service), installs them in `~/.local/bin`, and adds the desktop entry and
icon. Then add quick access and autostart to Hyprland:

```lua
-- ~/.config/hypr/bindings.lua
o.bind("SUPER + SHIFT + P", "Passwords", "countersign --quick")
-- ~/.config/hypr/autostart.lua
o.launch_on_start("countersign --background")
```

On first run Countersign offers to install the official CLI. It downloads the
pinned release from Bitwarden's GitHub, checks it against the recorded
SHA-256, and keeps it in `~/.local/share/countersign/bitwarden-cli/`. You can
point it at a CLI of your own in Settings instead.

## Use

| Key | Action |
| --- | --- |
| `Enter` | Copy the password, card number or note |
| `Shift+Enter` (quick access) | Copy the username or security code |
| `Ctrl+C` / `Ctrl+B` / `Ctrl+T` | Copy the password / username / verification code |
| `Ctrl+U` | Open the website |
| `Ctrl+O` (quick access) | Show the item in the main window |
| `Ctrl+F` or `/` | Search |
| `Ctrl+R` | Sync |
| `Ctrl+L` | Lock |
| `Ctrl+,` | Settings |
| `Esc` | Clear the search, then close quick access |

Copies made from quick access show a notification. When the verification
code is about to replace a copied password, the notification and the main
window offer **Keep password**. The tray icon shows whether the vault is open;
click it for Countersign, or use its menu for quick access and locking.

## How it works

See [ARCHITECTURE.md](docs/ARCHITECTURE.md) and [SECURITY.md](docs/SECURITY.md).

## License

[GNU General Public License v3.0 or later](LICENSE).
