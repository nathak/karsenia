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
  a `0600` Unix socket. It searches this chat's **project folder** only (there is deliberately no wider scope for Claude: a session allow rule covers the tool whatever its arguments, and searching every chat is for the person, in the sidebar), returns at most
  six short excerpts, and frames them as untrusted data quoted from the past (old tool outputs can carry instructions). The chat is
  identified by a per-process secret in a `0600` config file, never by an argument the model passes; the secret and the file go when the process ends.
  Calls go through Claude Code's normal permission flow, so the dialog asks; **"Allow this tool in this chat"** (shown for this tool only)
  adds a session allow rule that Claude Code applies until that process ends. The host never approves on its own.

## Memory (step 2 of the Hermes-like features)

Short notes the app keeps between chats, in `<data>/memory.sqlite` (`0600`; unlike the history index this is primary data and is never
rebuilt: a file that cannot be read is renamed `.broken` and you are told). A note is at most 300 characters, either about a
**project folder** or about **you**, and has a source: you, Claude (through a tool you approved), or auto-save.

- **Checks every save goes through**: size, caps (200 notes / 32 KB per project, 100 / 16 KB for you, 50 waiting; a full store refuses,
  nothing is evicted), a secret filter (keys, tokens, `password is ...`, 32+ hex digits, long mixed strings: refused, never redacted; a manual note can be
  kept after a confirmation), duplicates (the same words, and for automatic sources near-duplicates), and an order-like check
  (`always`, `never`, `ignore`, `from now on`, `run ...`, `curl`, a link, a shell operator, `항상`, `앞으로`, ...): such a note from Claude or auto-save
  waits for a click and is not given to chats until you approve it. Notes you write or edit yourself are trusted and used.
- **The Memory tab** (sidebar): waiting notes (approve / reject), this project's notes, notes about you, recent auto-saves with a link
  back to the message they came from, other projects, deleted notes (undo for 7 days), add / edit / pin / delete, export as JSON.
- **Claude's tools** (`mcp__swb_app__...`): `memory_search` (read-only, this project and your notes) and `memory_save` (at most five a
  minute). A save is **active only if you allowed that exact input in the permission dialog**; otherwise, and always for a note about you,
  it is saved as waiting. "Allow this tool in this chat" is offered for the read-only tools only, never for `memory_save`.
