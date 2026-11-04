# Performance

Measured on 31 October 2026 on Omarchy 4.0.6 (Hyprland, x86_64) with the
official CLI 2026.10.1 and a test account holding 1,142 fictional items.
Release builds.

## The official CLI

Countersign keeps the CLI running as `bw serve` on a private socket pair,
instead of starting a `bw` command for each operation.

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

## Countersign

| What | Target | Measured |
| --- | --- | --- |
| Quick access opens | under 100 ms | 30–50 ms from request to mapped window |
| Search per keystroke, 5,000 items | under 16 ms | 0.21–0.42 ms |
| Vault shown after it changes, 1,142 items | | 3.5 ms to fetch, 13–34 ms on screen |
| Copy to the clipboard | under 50 ms | 12–18 ms, including confirming the compositor published it |
| Countersign's own memory | under 100 MB | app 74 MB, daemon 14 MB |
| Idle CPU | zero | app, daemon and CLI 0.00% over 30 s |

The GTK figures were measured on GTK's headless Broadway display. Painting
quick access on Hyprland has not been timed.
