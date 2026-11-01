# Plan

Agreed 31 October 2026. Countersign is a Rust and GTK4 desktop app for
Bitwarden on Omarchy, built on the official Bitwarden CLI. The bar for the
project: fast, good-looking and stable, or not built at all.

## Principles

1. Never implement Bitwarden encryption or call Bitwarden's servers. The
   official CLI does all of it.
2. The CLI's `bw serve` listens only on a private socket pair, never on a TCP
   port or a filesystem socket.
3. Only the daemon holds the session. The app receives item metadata, and a
   secret only when you reveal or edit it. Copying never sends the secret to
   the app.
4. The CLI sits behind an interface shaped like `bw serve`, so Bitwarden's
   Rust CLI can replace the Node CLI once it ships `serve`.
5. A minimum stable CLI version, an optional pinned and checksum-verified
   install, and no background downloads.

## Milestones

| Milestone | Delivers | Status |
| --- | --- | --- |
| M0 | Foundation and feasibility gates: CLI adapter and contract test, daemon, protocol, the GTK app on real data, Omarchy theming, measured performance, screenshots | Done (#1) |
| M1 | Replaces the Warden plugin: managed CLI install, embedded sign-in, unlock, quick access with code-after-password, clipboard expiry, inactivity, screen-lock and sleep locks, tray, Hyprland binding, settings | Done (#2) |
| M2 | The full vault window: every item type, folders, collections, organizations, live codes, reveal, password history, attachment download, generator, sync status | Done (#3) |
| M3 | Editing and Send: create, edit and delete items and folders, custom fields, websites, attachments, moving to an organization, text and file Sends | Done (#4) |

M0–M3 reached `main` on 1 November 2026 (#5).

## Next

Known follow-ups, not yet scheduled:

- **Manual testing on a Premium account:** verification codes and the code
  after the password, attachments, and file Sends.
- **Installing a newer pinned CLI.** When a Countersign update raises the
  pinned release, Countersign keeps using the copy it installed before, and
  offers no way to install the new one. Settings should offer it.
- **zbus 6**, once the tray library `ksni` moves to it. The daemon stays on
  zbus 5 until then, so the workspace builds one zbus, not two.
- **SSH keys:** creating them, and changing their key material, if the
  official CLI gains key generation or import.
- **Edits right after unlocking.** The sync that starts at unlock was seen
  once to show an edit's old value until the next sync; the server had the
  new one.
- **A CI build on the minimum Rust version**, so `rust-version` cannot fall
  behind what the dependencies need again.

## Later

Not in this plan: auto-type, an SSH agent, polkit or fingerprint quick
unlock, multiple accounts, import and export, a native sign-in form,
packaging.

## Decisions

- Private repository, GPL-3.0-or-later.
- GTK4 with libadwaita, using gtk4-rs directly. GNOME's Rust applications use
  this idiom; it gives direct control over focus and list performance.
- Features that need Bitwarden Premium (one-time codes, attachments, file
  Sends) are built and tested manually on a Premium account.