- **Recall**: a new (or resumed) chat is started with `--append-system-prompt-file` naming a `0600` file with up to 20 notes / 2 KB
  (pinned first, then yours, then the project's; waiting and order-like notes left out; `<` in a note is neutralised), framed as
  background facts and not instructions. The notes are never in argv. The chat shows how many notes Claude was given.
- **Auto-save** (off by default; the switch is in the Memory tab, `settings.json`): after a turn ends well, and 20 seconds of quiet (or
  three turns), a separate small `claude` run (`haiku`, `--tools ""`, no MCP, no slash commands, no settings, no session, an empty
  folder, 60 s limit) reads **only your own messages since the last run and since you switched auto-save on** (what was said while it was off is never read;
  not Claude's text, not tool results; nothing over 2000 characters, nothing with a code fence or a secret; at most the latest ten
  messages, about 8000 characters) and proposes at most three notes as strict JSON; an answer that does not parse is
  discarded whole. The host then applies the checks above, and the setting decides which scopes may be saved (this project only, or
  also about you; a note about you always waits for a click because it reaches every project). One run at a time, 40 runs a day (retries count,
  and the count is kept on disk), about $0.0005 each. A saved note appears at once with an Undo toast; progress is
  kept per chat, so a restart never reads old messages again.

## Skills (step 3 of the Hermes-like features)

A skill is a procedure Claude follows when it applies (`SKILL.md`). It is **instructions for Claude**, so the app never enables one by itself:
a proposal always waits, you read the whole file, and only then it is approved. (That is a promise of this app, not of the file system: a chat
with broad Bash permission can write the data folder, and a skill planted there would pass only the block checks when a chat starts.)

- **Proposals**: the **Skills** tab has "Propose a skill from this chat" (with an optional hint, a scope, and "improve skill X"), and an
  optional **automatic** mode (off by default): after a turn that used 8 or more tools and ended well, 90 seconds of quiet, at most one per
  chat a day and five a day in all (counted on disk), and **never** in a chat that used, at any time, a web tool, an outside MCP tool, a
  subagent (`Task`/`Agent`) or a shell command that fetches (`curl`, `wget`, a URL): third-party text may have shaped it. An automatic proposal only creates new skills, never changes one.
- **What the proposer reads**: a separate `claude -p --model sonnet` run (no tools, no MCP, no slash commands, no settings, no session, an
  empty folder, 120 s) reads **your own messages** and **one-line summaries of the tool calls that succeeded** (never tool results, never
  Claude's own text, nothing with a secret), plus your hint, plus the names of existing skills (or, when improving, the current text). It
  answers in strict JSON; anything else discards the run. About $0.03-0.06 a run.
- **Checks the host does itself**, on the text as it will be written and again at every approval (the report is shown next to the full text):
  *blocks* (Approve is not offered until fixed): a bad or reserved name, a description that is not one line without `---`/`<`/`>`, a body
  that starts with `---`, a `` !`command` `` or a ```` ```! ```` block that would run when the skill loads, a secret, control, invisible or direction-changing
  characters (also in the name and the description, which every chat loads), a line over 1000 characters, a body over 7 KB or 200 lines. *Warnings* (you must tick "I read the warnings"): `curl | sh`,
  `sudo`, `rm -rf`, `eval`, `$(...)`, `base64 -d`, ssh/scp/nc, URLs, "ignore the rules / without asking / silently", permission words
  (`settings.json`, `allowed-tools`), credential paths, HTML or comments, long lines. Editing then approving goes through the same checks.
- **What is written**: the host builds `SKILL.md` itself: the frontmatter holds `name` and `description` and **nothing else** (no
  `allowed-tools`, hooks, model, context), whatever was proposed. Up to 30 skills, 20 waiting, the newest 5 versions kept.
- **Improvements** are proposals against a skill (a diff against the active version is shown); approving makes a new version, a stale one is
  refused, and rollback copies an old version forward, so the history only grows.
- **How Claude gets them**: each chat is started with `--plugin-dir` naming a one-off folder (`0700`/`0600`, plugin name `swb`, so the
  skills are `swb:<name>`) with the enabled skills of the user and of that project. It is rebuilt at every start and removed when the
  process ends, so a skill turned off or deleted is simply not in the next chat, and a running chat is never changed. The chat shows which
  skills it was given, and the tab shows when each was last used.

## Scheduled tasks (step 4 of the Hermes-like features)

The Schedule tab holds tasks you write: an instruction, a folder or the web, and a time. While the app is open and the switch is on,
the host looks at the wall clock every 30 seconds and runs what is due with one `claude -p`, **one run at a time**, with nobody to
click a permission prompt. So a run is given nothing to be asked about:

- **Two profiles that never mix.** *Files*: `Read`, `Grep`, `Glob` inside one project folder. *Web*: `WebFetch` and `WebSearch`
  only, in an empty folder of the app's own. A run that could read files and also fetch could carry a file out in an address.
  Writing, editing and running commands do not exist in a scheduled run (`--tools` names exactly the profile's tools; a
  `--disallowedTools` list backs it up).
- **No allow rule for the file tools.** `--allowedTools Read` would cover every path; without it Claude Code reads inside the
  project folder and refuses a path outside it and a link that leads outside it (checked with the real `claude`, see the live
  test). The refused requests are kept with the run. The web profile does have an allow rule, since nobody can answer for each address.
- **Nothing of the project's own is loaded:** `--setting-sources ""` (so a hook in a project you do not trust cannot run
  unattended, and your own user settings do not widen the run), `--strict-mcp-config` with no config, no plugins, no skills or
  slash commands, no session saved, no `--resume`. This is a deliberate exception to "Claude Code's own settings decide first".
- **Limits per run:** the model you chose, at most 50 turns, `--max-budget-usd` (default $0.50, at most $5), a time limit
  (default 10 minutes, at most 30; the whole process group is killed). Per day: 20 runs including "run now" (counted on disk, so a
  restart does not reset it). An interval is at least 15 minutes. Three failures in a row pause a task, and a change or a
  switch-on starts it fresh. A folder inside the app's data folder, or one that holds it, is refused when the task is made and again when it runs.
- **Time:** daily at a time of day on chosen weekdays, or every N minutes, in the machine's time zone (`localtime_r`/`mktime`; one run
  on the day clocks go forward, one at the first of a repeated wall-clock time when they go back). Times that pass while the app is
  off or the machine asleep are not run: one "missed" note is made (more than 10 minutes late) and the next time is counted from
  now. A task still running when its next time comes is skipped; a run that waited 15 minutes behind another is skipped.
- **Only you make tasks.** The only way to create or change one is the Schedule tab (`sched_*` on the WebSocket, which needs the
  token); there is no MCP tool for it, so a chat, or anything Claude read, cannot schedule a run of its own. Switching the whole
  thing on is yours too, and starts with every task's time counted from that moment.
- **A result is only shown.** It is stored in `schedule.sqlite` (0600, 64 KB each, the last 50 runs of a task), shown with the
  same Markdown sanitising as a chat (with a reminder that a web result may carry outside text), and carries the definition it ran
  with. Nothing reads it and acts: it does not reach memory, skill proposals or the search index. A toast and an unread count tell you it finished.
- **Web runs and the machine's own addresses.** A fetch goes out from your computer, so `localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0` and the cloud
  metadata addresses are refused by name (checked with the real `claude`). Other addresses of your network are **not** blocked, since the rule
  matches names, not address ranges: give a web task only to pages you would open yourself. Links in a result are shown as text with their
  whole address and cannot be clicked.
- **What stays outside the app's control:** a managed (organisation) policy of Claude Code applies to a scheduled run whatever the flags
  say. A project's own settings, hooks, allow rules and `CLAUDE.md` do not (live test with a hostile project folder), and the CLI's
  auto-memory is switched off for the run.
- **Stop:** each running task has a Stop button, which ends the run and everything it started and takes a waiting run out of the line; a stop is
  not counted as a failure. "Run now" works whatever the switch says, because you pressed it.
- **Switching off:** a run waiting in line that the clock queued is looked at again when its turn comes, and is skipped if the task or the whole scheduler was switched off meanwhile ("run now" is yours and still runs). A run already going is not stopped by switching off (press Stop); it ends by its own time limit. Task numbers are never reused, so a queued or running run cannot reach a task made later.
- **Not a daemon:** nothing runs while the app is closed. A run that was going when the app ended is marked failed at the next start.

## Model providers (step 5 of the Hermes-like features)

A chat can be made against another Anthropic-compatible address (a gateway, a company proxy, a local server) instead of your Claude
account. You add it in the **Model providers** box of the Chats tab (name, address, how the key is sent, the key, optional model names)
and pick it in the new-chat form. The app still never calls any API itself: the chat's `claude` process is simply started with that
address, and a chat keeps its provider for its whole life (it cannot be changed; make a new chat).

**What was measured, and why the setup is what it is.** With only `ANTHROPIC_BASE_URL` changed, a `claude` that is logged in to
an account sends the account's own OAuth token to that address (tested against a local capture server). So a provider chat:

- always has a token of its own set (your key; a placeholder for an address that needs none), so the account's token is never used;
- runs in a settings folder of its own (`<data>/chatcfg/<chat id>`, `0700`), so the account's credentials file is out of the CLI's reach;
- loses every inherited variable that could carry or redirect a credential (`ANTHROPIC_*`, `CLAUDE_CODE_USE_*`, `CLAUDE_CODE_OAUTH*`, `AWS_BEARER_TOKEN_BEDROCK`, `CLAUDE_CONFIG_DIR`, …; prefixes, so a variable a later version adds is covered);
- does not read the project's `.claude/settings.json` (`--setting-sources user`) and has its address pinned on the command line. Measured: a project's `env.ANTHROPIC_BASE_URL` replaces the one the host sets, and the key would follow it; a project hook ran next to the key. A normal chat has that same exposure (see Known limits).

Each of these is held by a test; the live test (`SWB_LIVE=1`) starts the real `claude` in a hostile project folder (its settings redirect
the address, bring a token and a key helper, and run a hook) and checks that the provider's address gets exactly its own key, a second
server gets nothing, and the hook does not run.

**The provider sees the whole chat**, including files Claude read and command output. The chat shows a banner and the list shows
`→ host` so it is never a surprise. Because it is a third party, the account's side of the app is cut off in **both** directions:
a provider chat gets no notes, no skills and no history-search tool, is not read by auto-save or by skill proposals (their runs go to your
account), and is not put into the history index (so it is not found by search, and the account's chats cannot pull its text across).
Scheduled tasks always use your account.

**The key.** Stored in `<data>/providers.json` (`0600`). The page is never sent it again (`has_token` and the host only), it is passed to
the process in the environment (never in the arguments, which any user can read), and anything of the chat that would be shown or stored
is scrubbed of it (`[provider key]`; it catches the key as one piece of text, not an encoded or split one; a key must be at least 8 characters). Providers are made and
changed only by you in the page: there is no MCP tool for it. An `https://` address is required, except for this computer (`localhost`,
`127.0.0.1`, `[::1]`), no user name, query or fragment. If a chat's provider is deleted, or its address (host) is changed, the chat does
not start again (it was made for another party): make a new chat. A changed key or path on the same host is used at the next start.

Limits worth knowing: tools that run on this computer (a command, a file read) run as you, so one you approve can read
`providers.json` or the account's credentials file, and a provider chat's permission prompts are the only guard (read them; the container option below narrows what a command can reach). The chat's `claude` and the commands it runs inherit the key in their environment. Your own plugins, hooks and
permission rules in `~/.claude` do not apply in a provider chat (its settings folder is empty), nor do the project's settings. Claude Code's
background model (haiku) goes to the same address: if the address names models differently, fill in the model names. Managed
organisation policy applies whatever the flags say. The transcripts Claude Code keeps for a provider chat are in its own settings
folder under `chatcfg/` and are not removed when the provider is.

### Running a provider chat in a container (experimental)

In the Model providers box, give the absolute path of `docker` or `podman` and the name of an image that contains `claude` (pulled by you
beforehand: the app passes `--pull=never`, so a mistyped name fails instead of fetching another image). A new chat on a provider then
offers "Run inside a container". **This part has not been run against a real docker or podman**: it was written from their documented
options and tested with a stand-in runtime that records what it was asked and starts the fake `claude` (`tests/container.rs`), so it
checks what the app asks for and how it cleans up, not what the runtime does with it. Try it on something unimportant first.

What the app asks for: `run --rm -i --init --pull=never --cap-drop=ALL --security-opt=no-new-privileges --read-only`, a small `/tmp`
tmpfs, `--pids-limit=512 --memory=4g`, your user id (`--userns=keep-id` for podman), exactly two mounts (the project folder at its own path,
read-write, and the chat's own settings folder), no host network, and the provider's key by name only (`-e NAME`, the value comes from the
runtime client's environment, so it is not in any argument). The home folder, the data folder and the account's credentials are not mounted.
The container has a name and a label of this data folder: stop, kill and process exit remove it by name (`rm -f`), because killing the
`docker run` client does not stop the container, and the next start of the app removes any left by a crash (never one of its own run: every name carries a tag of the run). Only one app should use a data folder at a time: a second one started on the same folder takes the first one's running containers for leftovers and removes them (there is no lock on the folder). Stop and exit start the removal and do not wait for it, so a container can outlive an app that is killed hard until the next start sweeps it (with the client gone, its stdin closes, which should end `claude`; that is not checked with a real runtime).

Limits: only provider chats (an account chat would need the account's credentials inside, where a command could read them); the project
folder is mounted read-write, so the container does not protect it, and **what a command writes there outlives the container**: a `.claude/settings.json` with an `env` or a hook, a `.git/hook`, an `.envrc` written by an approved command is read later by an account chat opened in the same folder, and an `env` there sends the account's token to another address (measured). The same holds for a provider chat outside a container once you approve a write. Read what a command writes into the project before you open an account chat there; the network is open (the provider, the web, your LAN); an address on
this computer (`localhost`) is refused because a container's `localhost` is not this computer's; no notes, skills or history search
(as for any provider chat); a path with `:` `,` or a backslash cannot be mounted; a project folder that is, holds, or lies inside the data folder, or that is the home folder, holds it, or lies in `~/.claude`, `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config`, `~/.kube`, `~/.docker` or `~/.local` is refused (the account's credentials and the app's key file are there), and so is `/tmp`, `/var/tmp`, `/dev/shm`, `/run`, the user's runtime folder, any folder that holds one of them, and any folder inside `/run` or the runtime folder (programs keep sockets there, tmux, ssh-agent, gpg-agent and the runtime's own, which a container running as you could drive); the folder is looked at again at every start and a link put in its place stops the chat; a container chat is refused when the home folder is not known; whoever can run `docker inspect` (the docker group is
root-equivalent) sees the key in the container's environment; the settings are checked again at every start, so a changed one stops the chat
instead of quietly running it outside a container.

## Security

Local only: binds `127.0.0.1`; every request needs the right `Host` (DNS rebinding gets 421); a WebSocket needs the exact
`Origin` and the token as its first message within 5 s; the file endpoint needs a `Bearer` token; a strict CSP (no inline script,
no remote image or script), `nosniff`, `no-referrer`. Markdown goes through DOMPurify (images and forms are removed).
The app can run commands through Claude Code, so treat its address like a terminal. The data folder (prompts, images and
tool results) is `0700` and its files `0600`. The MCP socket and its per-chat config files are `0600` too. Streaming text pieces are shown but never stored; a session read back after a
crash has its open requests cancelled. On Ctrl-C or SIGTERM the host kills every `claude` process group.

Known limits: a skill's text is read by you, and the risk report is a pattern list, not a judge (a roundabout instruction passes it, so your reading is the last line of defence); a chat with Bash permission can write to the data folder, snapshots included, and so could plant a scheduled task in `schedule.sqlite` (each run shows the definition it used; a task is limited to read-only tools or the web, whoever wrote it); auto-save and a note you approved are trusted as your words: a message you paste from elsewhere can still say something you did not mean, so read the Memory tab now and then; the order-like check and the secret filter are pattern lists, not judges (a key split by spaces or lines is not caught); the project scope is a guarantee of this tool only (a tool that can read files, such as Read or Bash through Claude Code's own permission flow, can read the data folder, and a secret read that way ends with its process); a local program that forges the `Origin` header can open a socket and make the host buffer up to 40 MB before the
token is checked; a scheduled folder run reads whatever the project folder holds (a `.env` there can end up in a result, as in a chat), and a web result is outside text that only your reading screens; a normal chat is not protected from its project's own `.claude/settings.json` either: its `env` can send the account's token to another address and its hooks run, so open only projects you trust (a provider chat skips project settings for this reason); a link swapped into the project folder between the path check and the read is not defended against
(an attacker who can write there can already do more); the event log and its base64 images are held in memory per session.

## Tests

```sh
cargo test                       # protocol, process, session, server (a fake claude replays scenarios; no model calls)
cargo clippy --all-targets -- -D warnings
node --test tests/ui.test.mjs    # the UI's pure helpers
PLAYWRIGHT=/path/to/playwright/index.mjs tests/smoke/run.sh   # headless-Chromium run of the UI against the fake
SWB_LIVE=1 cargo test --test live -- --ignored                # the real claude, eight tests (costs a few cents)
```

`fake-claude` replays a scenario file: lines are printed as they are, or are directives (`{"$wait":"user"|"control_response"|"interrupt"}`,
`{"$sleep":ms}`, `{"$exit":n}`).

## Layout

`src/claude/` everything specific to the CLI (protocol, process) · `src/session.rs` one chat's task (state, pending permissions, log) ·
`src/index.rs` the FTS5 index · `src/memory.rs` the notes, their checks and the recall block · `src/extract.rs` auto-save · `src/settings.rs` · `src/providers.rs` providers and their checks · `src/claude/env.rs` the environment of a provider chat · `src/claude/container.rs` the container arguments · `src/schedule.rs` times · `src/sched_store.rs` tasks and runs · `src/scheduler.rs` the clock loop and one run · `src/skills.rs` skills, their risk report and the one-off folder · `src/propose.rs` proposals · `src/mcp.rs` the stdio MCP server and the per-chat secrets · `src/registry.rs` sessions on disk · `src/server/` HTTP + WebSocket + auth + files + the MCP socket · `ui/` plain ES modules, `ui/vendor/` marked and DOMPurify (pinned, licenses included).

## Verified against Claude Code 2.1.316

Multi-turn over one process, `--resume <session id>` (remembers), image input, `can_use_tool` allow and deny, `interrupt`
(`control_response` then `result/error_during_execution` within 0.1 s), and the whole permission round trip through this host
with the real `claude` (the live test). Also checked: a stdio MCP server (tool name `mcp__swb_app__search_history`, env passed to it, connected before the first turn, a session allow rule through `updatedPermissions`, the CLI's `server/discover` probe answered with -32601) and the real tool call in the live test. Not yet checked: how SmartWorkbench's guard questions (`$.ui.ask`) behave in a headless
host, the IDE connection protocol, and Windows/macOS.

## Next

a Telegram gateway · Tauri wrap (needs `pkg-config` and `webkit2gtk-4.1` dev packages) · file tree and editor · PTY terminal.
