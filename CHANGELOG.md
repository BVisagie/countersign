# Changelog

What changed in each Countersign release. The Bitwarden CLI versions are
the ones that release works with and installs.

## 0.1.0

The first release, for Omarchy on x86_64. It works with the official
Bitwarden CLI 2026.10.0 or newer, and installs 2026.10.1 when you let it.

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
- SSH keys cannot be created, and an existing key's key material cannot be
  changed, because the official CLI cannot generate or import keys.
