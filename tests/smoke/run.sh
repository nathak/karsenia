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
printf '#!/bin/sh\nFAKE_SCENARIO=%s/scenario.jsonl exec %s/target/debug/fake-claude "$@"\n' "$work" "$root" > "$work/claude"
chmod +x "$work/claude"
cargo build --manifest-path "$root/Cargo.toml" >/dev/null
SWB_UI_DIR="$root/ui" "$root/target/debug/smart-workbench-app" --port 4311 --data "$work/data" --claude "$work/claude" > "$work/server.out" &
server=$!
trap 'kill $server 2>/dev/null; rm -rf "$work"' EXIT
sleep 1
node "$here/drive.mjs" "$work" "$(grep -o 'http://127.0.0.1:4311/#t=[0-9a-f]*' "$work/server.out")"
