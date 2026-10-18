#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != Darwin ]]; then
    echo "tinyRDP requires macOS." >&2
    exit 1
fi

icon_work="$(mktemp -d)"
trap 'rm -rf "$icon_work"' EXIT
swift scripts/icon.swift "$icon_work/tinyRDP.iconset" resources/StatusIcon.pdf

python3 scripts/build_backend.py
cargo build --release --locked
app="dist/tinyRDP.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Resources/Licenses"
# Replace executable inodes so rebuilding cannot overwrite a running session.
cp target/release/tinyrdp "$app/Contents/MacOS/tinyrdp.new"
mv -f "$app/Contents/MacOS/tinyrdp.new" "$app/Contents/MacOS/tinyrdp"
python3 scripts/bundle_runtime.py target/freerdp-backend/build/bin/sdl-freerdp "$app"
cp resources/Info.plist "$app/Contents/Info.plist"
cp target/freerdp-backend/FreeRDP-3.32.2/LICENSE "$app/Contents/Resources/Licenses/FreeRDP-LICENSE.txt"
cp target/freerdp-backend/FreeRDP-3.32.2/resources/font/OFL.txt "$app/Contents/Resources/Licenses/OpenSans-OFL.txt"

iconutil -c icns "$icon_work/tinyRDP.iconset" -o "$app/Contents/Resources/tinyRDP.icns"

# Ad hoc signing supports a local build; set a Developer ID to distribute it.
codesign --force --sign "${CODESIGN_IDENTITY:--}" "$app/Contents/Helpers/sdl-freerdp"
codesign --force --sign "${CODESIGN_IDENTITY:--}" --identifier com.minirdp.app "$app"
codesign --verify --deep --strict "$app"
echo "Built $PWD/$app"
echo "Open it with: open dist/tinyRDP.app"
