#!/usr/bin/env python3
"""Writes THIRD-PARTY-NOTICES.txt: every Rust crate the desktop app is built from (all targets, `--features desktop`) with its license
expression and the license files its package ships, the web page's vendored scripts and the fonts. Identical license texts are printed once.

    cargo fetch --locked && python3 packaging/make-third-party-notices.py THIRD-PARTY-NOTICES.txt

Needs the crates' sources in the cargo registry (`cargo fetch`) and `cargo` on PATH. Nothing is downloaded or changed by this script itself.
"""
import hashlib
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NAMES = ("LICENSE", "LICENCE", "COPYING", "UNLICENSE", "NOTICE", "COPYRIGHT")


def license_files(manifest_dir):
    """The license files of a crate package: top-level files and files in a LICENSE(S) folder."""
    found = []
    for entry in sorted(os.listdir(manifest_dir)):
        path = os.path.join(manifest_dir, entry)
        upper = entry.upper()
        if os.path.isfile(path) and upper.startswith(NAMES):
            found.append(path)
        elif os.path.isdir(path) and upper in ("LICENSES", "LICENSE", "LICENCES"):
            for sub in sorted(os.listdir(path)):
                sub_path = os.path.join(path, sub)
                if os.path.isfile(sub_path):
                    found.append(sub_path)
    return found


def read(path):
    with open(path, "rb") as f:
        return f.read().decode("utf-8", "replace").replace("\r\n", "\n").strip()


def main(out):
    meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked", "--features", "desktop"], cwd=ROOT))
    resolved = {n["id"] for n in meta["resolve"]["nodes"]}
    crates = sorted((p for p in meta["packages"] if p["id"] in resolved and p["name"] != "tenrec"), key=lambda p: (p["name"], p["version"]))
    texts = {}  # sha -> (text, [crate ids])
    lines = []
    missing = []
    for p in crates:
        files = license_files(os.path.dirname(p["manifest_path"]))
        ids = []
        for f in files:
            body = read(f)
            if not body:
                continue
            sha = hashlib.sha256(body.encode()).hexdigest()[:10]
            texts.setdefault(sha, (body, []))[1].append(f'{p["name"]} {p["version"]}')
            ids.append(sha)
        if not ids:
            missing.append(f'{p["name"]} {p["version"]}')
        lines.append(f'{p["name"]} {p["version"]}\n    license: {p.get("license") or "(see the package)"}\n    source: {p.get("repository") or p.get("homepage") or "crates.io"}\n    texts: {", ".join(sorted(set(ids))) or "(none in the package)"}')
    with open(out, "w", encoding="utf-8") as o:
        o.write("THIRD-PARTY NOTICES\n\nKarsenia is licensed under GPL-3.0-only (see LICENSE). It is built from the Rust crates listed here, each under the license named\n"
                "next to it; where a crate offers a choice (for example `MIT OR Apache-2.0`) the choice compatible with GPL-3.0-only is taken, and the Slint crates are\n"
                "used under their GNU GPLv3 option. The texts of the licenses, as the crates' packages ship them, follow the list (identical texts once; the\n"
                "`texts:` line names them by the short id before each text).\n\n")
        o.write(f"=== CRATES ({len(crates)}) ===\n\n" + "\n\n".join(lines) + "\n\n")
        if missing:
            o.write("Packages that ship no license file (their license is the expression above): " + ", ".join(missing) + "\n\n")
        o.write("=== OTHER COMPONENTS ===\n\n")
        for title, rel in (("Scripts of the web page (marked, DOMPurify), ui/vendor/LICENSES.txt", "ui/vendor/LICENSES.txt"), ("Fonts (Pretendard, JetBrains Mono), ui/vendor/fonts/LICENSE.txt", "ui/vendor/fonts/LICENSE.txt")):
            o.write(f"--- {title} ---\n\n{read(os.path.join(ROOT, rel))}\n\n")
        o.write(f"=== LICENSE TEXTS ({len(texts)}) ===\n\n")
        for sha, (body, users) in sorted(texts.items(), key=lambda kv: (-len(kv[1][1]), kv[0])):
            shown = ", ".join(users[:6]) + (f" and {len(users) - 6} more" if len(users) > 6 else "")
            o.write(f"--- text {sha} (used by {len(users)}: {shown}) ---\n\n{body}\n\n")
    print(f"{out}: {len(crates)} crates, {len(texts)} distinct texts, {len(missing)} without a license file")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "THIRD-PARTY-NOTICES.txt"))
