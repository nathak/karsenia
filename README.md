<p align="center">
  <img src="assets/icon-256.png" alt="Karsenia app icon" width="128" height="128">
</p>

# Karsenia

**English** · [한국어](#한국어)

A desktop-style host for [Claude Code](https://claude.com/claude-code). A Rust core runs the `claude` CLI headless
(`--input-format stream-json --output-format stream-json`) and gives it a chat window of its own: a native desktop app
(Slint, no browser, no network listener) or a local web page.

> Status: early (v0.1.4). The Linux test suite is large; Windows has no automated tests, only hand use. Expect rough edges.

## Download

Prebuilt Windows app: **[Releases](https://github.com/nathak/karsenia/releases/latest)** → `karsenia-desktop-0.1.4-windows-x86_64.zip`
(the 0.1.0 release was published before the rename and is named `tenrec-desktop-0.1.0-…`; check the zip against the `.sha256` file
next to it). Unzip, then double-click `karsenia-desktop.exe`. From 0.1.3 on, the zip is built by GitHub Actions from the release's tag
(`.github/workflows/release.yml`; the Linux tests run first), not on a personal machine.

- **macOS (experimental, from 0.1.4):** `karsenia-desktop-0.1.4-macos-universal.zip` (Apple silicon and Intel), built by the same workflow. It has never been run on a Mac by its author, and it is not signed by an Apple developer or notarized, so macOS asks you to allow it (the `READ-ME-FIRST.txt` in the zip says how).
- It needs `claude` (Claude Code) on your `PATH`.
- The `.exe` is **not signed**. Windows SmartScreen or a company security product may warn about an unknown program.
  `sign-windows.ps1` in the zip signs it on your own PC with a self-signed certificate; that makes the file tamper-evident,
  but it does not make a security product trust it. Do not try to get around a company's security product: ask its administrator
  to allow the file.

## What it does

- **Chats.** One `claude` process per chat with several turns, streaming text, images (paste, pick), tool cards with input and
  result, sub-agents nested under their card, thinking lines, code blocks with copy and folding, tables, cost and token counts,
  a "view as text" mode where a whole conversation can be selected and copied, interrupt, background jobs shown while a chat waits for them, and reopen or resume after a restart.
- **Permissions stay yours.** The host starts `claude` with `--permission-prompt-tool stdio`. A permission request waits for your
  click: no timeout, no auto-allow, an answer is bound to the request shown. Claude Code's own settings (allow rules, modes,
  hooks) still decide first, as in a terminal, so open only projects you would run `claude` in. Before a normal chat starts
  `claude` in a project, the host reads the project's own Claude Code settings (`.claude/settings*.json`, `.mcp.json`: hooks,
  redirecting `env`, credential helpers, wide allow rules, a never-asking mode, plug-ins, MCP servers) and, if something there can
  run commands or send your login elsewhere, the desktop app shows a card: trust it, start without the project's settings, or
  decide later (the web page shows the reason as text).
- **Commands.** The composer's "/" menu (desktop) lists the program's built-in commands (`/compact`, `/context`, `/cost`, `/usage`,
  `/model`, `/clear`) and the chat's skills and plug-in commands; a command is sent alone so Claude Code runs it itself. Between
  turns a chat changes its model in place when the program allows it, otherwise with a quick resume.
- **Accounts.** Several logins (for example personal and work) kept apart, each with its own notes, skills, history and usage
  limits; Codex logins can be managed too (chats run through Claude Code only).
- **Usage and cost.** Each account's 5-hour and weekly limits (from what a running `claude` reports, refreshed on request with the
  program's own `/usage`), and what it cost today, in 7 days and this month (estimates), with optional daily and monthly budgets
  across all accounts and a notice near them.
- **Memory, skills, search.** Notes that Claude proposes and you approve; skills with a risk report and version history;
  full-text search over all chats.
- **Scheduled tasks** (read-only file tasks or web tasks), **model providers and gateways** (Anthropic-compatible
  endpoints with your own key and request headers, optionally in a container; or a gateway that uses an account's login), chats
  in their **own git worktree**.
- **Files.** A project file tree and editor; Markdown is rendered and source files are syntax-coloured. Paths outside the
  project and dangerous paths are refused.
- **Live view.** A read-only local web page of what the chats do (prompts, tool calls with risk and guard marks, tokens), opened
  from the Team page or with `tenrec --live`. Bound to `127.0.0.1`, token in the address fragment, secrets masked.
- **Workbench and Team page.** A per-chat workbench (goal, constraints, pins, done conditions, guard rules, team mode with
  explorer and advisor sub-agents) that the app hands to the model and enforces with a guard hook. It is always on in the desktop
  app (the SmartWorkbench plug-in is switched off in its chats); the web server uses it when the setting is on, otherwise the plug-in.
- **Desktop extras.** On Windows, closing the window hides it to a tray icon. Notices (a Windows tray balloon; `notify-send` on
  Linux) when a chat waits for a permission, a long turn ends or the program stops with an error, and near a budget.
  Interface in Korean and English.

The long design notes, behaviour details and known limits are in [docs/NOTES.md](docs/NOTES.md).

## Build and run

Rust (stable) is needed; the bundled SQLite is C, so a C compiler (or a user-level Zig, see the notes) is needed as well.

```sh
cargo run                                   # web UI: prints http://127.0.0.1:PORT/#t=TOKEN, open that address
cargo run -- --port 4311 --data ./data --claude /path/to/claude
cargo run --features desktop --bin karsenia-desktop     # native desktop app (Slint)
cargo test                                  # no model calls: a fake `claude` replays scenarios
```

Windows build from Linux (needs `zig` as the linker):
`cargo build --release --target x86_64-pc-windows-gnu --features desktop --bin karsenia-desktop`
(set `CC_x86_64_pc_windows_gnu` and `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` to a `zig cc` wrapper; see the notes).

Sessions and settings are kept in `~/.local/share/tenrec` (the app was called Tenrec before; the folder, the `TENREC_*` variables, the web server program `tenrec`, the MCP name `tenrec` and the `tenrec/<id>` worktree branches keep that name so that nothing already saved is lost) (`%USERPROFILE%\.local\share\tenrec` on Windows;
`TENREC_DATA` or `--data` changes it).

## Security

Local only: the web server binds `127.0.0.1`, checks `Host` and `Origin`, and needs a token; the desktop app has no listener.
The app can run commands through Claude Code, so treat it like a terminal. This is **not a sandbox**. Open only projects you
trust: a project's own `.claude/settings.json` (hooks, `env`) applies to a normal chat once you trust it. Before a normal chat
starts, the host checks three of the project's Claude Code settings files and asks first (the project check); this lowers the risk,
it is not a sandbox (CLAUDE.md, skills and parent folders are not checked). The app does not store a claude.ai login or
subscription token itself (the `claude` program keeps its own); a gateway set to use an account's login sends that login to the
address you gave it: use it only for an address you trust. Details and known limits: [docs/NOTES.md](docs/NOTES.md#security).

## License

Source code: [MIT](LICENSE). The desktop app is built with [Slint](https://slint.dev) under its Royalty-free Desktop License
(the attribution is in Settings → About). Fonts (Pretendard, JetBrains Mono) are under the SIL Open Font License; their texts are in
`ui/vendor/`.

---

<a id="한국어"></a>

# Karsenia (한국어)

[English](#karsenia) · **한국어**

[Claude Code](https://claude.com/claude-code)를 위한 데스크톱형 호스트입니다. Rust 코어가 `claude` CLI를 헤드리스
(`--input-format stream-json --output-format stream-json`)로 실행하고, 전용 채팅 화면을 제공합니다. 화면은 네이티브
데스크톱 앱(Slint, 브라우저와 네트워크 수신 포트 없음)이거나 로컬 웹 페이지입니다.

> 상태: 초기 단계(v0.1.4). 리눅스용 테스트는 많지만 Windows는 자동 테스트 없이 손으로만 확인했습니다. 거친 부분이 있을 수 있습니다.

## 내려받기

Windows 빌드: **[Releases](https://github.com/nathak/karsenia/releases/latest)** → `karsenia-desktop-0.1.4-windows-x86_64.zip`
(0.1.0 릴리스는 이름을 바꾸기 전에 올린 것이라 `tenrec-desktop-0.1.0-…`입니다. 옆의 `.sha256` 파일로 확인하세요).
압축을 풀고 `karsenia-desktop.exe`를 실행합니다. 0.1.3부터 zip은 개인 PC가 아니라 릴리스 태그에서 GitHub Actions가 만듭니다
(`.github/workflows/release.yml`, 빌드 전에 리눅스 테스트를 먼저 돌립니다).

- **macOS(실험적, 0.1.4부터):** `karsenia-desktop-0.1.4-macos-universal.zip`(Apple 실리콘과 Intel), 같은 워크플로로 만듭니다. 만든 사람이 Mac에서 실행해 본 적이 없고, Apple 개발자 서명과 공증이 없어 macOS가 허용 여부를 묻습니다(zip 안의 `READ-ME-FIRST.txt`에 방법이 있습니다).
- `claude`(Claude Code)가 `PATH`에 있어야 합니다.
- `.exe`는 **서명되어 있지 않습니다**. Windows SmartScreen이나 회사 보안 제품이 모르는 프로그램이라고 경고할 수 있습니다.
  압축 안의 `sign-windows.ps1`은 본인 PC에서 자체 서명 인증서로 서명합니다. 변조 여부를 알 수 있게 될 뿐, 보안 제품이
  신뢰하게 만들지는 못합니다. 회사 보안 제품을 우회하려 하지 말고, 관리자에게 이 파일을 허용해 달라고 요청하세요.

## 기능

- **채팅.** 채팅마다 `claude` 프로세스 하나로 여러 턴을 이어 가며, 글자 스트리밍, 이미지(붙여넣기·선택), 입력과 결과가 보이는
  도구 카드, 카드 아래에 중첩된 서브 에이전트, 생각 줄, 복사·접기가 되는 코드 블록, 표, 비용과 토큰, 대화 전체를 선택해
  복사하는 "텍스트로 보기", 중단, 채팅이 기다리는 백그라운드 작업 표시, 재시작 뒤 다시 열기와 이어 하기를 지원합니다.
- **권한은 사용자 몫.** `--permission-prompt-tool stdio`로 `claude`를 띄웁니다. 권한 요청은 클릭을 기다리며 시간 초과도
  자동 허용도 없고, 답은 화면에 보이는 그 요청에만 묶입니다. Claude Code 자체 설정(허용 규칙, 모드, 훅)이 먼저 결정하는
  것은 터미널과 같으므로, `claude`를 실행해도 되는 프로젝트만 여세요. 일반 채팅이 프로젝트에서 `claude`를 띄우기 전에
  프로젝트 자체의 Claude Code 설정(`.claude/settings*.json`, `.mcp.json`: 훅, 접속을 돌리는 `env`, 자격 증명 명령, 넓은 허용
  규칙, 묻지 않는 모드, 플러그인, MCP 서버)을 읽습니다. 명령을 실행하거나 로그인을 다른 곳으로 보낼 수 있는 항목이 있으면
  데스크톱 앱이 카드를 띄웁니다: 신뢰, 프로젝트 설정 없이 시작, 나중에(웹 화면은 이유를 글로 보여 줍니다).
- **명령.** 입력창의 "/" 메뉴(데스크톱)에 프로그램 내장 명령(`/compact`, `/context`, `/cost`, `/usage`, `/model`, `/clear`)과
  채팅의 스킬·플러그인 명령이 나옵니다. 명령은 단독으로 보내져 Claude Code가 직접 실행합니다. 턴 사이에는 프로그램이 허용하면
  그 자리에서, 아니면 짧게 이어 시작해 모델을 바꿉니다.
- **계정.** 개인용·업무용 같은 여러 로그인을 따로 두며 계정마다 메모, 스킬, 기록, 사용 한도가 분리됩니다. Codex 로그인도
  관리할 수 있습니다(채팅은 Claude Code로만 실행).
- **사용량과 비용.** 계정별 5시간·주간 한도(실행 중인 `claude`가 알려 주는 값, 요청하면 프로그램 자체의 `/usage`로 새로 읽음)와
  오늘·7일·이번 달 비용(추정)을 보여 줍니다. 모든 계정을 합친 일·월 예산을 선택적으로 정할 수 있고, 예산에 가까워지면 알립니다.
- **메모리·스킬·검색.** Claude가 제안하고 사용자가 승인하는 메모, 위험 보고서와 버전 기록이 있는 스킬, 모든 채팅 전문 검색.
- **예약 작업**(읽기 전용 파일 작업, 웹 작업), **모델 제공자와 게이트웨이**(내 키와 요청 헤더를 쓰는 Anthropic 호환 주소, 선택적으로 컨테이너; 또는 계정 로그인을 쓰는 게이트웨이), 채팅별 **git worktree**.
- **파일.** 프로젝트 파일 트리와 편집기. Markdown은 렌더링하고 소스 파일은 구문 색을 입힙니다. 프로젝트 밖 경로와 위험한 경로는
  거부합니다.
- **라이브 보기.** 채팅이 하는 일(프롬프트, 위험·가드 표시가 붙은 도구 호출, 토큰)을 보여 주는 읽기 전용 로컬 웹 화면입니다.
  팀 화면이나 `tenrec --live`로 엽니다. `127.0.0.1`에만 열리고 토큰은 주소의 `#` 뒤에 있으며 비밀값은 가립니다.
- **워크벤치와 팀 화면.** 채팅별 워크벤치(목표, 제약, 핀, 완료 조건, 가드 규칙, explorer·advisor 서브 에이전트를 쓰는 팀 모드)를
  앱이 모델에 넘기고 가드 훅으로 지킵니다. 데스크톱 앱에서는 항상 켜져 있고(그 채팅에서는 SmartWorkbench 플러그인을 끕니다),
  웹 서버는 설정을 켰을 때 이것을, 아니면 플러그인을 씁니다.
- **데스크톱 부가 기능.** Windows에서는 창을 닫으면 트레이 아이콘으로 숨습니다. 채팅이 권한을 기다릴 때, 오래 걸린 턴이 끝났을 때,
  프로그램이 오류로 멈췄을 때, 예산에 가까울 때 알림을 띄웁니다(Windows 트레이 풍선, Linux는 `notify-send`).
  화면은 한국어와 영어를 지원합니다.

자세한 설계 메모, 동작 세부, 알려진 한계는 [docs/NOTES.md](docs/NOTES.md)(영어)에 있습니다.

## 빌드와 실행

Rust(stable)가 필요합니다. 내장 SQLite가 C라서 C 컴파일러(또는 사용자 영역 Zig, 메모 참고)도 필요합니다.

```sh
cargo run                                   # 웹 화면: http://127.0.0.1:PORT/#t=TOKEN 이 출력되면 그 주소를 엽니다
cargo run -- --port 4311 --data ./data --claude /path/to/claude
cargo run --features desktop --bin karsenia-desktop     # 네이티브 데스크톱 앱(Slint)
cargo test                                  # 모델 호출 없음: 가짜 `claude`가 시나리오를 재생
```

리눅스에서 Windows용 빌드(링커로 `zig` 필요):
`cargo build --release --target x86_64-pc-windows-gnu --features desktop --bin karsenia-desktop`
(`CC_x86_64_pc_windows_gnu`와 `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER`를 `zig cc` 래퍼로 지정, 메모 참고).

세션과 설정은 `~/.local/share/tenrec`(Windows는 `%USERPROFILE%\.local\share\tenrec`)에 저장됩니다. (예전 이름이 Tenrec이었습니다. 이미 저장된 것이 사라지지 않도록 이 폴더, `TENREC_*` 변수, 웹 서버 프로그램 `tenrec`, MCP 이름 `tenrec`, `tenrec/<id>` 작업 트리 브랜치는 그 이름을 그대로 씁니다.)
`TENREC_DATA` 또는 `--data`로 바꿀 수 있습니다.

## 보안

로컬 전용입니다. 웹 서버는 `127.0.0.1`에만 열리고 `Host`와 `Origin`을 검사하며 토큰이 필요합니다. 데스크톱 앱은 수신 포트가
없습니다. 이 앱은 Claude Code를 통해 명령을 실행할 수 있으므로 터미널처럼 다루세요. **샌드박스가 아닙니다.** 신뢰하는
프로젝트만 여세요. 신뢰한 뒤에는 일반 채팅에 프로젝트의 `.claude/settings.json`(훅, `env`)이 적용됩니다. 일반 채팅을 시작하기 전에
프로젝트의 Claude Code 설정 파일 세 개를 확인하고 먼저 묻습니다(프로젝트 확인). 위험을 줄일 뿐 샌드박스는 아닙니다(CLAUDE.md, 스킬,
상위 폴더는 확인하지 않음). 앱은 claude.ai 로그인이나 구독 토큰을 직접 저장하지 않습니다(`claude` 프로그램이 자체 보관). 계정 로그인을
쓰는 게이트웨이는 그 로그인을 입력한 주소로 보내므로 믿을 수 있는 주소에만 쓰세요. 자세한 내용과 한계: [docs/NOTES.md](docs/NOTES.md#security).

## 라이선스

소스 코드는 [MIT](LICENSE)입니다. 데스크톱 앱은 [Slint](https://slint.dev)의 Royalty-free Desktop License로 빌드하며
(고지는 설정 → 정보에 있음), 글꼴(Pretendard, JetBrains Mono)은 SIL Open Font License이고 전문은 `ui/vendor/`에 있습니다.
