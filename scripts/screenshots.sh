#!/usr/bin/env bash
# Renders Countersign's screenshots with the fictional demo vault, offscreen
# on GTK's headless Broadway display. Nothing appears on your desktop and no
# real vault is used.
#
#   scripts/screenshots.sh [out-dir] [theme ...]
#
# Themes are folder names in /usr/share/omarchy/themes; "current" (the
# default) uses your active Omarchy theme.
set -euo pipefail

out=${1:-docs/screenshots}
shift || true
themes=("$@")
[ ${#themes[@]} -eq 0 ] && themes=(current)
bin=${COUNTERSIGN_BIN:-target/debug/countersign}
display=:$((40 + RANDOM % 50))

gtk4-broadwayd "$display" >/dev/null 2>&1 &
broadway=$!
trap 'kill "$broadway" 2>/dev/null || true' EXIT
sleep 1

for theme in "${themes[@]}"; do
  state=$(mktemp -d)
  if [ "$theme" != current ]; then
    mkdir -p "$state/omarchy/current/theme"
    cp "/usr/share/omarchy/themes/$theme/colors.toml" "$state/omarchy/current/theme/"
    export XDG_STATE_HOME=$state
  fi
  GDK_BACKEND=broadway BROADWAY_DISPLAY=$display COUNTERSIGN_SCREENSHOT="$out/$theme" \
    timeout 120 "$bin" 2>&1 | grep -v -e CRITICAL -e 'not a layer surface' || true
  unset XDG_STATE_HOME
  rm -rf "$state"
done
