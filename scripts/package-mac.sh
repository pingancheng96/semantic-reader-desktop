#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
if [ "$(uname -s)" != Darwin ]; then
  printf '%s\n' '此命令需要 macOS。' >&2
  exit 1
fi
architecture="$(uname -m)"
target_dir="${CARGO_TARGET_DIR:-$PWD/src-tauri/target}"
output_dir="${1:-$PWD/dist/mac}"
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
npm run build
CARGO_TARGET_DIR="$target_dir" cargo build --locked --release --manifest-path src-tauri/Cargo.toml
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT HUP INT TERM
app="$stage/Semantic Reader.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp src-tauri/icons/icon.icns "$app/Contents/Resources/SemanticReader.icns"
cp "$target_dir/release/semantic-reader" "$app/Contents/MacOS/semantic-reader"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>semantic-reader</string>
<key>CFBundleIdentifier</key><string>local.semanticreader.desktop</string>
<key>CFBundleName</key><string>Semantic Reader</string>
<key>CFBundleDisplayName</key><string>Semantic Reader</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>SemanticReader.icns</string>
<key>CFBundleShortVersionString</key><string>0.20.24</string>
<key>CFBundleVersion</key><string>48</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
ditto -c -k --keepParent "$app" "$output_dir/Semantic-Reader-macOS-$architecture.zip"
printf '%s\n' "$output_dir/Semantic-Reader-macOS-$architecture.zip"
