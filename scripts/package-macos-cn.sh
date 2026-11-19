#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
  printf '%s\n' '需要 Apple 芯片版 macOS 才能构建此安装包。' >&2
  exit 1
fi

destination="${1:?usage: bash scripts/package-macos-cn.sh OUTPUT_DIR}"
mkdir -p "$destination"
destination="$(cd "$destination" && pwd)"
cd "$(dirname "$0")/.."

target_dir="$(cargo metadata --locked --no-deps --format-version 1 | jq -er .target_directory)"
cargo build --release --locked -p tofa --target aarch64-apple-darwin
cargo tauri build --ci --bundles app -- --locked

app="$target_dir/release/bundle/macos/Tofa CN.app"
cli="$target_dir/aarch64-apple-darwin/release/tofa"
test -x "$app/Contents/MacOS/Tofa"
test -x "$cli"
lipo -verify_arch arm64 "$app/Contents/MacOS/Tofa"
lipo -verify_arch arm64 "$cli"
"$cli" --version | grep -E '^tofa [0-9]+\.'

stage="$(mktemp -d "${TMPDIR:-/tmp}/temp-tofa-package.XXXXXXXX")"
trap 'rm -r "$stage"' EXIT
ditto "$app" "$stage/Tofa CN.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname 'Tofa CN' -srcfolder "$stage" -format UDZO -ov \
  "$destination/Tofa-CN-macOS-arm64.dmg"
hdiutil verify "$destination/Tofa-CN-macOS-arm64.dmg"
tar -czf "$destination/tofa-cli-macOS-arm64.tar.gz" -C "$(dirname "$cli")" tofa
cd "$destination"
shasum -a 256 Tofa-CN-macOS-arm64.dmg tofa-cli-macOS-arm64.tar.gz > SHA256SUMS.txt
