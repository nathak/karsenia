# SmartWorkbench App

A desktop-style host for [Claude Code](https://claude.com/claude-code): a Rust core runs `claude` headless
(`--input-format stream-json --output-format stream-json`) and serves a custom chat UI on this machine only.
MVP 1 is a browser UI over HTTP + WebSocket; a Tauri wrap, a file tree and editor, a PTY terminal and a second agent
backend (Hermes Agent) come later.

한국어: Claude Code를 헤드리스로 띄워 JSON 이벤트로 주고받고, 직접 만든 채팅 화면(이미지 붙여넣기·표시, 도구 호출 카드,
승인 대화상자, 중단, 다시 열기)을 보여 주는 로컬 앱입니다. 지금은 브라우저 화면이며, Tauri·에디터·터미널·헤르메스는 다음 단계입니다.

## Run

```sh
cargo run            # prints  http://127.0.0.1:PORT/#t=TOKEN  — open that address
cargo run -- --port 4311 --data ./data --claude /path/to/claude
```

The token is in the address fragment (never sent over HTTP); the page moves it to `sessionStorage` and clears the address.
Sessions are kept in `~/.local/share/smart-workbench-app` (`SWB_APP_DATA` or `--data` to change): a meta file and an event log each,
so a chat can be reopened and replayed, and continues with `claude --resume` after a restart.

`SWB_UI_DIR=./ui cargo run` serves the UI from disk, so editing it needs no rebuild.

### Building on a machine without a C compiler

Rust needs a linker driver named `cc`. If there is none (and no `sudo`), a user-level Zig works as one:

```sh
pip install --target ~/.cache/swb-tools/py ziglang
printf '#!/bin/sh\nPYTHONPATH=%s/py exec python3 -m ziglang cc "$@"\n' ~/.cache/swb-tools > ~/.cache/swb-tools/cc && chmod +x ~/.cache/swb-tools/cc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=~/.cache/swb-tools/cc
```

## What it does

- One `claude` process per chat, stdin kept open for several turns, `--include-partial-messages` for streaming text.
- **Permissions**: it starts `claude` with `--permission-prompt-tool stdio`, so every permission request arrives here as a
  `can_use_tool` control request and waits for a click. Nothing is approved on its own: no timeout, no auto-allow, a closed
  page leaves the request pending, an answer carries the request's own `input` (never one the page sent), each request is
  answered once, Esc in the dialog is a refusal.
- **Images**: paste, drop or pick (png, jpeg, gif, webp, 5 MB each, five per message) go in as base64 image blocks; images in tool
  results show in the tool card; an image file inside the project that a tool names (or a result mentions) is fetched through
  `GET /api/file`, which needs the token, resolves links and refuses anything outside the project or not an image (no SVG).
- **Interrupt** sends the CLI's `interrupt` control request; the turn ends with `error_during_execution`.
- Events the host does not know are passed to the UI as raw JSON (and shown in the debug drawer), so a new CLI version cannot break parsing.

## Security

Local only: binds `127.0.0.1`; every request needs the right `Host` (DNS rebinding gets 421); a WebSocket needs the exact
`Origin` and the token as its first message within 5 s; the file endpoint needs a `Bearer` token; a strict CSP (no inline script,
no remote image or script), `nosniff`, `no-referrer`. Markdown goes through DOMPurify (images and forms are removed).
The app can run commands through Claude Code, so treat its address like a terminal.

## Tests

```sh
cargo test                       # protocol, process, session, server (a fake claude replays scenarios; no model calls)
cargo clippy --all-targets -- -D warnings
node --test tests/ui.test.mjs    # the UI's pure helpers
PLAYWRIGHT=/path/to/playwright/index.mjs tests/smoke/run.sh   # headless-Chromium run of the UI against the fake
SWB_LIVE=1 cargo test --test live -- --ignored                # the real claude (costs a few cents)
```

`fake-claude` replays a scenario file: lines are printed as they are, or are directives (`{"$wait":"user"|"control_response"|"interrupt"}`,
`{"$sleep":ms}`, `{"$exit":n}`).

## Layout

`src/claude/` everything specific to the CLI (protocol, process) · `src/session.rs` one chat's task (state, pending permissions, log) ·
`src/registry.rs` sessions on disk · `src/server/` HTTP + WebSocket + auth + files · `ui/` plain ES modules, `ui/vendor/` marked and DOMPurify (pinned, licenses included).

## Verified against Claude Code 2.1.316

Multi-turn over one process, `--resume <session id>` (remembers), image input, `can_use_tool` allow and deny, `interrupt`
(`control_response` then `result/error_during_execution` within 0.1 s), and the whole permission round trip through this host
with the real `claude` (the live test). Not yet checked: how SmartWorkbench's guard questions (`$.ui.ask`) behave in a headless
host, the IDE connection protocol, and Windows/macOS.

## Next

Tauri wrap (needs `pkg-config` and `webkit2gtk-4.1` dev packages) · file tree and editor · PTY terminal · Hermes Agent as a second backend
(ACP over stdio or its `/v1/runs` API) · an "always allow" choice that writes `updatedPermissions`.
