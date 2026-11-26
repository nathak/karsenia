#!/usr/bin/env bash
# Makes the macOS release zip and its .sha256 from a built (universal) karsenia-desktop binary. Run on macOS (it uses sips, iconutil, codesign, ditto):
#   packaging/make-macos-zip.sh <version> <commit> <path to the binary> <output folder>
# The zip holds one folder (karsenia-desktop-<version>-macos-universal) with Karsenia.app, the read-me and the fonts' license.
# The app carries an ad-hoc signature only (no key, no certificate, nothing secret): it is neither signed by a developer nor notarized.
# The folder it is put together in is a new temporary one (the system cleans it up); nothing is deleted here.
set -euo pipefail
[ "$(uname)" = Darwin ] || { echo "this script runs on macOS" >&2; exit 1; }
[ $# -eq 4 ] || { echo "usage: $0 <version> <commit> <binary> <outdir>" >&2; exit 2; }
version=$1 commit=$2 bin=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
name="karsenia-desktop-${version}-macos-universal"
[ -f "$bin" ] || { echo "no such file: $bin" >&2; exit 1; }
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
folder="$stage/$name"
app="$folder/Karsenia.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/karsenia-desktop"
chmod +x "$app/Contents/MacOS/karsenia-desktop"
sed -e "s/@VERSION@/${version}/g" "$here/Info.plist" > "$app/Contents/Info.plist"
# The icon: assets/icon.png (1024 px) in the sizes an .icns holds.
iconset="$stage/icon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}.png" > /dev/null
  sips -z $((size * 2)) $((size * 2)) "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/icon.icns"
# Ad-hoc: Apple silicon will not run an unsigned program at all, and this keeps the files' seal checkable.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
sed -e "s/@VERSION@/${version}/g" -e "s/@COMMIT@/${commit}/g" "$here/READ-ME-FIRST-macos.txt" > "$folder/READ-ME-FIRST.txt"
cp "$here/FONTS-LICENSE.txt" "$folder/"
# A zip made again replaces the old one.
[ -e "$out/$name.zip" ] && mv -f "$out/$name.zip" "$stage/old.zip"
ditto -c -k --keepParent "$folder" "$out/$name.zip"
(cd "$out" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
echo "$out/$name.zip"
