#!/bin/sh
# Browser smoke test: the app against the fake claude, driven in headless Chromium.
#   PLAYWRIGHT=/path/to/node_modules/playwright/index.mjs tests/smoke/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work=$(mktemp -d)
mkdir -p "$work/project" "$work/data"
python3 - "$work" <<'PY'
import struct, sys, zlib
work = sys.argv[1]
w, h, rgb = 160, 100, (40, 120, 220)
raw = b''.join(b'\x00' + bytes(rgb) * w for _ in range(h))
def chunk(kind, data): return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
open(work + '/project/shot.png', 'wb').write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
PY
cp "$here/scenario.jsonl" "$work/scenario.jsonl"
cp "$here/propose-clean.json" "$work/propose.json"
cp "$here/sched-result.json" "$work/sched.json"
printf '#!/bin/sh\nFAKE_SCENARIO=%s/scenario.jsonl FAKE_PROPOSE=%s/propose.json FAKE_SCHED=%s/sched.json exec %s/target/debug/fake-claude "$@"\n' "$work" "$work" "$work" "$root" > "$work/claude"
chmod +x "$work/claude"
# A stand-in docker for the container section: `run ... smoke-image claude <args>` starts the fake claude, anything else does nothing.
printf '#!/bin/sh\ncase "$1" in\n  run) shift; while [ $# -gt 0 ]; do if [ "$1" = smoke-image ]; then shift; break; fi; shift; done; shift\n       FAKE_SCENARIO=%s/scenario.jsonl exec %s/target/debug/fake-claude "$@" ;;\n  *) exit 0 ;;\nesac\n' "$work" "$root" > "$work/docker"
chmod +x "$work/docker"
cargo build --manifest-path "$root/Cargo.toml" >/dev/null
SWB_UI_DIR="$root/ui" "$root/target/debug/smart-workbench-app" --port 4311 --data "$work/data" --claude "$work/claude" > "$work/server.out" &
server=$!
trap 'kill $server 2>/dev/null; rm -rf "$work"' EXIT
sleep 1
node "$here/drive.mjs" "$work" "$(grep -o 'http://127.0.0.1:4311/#t=[0-9a-f]*' "$work/server.out")"
