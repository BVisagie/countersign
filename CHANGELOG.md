# Changelog

What changed in each Countersign release. The Bitwarden CLI versions are
the ones that release works with and installs.

## 0.2.0

Adds Bitwarden's archive and fixes a folder that was not there. It needs the
official Bitwarden CLI 2026.10.1 or newer, and installs 2026.10.1 when you let
it.

To update, quit Countersign, run `pkill -x countersignd`, then install. The
app and its background service changed together: if the old service is still
running, the app says they are different versions until it exits, about a
minute later.

- **Archive**: archived items have their own row above Trash, and stay out
  of All items, search and quick access, as in the web vault. The item's
  menu offers **Archive** and **Unarchive**, with an Undo. Archiving needs
  Bitwarden Premium.
- **Archived items in the trash** now show there. They were in no list
  before.
- **No extra "No Folder"**: the CLI's own entry no longer shows in the
  sidebar or the editor as if it were a folder.
- **The main menu** moved beside the name at the top of the sidebar, so it
  is there on every page.
- **Cancel and Save** stay with the form on a wide window, away from the
  window's close button.
- **Quitting asks first**: the close button, `Ctrl+Q` and the tray's
  **Quit** no longer throw away an edit without asking.
- **The version** shows before the vault opens and in Settings, and any
  build but an official release says it is a dev build.
- **Long email addresses** fit in the sidebar.
- **Items show their latest change**: reading an item straight after
  changing it no longer shows the old version, or undoes the change with
  the next edit.

Known limitations:

- If Premium ends, the CLI cannot list archived items, so they stay in the
  web vault until they are unarchived there.
- Attachments and file Sends are built but not yet tried on a Premium
  account.
- The ARM build is built and tested automatically, and starts on the demo
  vault, but has not yet been tried on an ARM computer.
- SSH keys cannot be created, and an existing key's key material cannot be
  changed, because the official CLI cannot generate or import keys.

## 0.1.0

The first release, for Omarchy on x86_64 and ARM (aarch64). It works with
the official Bitwarden CLI 2026.10.0 or newer, and installs 2026.10.1 when
you let it.

- **Quick access**: a launcher-style overlay that copies a password in a
  few keystrokes, recent items first, with the item's one-time code copied
  a few seconds after its password.
- **The whole vault**: logins, cards, identities, notes and SSH keys, with
  folders, organization collections, favorites, trash, live verification
  codes and a password and passphrase generator.
- **Editing**: create and edit logins, cards, identities and notes, with
  websites and custom fields; manage folders and attachments, move items to
  an organization, and trash, restore or delete forever.
- **Send**: share text or a file through Bitwarden Send.
- **A clipboard that cleans up**: copies clear after 30 seconds and are
  kept out of Omarchy's clipboard history.
- **Locking**: on request, after inactivity, when the screen locks and
  before sleep.
- **Omarchy's look**: the active theme's palette and Hyprland's rounding,
  light or dark, following theme changes.
- **The official CLI, installed for you**: checked against a SHA-256
  recorded in Countersign's source, and updated from Settings when a
  Countersign release raises the pinned version.

Known limitations:

- Attachments and file Sends are built but not yet tried on a Premium
  account.
- The ARM build is built and tested automatically, and starts on the demo
  vault, but has not yet been tried on an ARM computer.
- SSH keys cannot be created, and an existing key's key material cannot be
  changed, because the official CLI cannot generate or import keys.
