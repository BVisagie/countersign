# M0 results

Measured 31 October 2026 on Omarchy 4.0.6 (Hyprland, x86_64) with the
official CLI 2026.10.1 and a throwaway EU account holding 1,142 fictional
items. Release builds unless noted.

## The official CLI

| Operation | One `bw` command each | `bw serve` over the socket pair |
| --- | --- | --- |
| Start | 2.6 s every command | 0.47–0.88 s, once |
| Unlock | 3.5 s | 0.69–0.94 s |
| List all items | 3.3 s | 0.3–0.5 s the first time, then 12–29 ms |
| Search (CLI side) | not measured | 10–12 ms |
| Open one item | not measured | 7–10 ms |
| Generate a password | not measured | about 7 ms |
| Idle CPU | | 0.0% |
| Memory | | 205–300 MB |

The contract test (`crates/countersign-bw/tests/contract.rs`) passes against
2026.10.1: create and edit round trips keep untouched fields, password history
is recorded by the CLI, re-unlocking rotates the session key, a restart with
the current key reopens the vault and the rotated-out key does not, trash and
restore, folders, text Sends, the generator and lock. One-time codes and
attachments answer "Premium status is required" on the free test account.

`bw serve` sometimes answers "Not found." for an item it created a moment
earlier, so the daemon uses the item returned by create instead of reading
it back.

## Countersign

| Gate | Target | Measured |
| --- | --- | --- |
| Quick access opens | under 100 ms | 30–50 ms from request to mapped window (Broadway); paint on Hyprland still to confirm |
| Search per keystroke, 5,000 items | under 16 ms | 0.21–0.42 ms |
| Vault shown after it changes, 1,142 items | | 3.5 ms to fetch, 13–34 ms on screen |
| Copy to the clipboard | under 50 ms | 17.5 ms from request to the clipboard owning it |
| Countersign's own memory | under 100 MB | app 74 MB, daemon 14 MB |
| Idle CPU | zero | app, daemon and CLI 0.00% over 30 s |

Copies offer `x-kde-passwordManagerHint` alongside the text types, which
Omarchy's clipboard history skips; the clipboard was empty after the
timeout. Locking stopped the CLI.

## Limits

The screen was locked during these runs, so the GTK measurements and
screenshots used GTK's headless Broadway display instead of Hyprland.
Hyprland stops sending frames to hidden surfaces, which stalls rendering
there. The paint latency of quick access and layer-shell placement still need
a check on an unlocked Hyprland session.
