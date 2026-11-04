# Installing Countersign

Countersign runs on Omarchy and other Hyprland desktops. There is no package
yet; you build it from source with one script.

## Requirements

- **A Hyprland desktop with GTK 4.18+ and libadwaita 1.7+.** Omarchy already
  has both, and `gtk4-layer-shell`, which quick access needs.
- **Rust 1.92 or newer**, with Cargo, from rustup or mise.
- **Build tools and libraries.** On Omarchy or Arch:

  ```bash
  sudo pacman -S --needed base-devel git gtk4 libadwaita gtk4-layer-shell vte4
  ```

  `vte4` runs the Bitwarden CLI's sign-in inside Countersign. Omarchy does
  not include it. The install script checks for these libraries and offers
  to install any that are missing.
- **Optional: a bar with a system tray** (StatusNotifierItem), for the tray
  icon. Everything else works without one.
- **The official Bitwarden CLI.** Countersign can install it for you; see
  [The Bitwarden CLI](#the-bitwarden-cli).

## Install

```bash
git clone https://github.com/BVisagie/countersign.git
cd countersign
scripts/install.sh
```

The script builds Countersign and installs it for your user only:

| What | Where |
| --- | --- |
| `countersign` (the app) and `countersignd` (its background service) | `~/.local/bin/` |
| Desktop entry | `~/.local/share/applications/io.github.bvisagie.Countersign.desktop` |
| Icons | `~/.local/share/icons/hicolor/scalable/apps/io.github.bvisagie.Countersign.svg` and `~/.local/share/icons/hicolor/symbolic/apps/io.github.bvisagie.Countersign-symbolic.svg` |

`XDG_BIN_HOME` and `XDG_DATA_HOME` move these folders if you set them.
`~/.local/bin` needs to be on your `PATH`, as it is on Omarchy.

Before building, the script checks for the libraries in
[Requirements](#requirements). If any are missing, it offers to install them
with `sudo pacman -S --needed`, and pacman lists what it will install and
asks before it changes anything. Run without a terminal, the script prints
that command and stops instead. Nothing else on your system is changed.

## Add it to Hyprland

Bind a key for quick access, and start Countersign with your session so
quick access opens instantly:

```lua
-- ~/.config/hypr/bindings.lua
o.bind("SUPER + SHIFT + P", "Passwords", "countersign --quick")
-- ~/.config/hypr/autostart.lua
o.launch_on_start("countersign --background")
```

The same lines, with copy buttons, are under **Settings → Desktop**.

| Command | Does |
| --- | --- |
| `countersign` | Opens the main window |
| `countersign --quick` | Shows or hides quick access |
| `countersign --lock` | Locks the vault |
| `countersign --background` | Starts without opening a window |

Each command starts Countersign if it is not running, and otherwise goes to
the copy that is. The desktop entry also offers **Quick access** and **Lock
the vault** in your launcher.

## The Bitwarden CLI

Countersign never encrypts or talks to Bitwarden's servers itself; the
official Bitwarden CLI does both. Countersign needs a copy.

### Let Countersign install it

On first run, choose **Install the official CLI**. Countersign downloads
Bitwarden CLI **2026.10.1** for your computer from Bitwarden's GitHub
releases over HTTPS. It refuses the download unless its SHA-256 matches the
checksum recorded in Countersign's source, then checks `bw --version` and
keeps it in `~/.local/share/countersign/bitwarden-cli/2026.10.1/`.
Countersign downloads only when you ask, never in the background.

### Use your own CLI

Any stable release **2026.10.0 or newer** works; prereleases and development
builds are refused. Put `bw` on your `PATH`, or choose it under **Settings →
Bitwarden CLI → Use a CLI on this computer…**. **Use the managed CLI** switches
back.

Arch's own `bitwarden-cli` package is too old (2026.2.0 at the time of
writing), and Countersign reports "This Bitwarden CLI is too old" for it. Use
the managed install, or Bitwarden's npm package (`@bitwarden/cli`) or
release download.

### Which copy runs

Countersign uses the first of:

1. The CLI chosen in Settings.
2. The copy Countersign installed.
3. `bw` on your `PATH`.

### Versions

The minimum, 2026.10.0, is the oldest release Countersign has been tested
against. There is no maximum: newer releases are accepted. The copy
Countersign installs is pinned to one release, because its checksum is
recorded in Countersign's source.

A Countersign update can raise the pin. Countersign keeps using the copy it
installed before until you choose **Install** under **Settings → Bitwarden
CLI**, next to the newer version. The vault stays open while the download
is checked, then Countersign locks, switches to the new copy and removes the
old one. If the download fails, nothing changes. A CLI you chose yourself is
never offered an update; update it as you normally would.

### Your own `bw` stays separate

Countersign keeps its own CLI profile in `~/.local/share/countersign/profile`.
Signing in or out in Countersign does not touch the `bw` you use in a
terminal, and the other way round.

## Updating

```bash
cd countersign
git pull
scripts/install.sh
```

Then restart Countersign so the new version runs: quit it with `Ctrl+Q` or
the tray's **Quit**, which locks the vault. Its background service exits by
itself about a minute later; `pkill -x countersignd` stops it at once. Then
start Countersign again.

If you use your own CLI, update it as you normally would.

## Uninstalling

1. Sign out under **Settings → Account → Sign out…**. Your vault stays on
   Bitwarden's servers.
2. Quit Countersign with `Ctrl+Q`, and run `pkill -x countersignd`.
3. Remove the two lines from your Hyprland configuration.
4. Remove its files:

   ```bash
   rm ~/.local/bin/countersign ~/.local/bin/countersignd
   rm ~/.local/share/applications/io.github.bvisagie.Countersign.desktop
   rm ~/.local/share/icons/hicolor/scalable/apps/io.github.bvisagie.Countersign.svg
   rm ~/.local/share/icons/hicolor/symbolic/apps/io.github.bvisagie.Countersign-symbolic.svg
   rm -r ~/.config/countersign ~/.local/share/countersign
   ```

   The last line removes your settings, the CLI Countersign installed and its
   CLI profile. If you installed with `XDG_BIN_HOME` or `XDG_DATA_HOME` set,
   or keep your configuration elsewhere, use those folders instead.

## Troubleshooting

**"This Bitwarden CLI is too old".** Countersign found a CLI older than
2026.10.0, often Arch's package. Install the official CLI from the same
page, or choose a newer one.

**"Countersign can't reach the Bitwarden CLI".** The message below it comes
from the CLI. Check the CLI chosen in Settings, or switch to the managed CLI.

**Quick access does not appear.** Run `countersign --quick` in a terminal and
read its output. Quick access needs `gtk4-layer-shell`. Check that the
binding's command is `countersign --quick`.

**No tray icon.** Your bar needs a StatusNotifierItem tray. The tray is
optional; the main window and quick access do everything it does.

**The build stops: "The system library `vte-2.91-gtk4` … was not found".**
A library from [Requirements](#requirements) is missing; the message names
it. Run `scripts/install.sh` in a terminal so it can offer to install it, or
install the packages listed there yourself.
