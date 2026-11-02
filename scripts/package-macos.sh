#!/bin/bash
# Build the native bundle, then package without Finder/AppleScript layout automation.
set -euo pipefail
cd "$(dirname "$0")/.."
pnpm tauri build --bundles app
radar_version="$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).version")"
radar_arch="$(uname -m)"
radar_app="$PWD/target/release/bundle/macos/库存雷达.app"
radar_output="$PWD/target/release/bundle/dmg/库存雷达_${radar_version}_${radar_arch}.dmg"
radar_stage="$(mktemp -d "${TMPDIR:-/tmp}/radar-dmg.XXXXXX")"
trap 'rm -rf "$radar_stage"' EXIT
ditto "$radar_app" "$radar_stage/库存雷达.app"
ln -s /Applications "$radar_stage/Applications"
mkdir -p "$(dirname "$radar_output")"
hdiutil create -volname '库存雷达' -srcfolder "$radar_stage" -format UDZO -ov "$radar_output"
hdiutil verify "$radar_output"
printf 'macOS package: %s\n' "$radar_output"
