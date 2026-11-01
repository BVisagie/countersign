# Countersign

A Rust and GTK4 desktop app for Bitwarden on Omarchy and Hyprland, built on
the official Bitwarden CLI.

| Crate | Role |
| --- | --- |
| `crates/countersign-bw` | Adapter for the official CLI: one-off commands and a `bw serve` child on a private socket pair |
| `crates/countersign-ipc` | App ↔ daemon protocol, item summaries, field model and redaction |
| `crates/countersign-daemon` | `countersignd`: owns the session, clipboard and lock triggers |
| `crates/countersign` | The GTK app: main window, quick access, theming |

## Checks

Run these before every commit. CI runs the same.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Without the `vte4` library installed, add `--no-default-features` to the
clippy and test commands; the `terminal` feature needs it.

## Rules that are easy to break

- Never implement Bitwarden encryption, key derivation or server API calls.
  Every vault operation goes through the official CLI. This is the project's
  reason to exist.
- Never put a secret in logs, `Debug` output, process arguments or files.
  Hold secrets in `Secret`, `SecretString` or `Zeroizing`. The app process
  receives a secret only when the user reveals or edits it; copying happens
  in the daemon.
- `bw serve` listens only on the socket pair (`fd+connected://`). Never bind
  a TCP port. Requests must send `Host: localhost:8087`, or the CLI refuses
  them.
- Editing sends back the complete item read from the CLI with changes
  applied. Building an item from scratch drops fields Countersign does not
  know.
- Bump `PROTOCOL_VERSION` in `countersign-ipc` for any incompatible change to
  a request, response or event.
- When changing `MINIMUM_VERSION` or `PINNED_VERSION` in `countersign-bw`,
  rerun the contract test against that release and update the installer's
  checksums.

## Development data

Your default Countersign folders belong to your real vault. Run development
daemons in isolated folders:

```bash
export COUNTERSIGN_RUNTIME_DIR=/tmp/cs-dev/run XDG_DATA_HOME=/tmp/cs-dev/data XDG_CONFIG_HOME=/tmp/cs-dev/config
```

The contract test (`crates/countersign-bw/tests/contract.rs`) and any manual
CLI testing use a throwaway Bitwarden account only. The test documents its
environment variables; it creates and permanently deletes objects named
`countersign-contract …`. Bitwarden requires Premium for one-time codes,
attachments and file Sends, so a free test account reports those as
unavailable.

`COUNTERSIGN_DEMO=1` runs the app on a fictional vault without the CLI.
`scripts/screenshots.sh` renders screenshots offscreen from that vault; use it
for every image in the repository.
