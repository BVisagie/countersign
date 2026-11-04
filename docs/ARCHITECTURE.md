# Architecture

```text
Hyprland key / tray / `countersign --quick`
            │
            ▼
  countersign (GTK app)  main window · quick access · theme
            │   private Unix socket, same user only, one JSON object per line
            ▼
  countersignd (daemon)  session · vault index · clipboard · locks
            │   socket pair (fd+connected://), HTTP/1.1
            ▼
  bw serve (official Bitwarden CLI)  encryption · sync · server API
```

## Principles

- Countersign never implements Bitwarden's encryption or calls Bitwarden's
  servers. The official CLI does both.
- `bw serve` listens only on a private socket pair, never on a TCP port or a
  filesystem socket.
- Only the daemon holds the session. The app receives item metadata, and a
  secret only when you reveal or edit it. Copying never sends the secret to
  the app.
- The daemon talks to the CLI through an interface shaped like `bw serve`,
  so Bitwarden's Rust CLI can replace the Node CLI once it has `serve`.
- The app uses GTK4 and libadwaita through gtk4-rs directly, the idiom of
  GNOME's Rust applications, for direct control over focus and list
  performance.

## The CLI adapter

`countersign-bw` runs one-off commands (`--version`, `status`, `config
server`, `login --apikey`, `logout`, `lock`) with a scrubbed environment,
bounded output and timeouts. Only proxy and certificate variables pass
through; `LD_PRELOAD`, `NODE_OPTIONS` and everything else are dropped.

For vault work it starts `bw serve --hostname fd+connected://N` with the
other end of a socket pair. The CLI serves exactly one connection, which
only the daemon holds, and exits when it closes. `bw serve` checks the Host
header against its configured hostname and port, so every request sends
`Host: localhost:8087`. One-off commands cost two to three seconds each; a
request to the running server costs milliseconds
([measurements](PERFORMANCE.md)).

The CLI answers one request at a time on its connection, and a sync takes
0.2 to 2 seconds, so the daemon syncs ten seconds after unlocking, after the
usual unlock-then-copy. `bw serve` sometimes answers "Not found." for an
item or folder it created a moment earlier, so the daemon uses what a change
returns instead of reading it back, and asks again for up to two seconds
when a call on an object the index knows finds nothing.

`bw serve` 2026.10.1 also often lists items and folders as they were before
the latest change: an edited item keeps its old name, a renamed folder its
old name, a new one is missing. The list stays behind until a later change
or a sync, and reading an item back can be behind for a moment too. So the
daemon keeps what each change left, from the change's reply or, for the
trash and attachments, from its own index, and lays it over every list
until the list catches up. A newer revision from elsewhere wins.
`crates/countersign-daemon/src/contract.rs` checks this against the real
CLI.

The CLI refuses to serve until an account is logged in, and has no login
route, so sign-in uses `bw login` itself.

Two Send commands also run one-off, with the session key in the child's
environment. `bw serve` has no route for file Sends, so `bw send create`
reads its request on stdin. And `bw serve` 2026.10.1 answers `/send/list`
from a cache it never refreshes: creates, edits, deletes and syncs do not
show there until the server restarts. `bw send list` reads the stored vault
each time, at the usual two to three seconds, so the app updates its Send
list from each change's reply and lists again only after a sync or unlock.

Editing a Send through the CLI removes its password unless the request
carries one; the CLI never returns it. The editor therefore asks for the
password again to keep it.

## The daemon

`countersignd` starts on demand from the app, holds a lock so only one runs
per user, and exits once no app is connected, the vault is locked and none of
its values remain on the clipboard.

- **Unlocking.** The unlock screen asks the daemon to start the CLI while you
  type. Unlock posts the password to `/unlock`, keeps the session key the CLI
  returns, then builds a metadata index from `/list/object/items` and syncs
  in the background.
- **The index.** Summaries hold names, usernames, hosts, flags, folders and
  card brand with the last four digits. Passwords, codes, card numbers and
  notes are never in it.
- **Sends** reach the app without their text, key or access link. The editor
  asks for the text when you edit a Send, and the daemon copies links.
- **Recovery.** If the CLI stops while unlocked, the next request restarts
  it with the session key, which reopens the vault without the password.
- **Locking** never waits behind other work. It takes the session out first,
  bumps an epoch so in-flight unlocks, syncs and rebuilds discard their
  results, asks the CLI to lock, stops it and clears our clipboard value.
- **Re-prompt.** Items that require the master password refuse reveal, copy
  and edit until it is entered. The daemon checks it with `/unlock`, which
  rotates the CLI's session key, and adopts the new key.
- **Clipboard.** `wl-clipboard-rs` serves each copy from its own thread
  inside the daemon, with the `x-kde-passwordManagerHint` type that tells
  clipboard history, including Omarchy's, to skip it. A timer clears it only
  if it is still Countersign's value.
- **Triggers.** An inactivity timer; Hyprland's session lock, read from its
  IPC socket every 500 ms while unlocked; logind's `Lock` signal for the
  display session; and a logind sleep delay inhibitor held only while
  unlocked, released after locking.

## Installing the CLI

Only when you ask, the daemon downloads the pinned release archive for this
machine from Bitwarden's GitHub release over HTTPS, hashing it as it streams
and refusing it unless the SHA-256 matches the digest recorded with the pin.
It extracts only `bw`, runs `bw --version` in an empty profile, requires
exactly the pinned version, and moves it into
`~/.local/share/countersign/bitwarden-cli/<version>/` with one rename. A CLI
you configured is never replaced; installing switches Countersign to the
managed copy.

When a Countersign update raises the pin, Settings offers the new release in
place of the older managed copy. The download and checks run with the vault
still open. Only then does the daemon lock, stopping every CLI process
started from the old copy, move the new one in and remove the older
releases.

## The app

`countersign` is a single-instance GTK application that stays running in the
background, so quick access is a window that is already built. A second
`countersign --quick` hands its command line to the running instance.

The app talks to the daemon with GIO's asynchronous socket API on the main
loop, so it has no threads of its own. Search ranks the index in the app with
nucleo's fuzzy matcher. Lists are `GtkListView`s, which create rows only for
what is visible.

A StatusNotifierItem tray icon (ksni) shows the lock state and opens the
windows. Copies made while the main window is not focused raise a desktop
notification, withdrawn when the value is cleared.

The theme reads `colors.toml` from the active Omarchy theme, maps it onto
libadwaita's color variables, matches Hyprland's `decoration:rounding`, and
reloads when the theme folder changes. Without Omarchy, libadwaita's own look
remains. The mascot is built into the app as symbolic icons (`build.rs`,
`data/icons`), which GTK draws in the theme's colors.

`COUNTERSIGN_DEMO=1` swaps the daemon for a fictional in-process vault, which
the screenshot script and UI work use.
