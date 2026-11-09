#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/iridium-decks.svg.
#
# Needs: resvg (brew install resvg / cargo install resvg). On macOS, iconutil also writes the
# .icns. The outputs are committed, so builds and packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/iridium-decks.svg"
ID="ph.decks"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

# The artwork is a full-bleed 512 tile (rx=112). Windows and Linux use it as is. macOS icons sit on
# Apple's grid: an 824/1024 body with a transparent margin, so widen the viewBox to 512*1024/824.
MAC="$TMP/mac.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62.136 -62.136 636.272 636.272"/' "$SVG" >"$MAC"
grep -q 'viewBox="-62.136' "$MAC" || { echo "error: expected viewBox=\"0 0 512 512\" in $SVG" >&2; exit 1; }

render() {
  local src="$1" size="$2" dst="$3"
  resvg -w "$size" -h "$size" "$src" "$dst" </dev/null
  return 0
}

render "$SVG" 1024 "$DIR/iridium-decks-1024.png"
# Runtime window/Dock icon on macOS (embedded by apps/iridium-decks); other platforms use hicolor 256.
render "$MAC" 512 "$DIR/iridium-decks-macos-512.png"

# Linux hicolor theme.
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$SVG" "$DIR/hicolor/scalable/apps/$ID.svg"

# Windows icon: the hicolor PNGs, as a multi-resolution .ico.
ICO_PNGS=()
for s in 16 24 32 48 64 128 256; do
  ICO_PNGS+=("$DIR/hicolor/${s}x${s}/apps/$ID.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/iridium-decks.ico" "${ICO_PNGS[@]}")
echo "wrote $DIR/iridium-decks.ico"

# macOS icon: an .iconset, then iconutil.
if command -v iconutil >/dev/null && command -v sips >/dev/null; then
  SET="$TMP/iridium-decks.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$DIR/iridium-decks-1024.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z "$d" "$d" "$DIR/iridium-decks-1024.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns -o "$DIR/iridium-decks.icns" "$SET"
  echo "wrote $DIR/iridium-decks.icns"
else
  echo "warning: sips/iconutil not found (macOS only); iridium-decks.icns not regenerated" >&2
fi
