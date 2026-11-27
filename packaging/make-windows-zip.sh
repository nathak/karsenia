#!/usr/bin/env bash
# Makes the Windows release zip and its .sha256 from a built karsenia-desktop.exe:
#   packaging/make-windows-zip.sh <version> <commit> <path to karsenia-desktop.exe> <output folder>
# The zip holds one folder (karsenia-desktop-<version>-windows-x86_64) with the program, the signing script, the read-me, the license (GPL-3.0),
# the third-party notices and the fonts' license.
# The folder it is put together in is a new temporary one (the system cleans it up); nothing is deleted here.
set -euo pipefail
[ $# -eq 4 ] || { echo "usage: $0 <version> <commit> <exe> <outdir>" >&2; exit 2; }
version=$1 commit=$2 exe=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
name="karsenia-desktop-${version}-windows-x86_64"
[ -f "$exe" ] || { echo "no such file: $exe" >&2; exit 1; }
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
mkdir "$stage/$name"
cp "$exe" "$stage/$name/karsenia-desktop.exe"
cp "$here/sign-windows.ps1" "$here/FONTS-LICENSE.txt" "$root/LICENSE" "$root/THIRD-PARTY-NOTICES.txt" "$stage/$name/"
sed -e "s/@VERSION@/${version}/g" -e "s/@COMMIT@/${commit}/g" "$here/READ-ME-FIRST.txt" > "$stage/$name/READ-ME-FIRST.txt"
# A zip made again replaces the old one (zip would add to it).
[ -e "$out/$name.zip" ] && mv -f "$out/$name.zip" "$stage/old.zip"
(cd "$stage" && zip -r -X -q "$out/$name.zip" "$name")
(cd "$out" && sha256sum "$name.zip" > "$name.zip.sha256")
echo "$out/$name.zip"
