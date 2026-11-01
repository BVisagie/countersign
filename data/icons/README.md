# Icon and mascot

Countersign's mascot is a meerkat standing sentry. A countersign is the
password a sentry asks for, and the sentry's upright outline is also a
keyhole. In the app icon, its tail underlines it like a second signature:
the other meaning of countersign.

| File | Where it appears |
| --- | --- |
| `io.github.bvisagie.Countersign.svg` | The app icon, in full color on its own tile: launchers, notifications and the README. |
| `io.github.bvisagie.Countersign-symbolic.svg` | The sentry on watch: the unlock, sign-in, CLI setup and "Opening your vault…" pages. Also installed as the app's symbolic icon. |
| `io.github.bvisagie.Countersign-at-ease-symbolic.svg` | The sentry at ease, eyes closed: the "No item selected" page in the open vault. |

## Rules

- **Inside the app the mascot has one color, the theme's.** GTK draws a
  `-symbolic` icon in its widget's CSS `color`, so the mascot follows every
  Omarchy theme and changes with it. Full-color art inside the app would
  clash with some themes.
- **Symbolic files are a single path with nonzero winding.** Clockwise
  shapes fill and anticlockwise shapes cut out. Keep to plain paths: GTK
  draws symbolic icons from a subset of SVG.
- **The app carries its own copies.** `crates/countersign/build.rs` builds
  the files listed in `../countersign.gresource.xml` into the binary, so they
  show before Countersign is installed, in the demo and in screenshots. Add a
  new icon there, as a constant in `crates/countersign/src/ui/mod.rs`, and to
  the test in `crates/countersign/src/main.rs`.
- **No shield, no Bitwarden blue.** Countersign is not affiliated with
  Bitwarden, and its icon must not suggest that it is.
- **Check changes at 16, 32 and 64 pixels, and in every Omarchy theme.**
  `scripts/screenshots.sh /tmp/shots $(ls /usr/share/omarchy/themes)` renders
  the unlock page (`unlock.png`) and the empty item page (`main-empty.png`)
  in each installed theme.

App icon colors: tile `#1b2633`, fur `#e3c393`, ears `#c9a476`, belly
`#efdab8`, eye patches and nose `#3b2a20`, tail tip `#5a3d2b`.

The artwork is licensed with the rest of the repository, under the GNU
General Public License v3.0 or later.
