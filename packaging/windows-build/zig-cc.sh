#!/usr/bin/env bash
# `zig cc` as the C compiler and the linker for the x86_64-pc-windows-gnu target, when building on Linux (CI and the author's machine).
# Environment: ZIG = the zig program. The flags rustc and the cc crate pass for a GNU toolchain are dropped or changed to what zig's own
# mingw has; the import library of the `windows` crate (windows_x86_64_gnu) is found in the cargo registry.
set -euo pipefail
: "${ZIG:?ZIG must name the zig program}"
args=()
link=0
implib=""
for a in "$@"; do
  case "$a" in
    --target=*|-lgcc|-lgcc_eh|-lmsvcrt|-lmingwex|-lmingw32|-l:libpthread.a|-fno-use-linker-plugin) ;;
    -lwindows.0.52.0)
      if [ -z "$implib" ]; then
        implib=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -path '*/windows_x86_64_gnu-0.52.*/lib/libwindows.0.52.0.a' 2>/dev/null | sort | tail -n 1)
        [ -n "$implib" ] || { echo "zig-cc.sh: libwindows.0.52.0.a not found in the cargo registry (run cargo fetch first)" >&2; exit 1; }
      fi
      args+=("$implib");;
    -o) args+=("$a"); link=1;;
    *) args+=("$a");;
  esac
done
[ "$link" = 1 ] && args+=(-lunwind)
exec "$ZIG" cc -target x86_64-windows-gnu "${args[@]}"
