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

Rust needs a linker driver named `cc`, and the bundled SQLite is C. If there is no compiler (and no `sudo`), a user-level Zig works as one:

```sh
mkdir -p ~/.cache/swb-tools/bin
pip install --target ~/.cache/swb-tools/py ziglang
cat > ~/.cache/swb-tools/bin/cc <<'WRAPPER'
#!/bin/sh
# zig cc as a cc: zig spells the target x86_64-linux-gnu, rustc and cc-rs say x86_64-unknown-linux-gnu.
for a in "$@"; do
  shift
  case "$a" in --target=x86_64-unknown-linux-gnu) a=--target=x86_64-linux-gnu ;; --target=aarch64-unknown-linux-gnu) a=--target=aarch64-linux-gnu ;; esac
  set -- "$@" "$a"
done
PYTHONPATH=$HOME/.cache/swb-tools/py exec python3 -m ziglang cc "$@"
WRAPPER
chmod +x ~/.cache/swb-tools/bin/cc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=~/.cache/swb-tools/bin/cc CC=~/.cache/swb-tools/bin/cc
```

## What it does

- One `claude` process per chat, stdin kept open for several turns, `--include-partial-messages` for streaming text.
- **Permissions**: it starts `claude` with `--permission-prompt-tool stdio`, so a permission request that Claude Code decides to
  ask arrives here as a `can_use_tool` control request and waits for a click. This host approves nothing on its own: no timeout,
  no auto-allow, a closed page leaves the request pending, an answer carries the request's own `input` (never one the page
  sent), each request is answered once, a request that the turn's end or the CLI cancels is withdrawn, Esc in the dialog is a refusal.
  **Claude Code's own settings still apply**: an `allow` rule, a permission mode or a hook in the user, project or local
  settings decides before anything reaches this app, so a project whose `.claude/settings.json` allows `Bash(*)` runs commands
  without a click here, exactly as in the terminal. Open only projects you would run `claude` in.
- **Images**: paste, drop or pick (png, jpeg, gif, webp, 5 MB each, five per message) go in as base64 image blocks; images in tool
  results show in the tool card; an image file inside the project that a tool names (or a result mentions) is fetched through
  `GET /api/file`, which needs the token, resolves links and refuses anything outside the project or not an image (no SVG).
- **Interrupt** sends the CLI's `interrupt` control request; the turn ends with `error_during_execution`.
- Events the host does not know are passed to the UI as raw JSON (and shown in the debug drawer), so a new CLI version cannot break parsing.

## Search your history (step 1 of the Hermes-like features)

- **Index**: every chat's log is indexed in SQLite FTS5 (`<data>/index.sqlite`, `0600`), one row per message block: what you wrote, what
  Claude answered, a tool call's name and summary, and the first 2 KB of a tool result. Images, thinking, sub-agent messages,
  permissions, stderr and the search tool's own results are not indexed. The index is only a cache of the `.jsonl` logs: it is
  brought up to date at start, is rebuilt when it is old or damaged, and drops chats that are gone.
- **Sidebar search** (Korean and English, any part of a word; a two-letter Korean word works too): the match is marked, a click opens the chat
  and scrolls to the message.
- **`search_history` for Claude**: each chat's `claude` is started with an MCP server (`smart-workbench-app mcp`, stdio) that asks the host over
  a `0600` Unix socket. It searches this chat's **project folder** by default (`scope: "all"` for every chat), returns at most
  six short excerpts, and frames them as untrusted data quoted from the past (old tool outputs can carry instructions). The chat is
  identified by a per-process secret in a `0600` config file, never by an argument the model passes; the secret and the file go when the process ends.
  Calls go through Claude Code's normal permission flow, so the dialog asks; **"Allow this tool in this chat"** (shown for this tool only)
  adds a session allow rule that Claude Code applies until that process ends. The host never approves on its own.

## Security

Local only: binds `127.0.0.1`; every request needs the right `Host` (DNS rebinding gets 421); a WebSocket needs the exact
`Origin` and the token as its first message within 5 s; the file endpoint needs a `Bearer` token; a strict CSP (no inline script,
no remote image or script), `nosniff`, `no-referrer`. Markdown goes through DOMPurify (images and forms are removed).
The app can run commands through Claude Code, so treat its address like a terminal. The data folder (prompts, images and
tool results) is `0700` and its files `0600`. The MCP socket and its per-chat config files are `0600` too. Streaming text pieces are shown but never stored; a session read back after a
crash has its open requests cancelled. On Ctrl-C or SIGTERM the host kills every `claude` process group.

Known limits: a local program that forges the `Origin` header can open a socket and make the host buffer up to 40 MB before the
token is checked; a link swapped into the project folder between the path check and the read is not defended against
(an attacker who can write there can already do more); the event log and its base64 images are held in memory per session.

## Tests

```sh
cargo test                       # protocol, process, session, server (a fake claude replays scenarios; no model calls)
cargo clippy --all-targets -- -D warnings
node --test tests/ui.test.mjs    # the UI's pure helpers
PLAYWRIGHT=/path/to/playwright/index.mjs tests/smoke/run.sh   # headless-Chromium run of the UI against the fake
SWB_LIVE=1 cargo test --test live -- --ignored                # the real claude, two tests (costs a few cents)
```

`fake-claude` replays a scenario file: lines are printed as they are, or are directives (`{"$wait":"user"|"control_response"|"interrupt"}`,
`{"$sleep":ms}`, `{"$exit":n}`).

## Layout

`src/claude/` everything specific to the CLI (protocol, process) · `src/session.rs` one chat's task (state, pending permissions, log) ·
`src/index.rs` the FTS5 index · `src/mcp.rs` the stdio MCP server and the per-chat secrets · `src/registry.rs` sessions on disk · `src/server/` HTTP + WebSocket + auth + files + the MCP socket · `ui/` plain ES modules, `ui/vendor/` marked and DOMPurify (pinned, licenses included).

## Verified against Claude Code 2.1.316

Multi-turn over one process, `--resume <session id>` (remembers), image input, `can_use_tool` allow and deny, `interrupt`
(`control_response` then `result/error_during_execution` within 0.1 s), and the whole permission round trip through this host
with the real `claude` (the live test). Also checked: a stdio MCP server (tool name `mcp__swb_app__search_history`, env passed to it, connected before the first turn, a session allow rule through `updatedPermissions`, the CLI's `server/discover` probe answered with -32601) and the real tool call in the live test. Not yet checked: how SmartWorkbench's guard questions (`$.ui.ask`) behave in a headless
host, the IDE connection protocol, and Windows/macOS.

## Next

Memory (SQLite, saved only with a click) · skill proposals with an approval screen · a scheduler with a fixed tool list for unattended runs · per-chat model provider (`ANTHROPIC_BASE_URL`) and a container launcher · a Telegram gateway · Tauri wrap (needs `pkg-config` and `webkit2gtk-4.1` dev packages) · file tree and editor · PTY terminal.
