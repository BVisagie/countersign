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

| Milestone | Delivers |
| --- | --- |
| M0 | Foundation and feasibility gates: CLI adapter and contract test, daemon, protocol, the GTK app on real data, Omarchy theming, measured performance, screenshots |
| M1 | Replaces the Warden plugin: managed CLI install, embedded sign-in, unlock, quick access with code-after-password, clipboard expiry, inactivity, screen-lock and sleep locks, tray, Hyprland binding, settings |
| M2 | The full vault window: every item type, folders, collections, organizations, live codes, reveal, password history, attachment download, generator, sync status |
| M3 | Editing and Send: create, edit and delete items and folders, custom fields, websites, attachments, moving to an organization, text and file Sends |

Later, not in this plan: auto-type, an SSH agent, polkit or fingerprint quick
unlock, multiple accounts, import and export, a native sign-in form,
packaging.

## Decisions

- Private repository, GPL-3.0-or-later.
- GTK4 with libadwaita, using gtk4-rs directly. GNOME's Rust applications use
  this idiom; it gives direct control over focus and list performance.
- Features that need Bitwarden Premium (one-time codes, attachments, file
  Sends) are built and tested manually on a Premium account.
