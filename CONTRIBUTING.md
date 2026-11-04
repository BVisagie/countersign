# Contributing

Bug reports, fixes and ideas are welcome. Countersign is built for Omarchy
first; changes for other Hyprland desktops are welcome when they keep Omarchy
working the same.

## Reporting a bug

Open an issue with the bug report form. Never include passwords, session
keys, API keys or anything from your vault. Report security problems
[privately](https://github.com/BVisagie/countersign/security/advisories/new),
as [SECURITY.md](docs/SECURITY.md) describes.

## Building

[INSTALL.md](docs/INSTALL.md#requirements) lists what the build needs. Then:

```bash
cargo build
```

Run these before every commit. CI runs the same:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Keep your real vault out of it

Your default Countersign folders belong to your real vault. Run a
development build in isolated folders:

```bash
export COUNTERSIGN_RUNTIME_DIR=/tmp/cs-dev/run XDG_DATA_HOME=/tmp/cs-dev/data XDG_CONFIG_HOME=/tmp/cs-dev/config
```

`COUNTERSIGN_DEMO=1` runs the app on a fictional vault, without the CLI or
an account. Use a throwaway Bitwarden account for anything that needs the
real CLI, including the contract test in
`crates/countersign-bw/tests/contract.rs`.

## Rules a change must keep

[AGENTS.md](AGENTS.md) has the full list. The ones that matter most:

- Countersign never implements Bitwarden's encryption, key derivation or
  server API. Every vault operation goes through the official CLI.
- No secret in logs, `Debug` output, process arguments or files.
- `bw serve` listens only on its socket pair, never on a TCP port.
- Editing sends back the complete item from the CLI with the changes
  applied.

## Pull requests

Keep a pull request to one change. Its description says what changed and
how you tested it. Screenshots come from `scripts/screenshots.sh`, which
uses the demo vault.

By contributing, you agree your work is licensed under the
[GPL-3.0-or-later](LICENSE), like the rest of Countersign.
