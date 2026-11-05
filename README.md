<p align="center">
  <img src="data/icons/io.github.bvisagie.Countersign.svg" width="128" alt="Countersign's icon: a meerkat standing sentry, shaped like a keyhole">
</p>

# Countersign

A fast, keyboard-first desktop app for Bitwarden on Omarchy and other
Hyprland desktops, written in Rust and built on the official Bitwarden CLI.

![The main window in the InkyPinky theme](docs/screenshots/main.png)

Countersign is an independent project, not affiliated with or endorsed by
Bitwarden, Inc. It never implements Bitwarden's encryption and never talks to
Bitwarden's servers itself. The official CLI does both; Countersign is the
interface and the desktop integration around it.

## What it does

- **Quick access.** A launcher-style overlay: type a few letters, press
  Enter, and the password is on the clipboard. Recent items come first.
- **The whole vault.** Logins, cards, identities, notes and SSH keys, with
  folders, organization collections, favorites and trash, and a password and
  passphrase generator. Secrets stay masked until you
  reveal them. Verification codes count down live.
- **Editing.** Create and edit logins, cards, identities and notes, with
  websites and custom fields; SSH keys keep their key and the rest can be
  edited. Manage folders and attachments, move items to an organization,
  and trash, restore or delete forever.
- **Send.** Share text or a file through Bitwarden Send. A new Send's link
  goes straight to the clipboard.
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

| Editing an item | Send |
| --- | --- |
| ![The item editor](docs/screenshots/edit.png) | ![The Send page](docs/screenshots/sends.png) |

Screenshots use a fictional demo vault; see `scripts/screenshots.sh`. Most
are in HANCORE's [InkyPinky](https://github.com/HANCORE-linux/omarchy-inkypinky-theme)
theme for Omarchy.

## Status

Early: 0.1.0 is the first release. It is built and tested on Omarchy.
[The roadmap](docs/ROADMAP.md) lists the known limitations,
[the changelog](CHANGELOG.md) what each release changed, and
[Performance](docs/PERFORMANCE.md) has the measurements.

## Quick start

You need Omarchy on an Intel or AMD (x86_64) computer, and a Bitwarden
account. On another Hyprland desktop, or to run the latest code,
[build it from source](docs/INSTALL.md#build-from-source) instead.

1. **Download and install.** Run these in a terminal:

   ```bash
   mkdir -p ~/Downloads/countersign && cd ~/Downloads/countersign
   curl -LO https://github.com/BVisagie/countersign/releases/latest/download/countersign-x86_64.tar.gz
   curl -LO https://github.com/BVisagie/countersign/releases/latest/download/SHA256SUMS
   sha256sum -c SHA256SUMS && tar xf countersign-x86_64.tar.gz && countersign-x86_64/scripts/install.sh
   ```

   `sha256sum -c` checks the download and should say
   `countersign-x86_64.tar.gz: OK`; if it does not, nothing is unpacked or
   installed. The install script installs Countersign
   for your user. Omarchy lacks one library it needs, `vte4`, so the script
   offers to install it with pacman, which asks for your password first.

2. **Open Countersign** from your app launcher, or run `countersign`.

3. **Install the Bitwarden CLI.** Countersign never encrypts your vault or
   talks to Bitwarden's servers itself. Bitwarden's own command-line program,
   the Bitwarden CLI, does both, and Countersign is the window around it.
   If it asks, choose **Install the official CLI**: Countersign downloads it
   and checks it against a checksum recorded in its source. Countersign works with CLI
   **2026.10.0 or newer**, and installs 2026.10.1.

4. **Sign in.** Choose your server (US cloud, EU cloud or self-hosted), sign
   in with your email or an API key, then unlock with your master password.
   [First run](docs/USAGE.md#first-run) explains each choice.

5. **Add a key for quick access.** Add these lines to your Hyprland
   configuration:

   ```lua
   -- ~/.config/hypr/bindings.lua
   o.rebind("SUPER + SHIFT + P", "Passwords", "countersign --quick")
   -- ~/.config/hypr/autostart.lua
   o.launch_on_start("countersign --background")
   ```

   The first line makes `Super+Shift+P` open quick access, in place of
   Omarchy's Google Photos shortcut. Hyprland picks it up when you save.
   The second starts Countersign when you log in, so quick access opens
   instantly.

Now press `Super+Shift+P`, type a few letters of a site and press `Enter`:
the password is on the clipboard, and it clears itself after 30 seconds.

- [Installing](docs/INSTALL.md): requirements, choosing a CLI, updating,
  uninstalling and troubleshooting.
- [Using Countersign](docs/USAGE.md): sign-in, quick access, copying,
  editing, Send, locking and settings.

## Keys

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
| `Ctrl+N` | New login |
| `Ctrl+G` | Password generator |
| `Ctrl+,` | Settings |
| `Esc` | Clear the search, then close quick access |

More in [Using Countersign](docs/USAGE.md).

## How it works

See [ARCHITECTURE.md](docs/ARCHITECTURE.md) and [SECURITY.md](docs/SECURITY.md).

## License

[GNU General Public License v3.0 or later](LICENSE).
