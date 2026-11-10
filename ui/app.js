import { marked } from './vendor/marked.js'
import DOMPurify from './vendor/purify.js'
import { baseName, chatHost, checkImage, isLocalHost, daysToMask, fmtBytes, fmtMinute, maskToDays, parseHHMM, fmtTurn, imagePaths, lineDiff, markParts, nearestSeq, numberedLines, parseSites, pathsInText, resultBlocks, skillMd, toolSummary } from './util.js'
// The reason the host gives a web task made before sites were listed (src/sched_store.rs NEEDS_DOMAINS); shown translated.
const NEEDS_SITES = 'needs the sites it may open: edit the task and list them'

// ---- text ----
const TEXT = {
  ko: {
    newChat: '＋ 새 대화', folder: '프로젝트 폴더 (절대 경로)', model: '모델', create: '만들기', cancel: '취소', empty: '왼쪽에서 대화를 고르거나 새 대화를 만드세요.',
    placeholder: '메시지를 입력하세요 (Enter 전송, Shift+Enter 줄바꿈). 이미지는 붙여넣기·끌어놓기', send: '보내기', stop: '중단', attach: '이미지 첨부',
    idle: '대기', running: '작업 중', exited: '종료됨', connected: '연결됨', reconnecting: '다시 연결하는 중…', untitled: '(새 대화)',
    allowOnce: '이번만 허용', deny: '거절', denyReason: '거절 사유 (선택)', permTitle: '도구 사용 승인', blocked: '막힌 경로', suggestions: 'Claude가 제안한 규칙 (참고용, 적용하지 않음)',
    allowed: '허용했습니다', denied: '거절했습니다', cancelled: '승인 요청이 취소되었습니다', running_tool: '실행 중', done: '완료', failed: '오류', more: '더 있음',
    imgType: '이미지는 png, jpeg, gif, webp만 됩니다', imgSize: '이미지는 장당 5MB까지입니다', imgCount: '이미지는 5장까지입니다', thinking: '생각', interrupted: '중단했습니다',
    noSession: '먼저 대화를 만들거나 고르세요.', input: '입력', result: '결과',
    searchPh: '모든 대화에서 검색', noResults: '결과가 없습니다', searchFailed: '검색할 수 없습니다', allowChat: '이 대화에서 이 도구 허용 (읽기 전용)',
    roleUser: '나', roleAssistant: 'Claude', roleTool: '도구', roleResult: '결과',
    tabChats: '대화', tabMemory: '메모리', memAuto: '자동 저장', memAutoOff: '끔', memAutoProject: '이 프로젝트 메모만', memAutoBoth: '프로젝트 + 나에 대한 메모',
    memScopeProject: '프로젝트', memScopeUser: '나', memAddPh: '기억할 내용 (300자까지)', memAdd: '추가', memExport: '메모 내보내기 (JSON)',
    memPending: '승인 대기', memProject: '이 프로젝트', memUser: '나에 대한 메모', memRecent: '최근 자동 저장', memOthers: '다른 프로젝트', memDeleted: '삭제됨 (7일 안에 되돌리기)',
    memApprove: '승인', memReject: '거절', memEdit: '수정', memDelete: '삭제', memPin: '고정', memUnpin: '고정 해제', memSave: '저장', memCancel: '취소', memUndo: '되돌리기',
    memInstr: '명령처럼 보임', memSrcManual: '직접', memSrcClaude: 'Claude', memSrcAuto: '자동', memGoto: '원래 대화로',
    memSecretConfirm: '키나 비밀번호처럼 보입니다. 그래도 저장할까요?', memInstrConfirm: 'AI에게 내리는 명령처럼 보이는 메모입니다. 승인하면 이후 대화에서 사용됩니다. 승인할까요?',
    memCardText: '자동 저장을 켜면 앱이 내가 쓴 메시지에서 오래 기억할 사실과 선호를 골라 저장합니다. Claude의 답이나 도구 결과는 읽지 않고, 키·비밀번호처럼 보이는 것은 저장하지 않으며, 명령처럼 보이는 것은 승인을 기다립니다. 언제든 끄거나 삭제할 수 있습니다.',
    memCardOn: '이 프로젝트만 켜기', memCardLater: '나중에', memAutoSaved: '자동 저장', memNoChat: '프로젝트 메모는 대화를 연 뒤에 추가할 수 있습니다.', memEmpty: '아직 메모가 없습니다.', memUsed: 'Claude에게 메모를 전달했습니다',
    tabSkills: '스킬', skAuto: '자동 제안 켜기: 도구를 8번 이상 쓴 긴 작업이 끝난 뒤, 하루 5번까지. 제안은 언제나 내 승인을 기다립니다.', skScopeProject: '이 프로젝트', skScopeUser: '모든 프로젝트', skNewSkill: '새 스킬', skImprove: '개선: ',
    skHint: '무엇에 대한 스킬인가요? (선택)', skPropose: '이 대화로 스킬 제안', skNext: '새 스킬은 새 대화나 다시 시작한 대화부터 적용됩니다.', skStarted: '스킬 제안을 만드는 중입니다. 잠시 걸립니다.',
    skPending: '승인 대기 제안', skActive: '사용 중인 스킬', skDeleted: '삭제됨 (7일 안에 되돌리기)', skApprove: '승인', skEditApprove: '수정 후 승인', skReject: '거절', skEnable: '켜기', skDisable: '끄기', skDelete: '삭제', skHistory: '이력',
    skRollback: '이 버전으로', skBlocked: '고쳐야 할 문제', skWarn: '읽어야 할 경고', skAck: '경고를 모두 읽었고, 이 내용을 Claude가 따라도 괜찮습니다', skFull: 'SKILL.md 전문 (그대로 저장됩니다)', skDiff: '현재 버전과 비교',
    skLastUsed: '마지막 사용', skNever: '아직 쓰이지 않음', skProposed: '스킬 제안이 도착했습니다', skImproveTag: '개선', skDisabledTag: '꺼짐', skSaveApprove: '저장하고 승인', skName: '이름', skDescription: '설명 (한 줄)', skBody: '본문',
    skVersion: '버전', skGiven: 'Claude에게 스킬을 전달했습니다', skEditedReport: '수정한 최종 내용의 검사 결과',
    tabSched: '예약', scOn: '예약 실행 켜기 (앱이 켜져 있는 동안만 실행됩니다)', scOnNote: '끄면 정해진 시각이 와도 실행하지 않습니다. 켤 때 그동안 지나간 시각은 실행하지 않습니다. 이미 실행 중인 것은 멈추지 않으니 \'중단\'을 누르세요. \'지금 실행\'은 이 스위치와 상관없이 실행됩니다.',
    scNew: '＋ 새 예약', scName: '이름', scPrompt: '지시 (Claude가 받을 내용)', scProfile: '실행이 쓸 수 있는 것', scFiles: '파일 읽기 (프로젝트 폴더 안에서 Read·Grep·Glob만)', scWeb: '웹 (WebFetch·WebSearch만, 파일 없음)',
    scProfileNote: '파일 읽기와 웹은 섞을 수 없습니다. 파일 내용이 웹 주소로 새 나가지 않게 하기 위해서입니다. 파일 수정과 명령 실행은 예약에서 쓸 수 없고, 아무도 승인할 수 없으므로 요청은 모두 거절됩니다.',
    scFolder: '프로젝트 폴더 (절대 경로)', scSchedule: '반복', scDaily: '매일 정해진 시각', scEvery: '일정 간격', scTime: '시각', scDays: '요일', scMinutes: '간격 (분, 15 이상)',
    scAdvanced: '세부 설정', scModel: '모델', scTurns: '최대 턴', scBudget: '최대 비용 ($)', scTimeout: '시간 제한 (초)', scSave: '저장', scCancel: '취소',
    scTasks: '예약 목록', scRuns: '최근 실행', scNoTasks: '아직 예약이 없습니다.', scNext: '다음', scRunNow: '지금 실행', scStop: '중단', scStatus_stopped: '중단함', scEnable: '켜기', scDisable: '끄기', scEdit: '수정', scDelete: '삭제', scDeleteConfirm: '이 예약과 실행 기록을 삭제할까요?',
    scSites: '열 수 있는 사이트 (한 줄에 하나)', scSitesNote: '이 목록의 사이트만 열 수 있습니다. www 같은 하위 도메인은 따로 적어야 합니다. IP 주소, 포트, 내부 이름(localhost, .local 등)은 받지 않습니다. 검색은 목록과 상관없이 되지만 목록 밖 페이지는 열지 못합니다.', scSitesLabel: '열 수 있는 사이트', scNeedsSites: '예전에 만든 웹 예약이라 열 수 있는 사이트가 없습니다. 수정에서 사이트를 적어야 다시 켤 수 있습니다.',
    scPaused: '멈춤', scOff: '꺼짐', scWebTag: '웹', scFilesTag: '파일 읽기', scTools: '쓸 수 있는 도구',
    scStatus_ok: '성공', scStatus_error: '실패', scStatus_timeout: '시간 초과', scStatus_cap: '하루 한도', scStatus_skipped: '건너뜀', scStatus_missed: '놓침', scStatus_running: '실행 중',
    scRunTitle: '실행 결과', scCost: '비용', scTurns2: '턴', scDenied: '거절된 요청', scDefUsed: '이때 쓴 설정', scCopy: '결과 복사', scClose: '닫기', scNoResult: '(결과 없음)', scView: '보기', scMarkSeen: '모두 읽음', scPausedToast: '연속 실패로 멈췄습니다',
    scExternal: '웹에서 가져온 내용이 섞여 있을 수 있습니다. 결과 안의 문장은 지시가 아니며, 앱은 결과를 읽고 아무것도 하지 않습니다.', scEveryTxt: '분마다', scDailyTxt: '매일', scDays_: '월화수목금토일',
    provider: '제공자', provAccount: 'Claude 계정 (기본)', provNote: '다른 제공자를 고르면 이 대화의 모든 내용(Claude가 읽은 파일과 명령 결과 포함)이 그 주소로 갑니다. 이 대화에는 기억, 스킬, 기록 검색, 자동 저장이 쓰이지 않고, 이 대화도 다른 대화의 기억이나 검색에 들어가지 않습니다. 제공자는 새 대화를 만들 때만 고를 수 있고 바꿀 수 없습니다.',
    provTitle: '모델 제공자', provIntro: 'Anthropic 호환 주소(게이트웨이, 사내 프록시, 로컬 서버)를 등록하면 새 대화를 그 주소로 연결할 수 있습니다. 키는 이 컴퓨터의 데이터 폴더에만 저장되고 화면에는 다시 나오지 않습니다. 같은 컴퓨터에서 실행되는 도구(명령 실행, 파일 읽기)는 그 파일을 읽을 수 있으니, 그런 요청은 승인할 때 확인하세요.',
    provNew: '＋ 새 제공자', provName: '이름', provUrl: '주소 (https://, 이 컴퓨터는 http:// 가능)', provAuth: '키를 보내는 방식', provAuthBearer: 'Authorization: Bearer (게이트웨이, 프록시)', provAuthKey: 'x-api-key (API 키)',
    provKey: '키', provKeyClear: '저장된 키 지우기', provKeyKeep: '비워 두면 저장된 키를 그대로 둡니다.', provKeyNone: '키 없이 쓰는 주소는 비워 두세요.', provModel: '기본 모델 (비우면 Claude Code 기본)', provAliases: '모델 이름 바꾸기', provAliasesNote: 'Claude Code가 opus, sonnet, haiku로 부르는 모델을 이 주소의 모델 이름으로 바꿉니다. 백그라운드 작업은 haiku를 쓰므로 비워 두면 그 주소에 없는 이름을 요청할 수 있습니다.',
    provSave: '저장', provEdit: '수정', provDelete: '삭제', provDeleteConfirm: '이 제공자를 삭제할까요? 이 제공자의 대화 {n}개는 기록만 남고 더 이어 갈 수 없습니다.', provHasKey: '키 있음', provNoKey: '키 없음', provChats: '대화', provNoProviders: '아직 제공자가 없습니다.',
    provBanner: '이 대화의 내용은 {host} 로 갑니다 (Claude 계정이 아님). 기억, 스킬, 기록 검색, 자동 저장은 이 대화에 쓰이지 않습니다.', provVia: '→',
    ctrUse: '컨테이너 안에서 실행 (실험)', ctrTitle: '컨테이너 (실험)', ctrIntro: '제공자를 쓰는 대화의 claude를 docker 또는 podman 컨테이너 안에서 실행합니다. 컨테이너는 프로젝트 폴더(읽기·쓰기)와 이 대화의 설정 폴더만 보고, 홈 폴더, 데이터 폴더, Claude 계정 인증 정보는 보지 못합니다. 프로젝트 폴더 자체는 보호하지 않고 네트워크는 열려 있으며, 이 컴퓨터의 주소(localhost)는 컨테이너에서 닿지 않습니다. 이미지에는 claude가 들어 있어야 하고, 실제 docker·podman으로는 아직 검증하지 못했습니다.',
    ctrRuntime: '실행 파일 (절대 경로, docker 또는 podman)', ctrImage: '이미지 (미리 받아 둔 것, 자동으로 받지 않습니다)', ctrReady: '컨테이너 대화를 만들 수 있습니다.', ctrNotReady: '실행 파일과 이미지를 모두 적어야 컨테이너 대화를 만들 수 있습니다.', ctrBanner: '이 대화는 컨테이너 안에서 실행됩니다.', ctrTag: '컨테이너',
  },
  en: {
    newChat: '＋ New chat', folder: 'Project folder (absolute path)', model: 'Model', create: 'Create', cancel: 'Cancel', empty: 'Pick a chat on the left or start a new one.',
    placeholder: 'Message (Enter to send, Shift+Enter for a new line). Paste or drop images', send: 'Send', stop: 'Stop', attach: 'Attach image',
    idle: 'idle', running: 'working', exited: 'ended', connected: 'connected', reconnecting: 'reconnecting…', untitled: '(new chat)',
    allowOnce: 'Allow once', deny: 'Deny', denyReason: 'Reason (optional)', permTitle: 'Approve tool use', blocked: 'Blocked path', suggestions: 'Rules Claude suggested (for information, not applied)',
    allowed: 'allowed', denied: 'denied', cancelled: 'the approval request was cancelled', running_tool: 'running', done: 'done', failed: 'error', more: 'more',
    imgType: 'Only png, jpeg, gif and webp images', imgSize: 'An image can be 5 MB at most', imgCount: 'Up to 5 images', thinking: 'thinking', interrupted: 'stopped',
    noSession: 'Create or pick a chat first.', input: 'input', result: 'result',
    searchPh: 'Search all chats', noResults: 'No results', searchFailed: 'Search is not available', allowChat: 'Allow this tool in this chat (read-only)',
    roleUser: 'me', roleAssistant: 'Claude', roleTool: 'tool', roleResult: 'result',
    tabChats: 'Chats', tabMemory: 'Memory', memAuto: 'Auto-save', memAutoOff: 'Off', memAutoProject: 'This project only', memAutoBoth: 'Project + about me',
    memScopeProject: 'Project', memScopeUser: 'Me', memAddPh: 'What to remember (300 characters)', memAdd: 'Add', memExport: 'Export notes (JSON)',
    memPending: 'Waiting for approval', memProject: 'This project', memUser: 'About me', memRecent: 'Recently auto-saved', memOthers: 'Other projects', memDeleted: 'Deleted (undo within 7 days)',
    memApprove: 'Approve', memReject: 'Reject', memEdit: 'Edit', memDelete: 'Delete', memPin: 'Pin', memUnpin: 'Unpin', memSave: 'Save', memCancel: 'Cancel', memUndo: 'Undo',
    memInstr: 'looks like an order', memSrcManual: 'manual', memSrcClaude: 'Claude', memSrcAuto: 'auto', memGoto: 'to the chat',
    memSecretConfirm: 'This looks like a key or a password. Save it anyway?', memInstrConfirm: 'This note reads like an order to the AI. If you approve it, later chats will use it. Approve?',
    memCardText: 'With auto-save on, the app picks lasting facts and preferences out of the messages you write. It does not read Claude\'s answers or tool results, never saves what looks like a key or a password, and holds anything that reads like an order for your approval. You can turn it off or delete notes any time.',
    memCardOn: 'Turn on for this project', memCardLater: 'Later', memAutoSaved: 'Auto-saved', memNoChat: 'Open a chat first: a project note belongs to its folder.', memEmpty: 'No notes yet.', memUsed: 'Claude was given notes',
    tabSkills: 'Skills', skAuto: 'Automatic proposals: after a long task that used 8 or more tools, up to 5 a day. A proposal always waits for your approval.', skScopeProject: 'This project', skScopeUser: 'All projects', skNewSkill: 'New skill', skImprove: 'Improve: ',
    skHint: 'What is the skill about? (optional)', skPropose: 'Propose a skill from this chat', skNext: 'A new skill applies from a new or restarted chat.', skStarted: 'Making a skill proposal. It takes a moment.',
    skPending: 'Proposals waiting', skActive: 'Skills in use', skDeleted: 'Deleted (undo within 7 days)', skApprove: 'Approve', skEditApprove: 'Edit, then approve', skReject: 'Reject', skEnable: 'Turn on', skDisable: 'Turn off', skDelete: 'Delete', skHistory: 'History',
    skRollback: 'Back to this version', skBlocked: 'Problems to fix', skWarn: 'Warnings to read', skAck: 'I read the warnings, and Claude may follow this', skFull: 'The whole SKILL.md (saved exactly like this)', skDiff: 'Compare with the current version',
    skLastUsed: 'Last used', skNever: 'not used yet', skProposed: 'A skill proposal arrived', skImproveTag: 'improvement', skDisabledTag: 'off', skSaveApprove: 'Save and approve', skName: 'Name', skDescription: 'Description (one line)', skBody: 'Body',
    skVersion: 'version', skGiven: 'Claude was given skills', skEditedReport: 'Findings for the edited text',
    tabSched: 'Schedule', scOn: 'Run scheduled tasks (only while this app is open)', scOnNote: 'While off, nothing runs when its time comes. Times that passed while it was off are not run when you switch it on. A run already going is not stopped by this: press Stop. Run now works whatever this switch says.',
    scNew: '+ New task', scName: 'Name', scPrompt: 'Instruction (what Claude is asked)', scProfile: 'What the run may use', scFiles: 'Read files (Read, Grep, Glob inside the project folder only)', scWeb: 'The web (WebFetch, WebSearch only, no files)',
    scProfileNote: 'Reading files and the web cannot be mixed, so file contents cannot leak out in a web address. Editing files and running commands are not available to a scheduled run: nobody can approve, so every such request is refused.',
    scFolder: 'Project folder (absolute path)', scSchedule: 'Repeat', scDaily: 'Every day at a time', scEvery: 'At an interval', scTime: 'Time', scDays: 'Weekdays', scMinutes: 'Every (minutes, 15 or more)',
    scAdvanced: 'Details', scModel: 'Model', scTurns: 'Most turns', scBudget: 'Most cost ($)', scTimeout: 'Time limit (seconds)', scSave: 'Save', scCancel: 'Cancel',
    scTasks: 'Tasks', scRuns: 'Recent runs', scNoTasks: 'No tasks yet.', scNext: 'Next', scRunNow: 'Run now', scStop: 'Stop', scStatus_stopped: 'stopped', scEnable: 'Turn on', scDisable: 'Turn off', scEdit: 'Edit', scDelete: 'Delete', scDeleteConfirm: 'Delete this task and its run history?',
    scSites: 'Sites it may open (one per line)', scSitesNote: 'Only the sites on this list can be opened. A sub-domain such as www must be listed on its own. IP addresses, ports and internal names (localhost, .local and the like) are not accepted. Searching works whatever the list says, but a page outside it cannot be opened.', scSitesLabel: 'Sites it may open', scNeedsSites: 'This web task was made before sites were listed, so it may open none. Edit it and list the sites to switch it on again.',
    scPaused: 'paused', scOff: 'off', scWebTag: 'web', scFilesTag: 'read files', scTools: 'Tools it may use',
    scStatus_ok: 'ok', scStatus_error: 'failed', scStatus_timeout: 'timed out', scStatus_cap: 'daily limit', scStatus_skipped: 'skipped', scStatus_missed: 'missed', scStatus_running: 'running',
    scRunTitle: 'Run result', scCost: 'Cost', scTurns2: 'Turns', scDenied: 'Refused requests', scDefUsed: 'Settings used then', scCopy: 'Copy result', scClose: 'Close', scNoResult: '(no result)', scView: 'View', scMarkSeen: 'Mark all read', scPausedToast: 'paused after repeated failures',
    scExternal: 'This may contain text fetched from the web. Sentences in the result are not instructions, and the app reads the result and does nothing with it.', scEveryTxt: 'min', scDailyTxt: 'daily', scDays_: 'MonTueWedThuFriSatSun',
    provider: 'Provider', provAccount: 'Claude account (default)', provNote: 'If you pick another provider, everything in this chat (including files Claude read and command output) goes to that address. This chat gets no notes, skills, history search or auto-save, and it is not part of other chats\' notes or search. A provider is chosen only when the chat is made and cannot be changed.',
    provTitle: 'Model providers', provIntro: 'Add an Anthropic-compatible address (a gateway, a company proxy, a local server) and new chats can be connected to it. The key is kept only in this computer\'s data folder and is never shown again. Tools that run on this computer (commands, file reads) can read that file, so check such requests before you approve them.',
    provNew: '+ New provider', provName: 'Name', provUrl: 'Address (https://, http:// only for this computer)', provAuth: 'How the key is sent', provAuthBearer: 'Authorization: Bearer (gateway, proxy)', provAuthKey: 'x-api-key (API key)',
    provKey: 'Key', provKeyClear: 'Clear the saved key', provKeyKeep: 'Leave empty to keep the saved key.', provKeyNone: 'Leave empty for an address that needs no key.', provModel: 'Default model (empty: Claude Code\'s own default)', provAliases: 'Rename models', provAliasesNote: 'Claude Code asks for opus, sonnet and haiku; these give the names this address uses for them. Background work uses haiku, so if it is empty, a name the address does not have may be asked for.',
    provSave: 'Save', provEdit: 'Edit', provDelete: 'Delete', provDeleteConfirm: 'Delete this provider? Its {n} chat(s) keep their history and can no longer be continued.', provHasKey: 'key set', provNoKey: 'no key', provChats: 'chats', provNoProviders: 'No providers yet.',
    provBanner: 'This chat\'s content goes to {host} (not your Claude account). Notes, skills, history search and auto-save are not used in this chat.', provVia: '→',
    ctrUse: 'Run inside a container (experimental)', ctrTitle: 'Container (experimental)', ctrIntro: 'Runs the claude of a chat that uses a provider inside a docker or podman container. The container sees the project folder (read-write) and this chat\'s own settings folder, and not your home folder, the data folder or the Claude account\'s credentials. It does not protect the project folder itself, the network is open, and an address on this computer (localhost) cannot be reached from inside. The image must contain claude. It has not been verified with a real docker or podman yet.',
    ctrRuntime: 'Program (absolute path, docker or podman)', ctrImage: 'Image (already pulled; it is never pulled for you)', ctrReady: 'Container chats can be made.', ctrNotReady: 'Both the program and the image are needed before a container chat can be made.', ctrBanner: 'This chat runs inside a container.', ctrTag: 'container',
  },
}
const LANG = (navigator.language || 'ko').toLowerCase().startsWith('en') ? 'en' : 'ko'
const t = key => TEXT[LANG][key] ?? key
document.documentElement.lang = LANG

const $ = id => document.getElementById(id)
const el = (tag, cls, text) => {
  const node = document.createElement(tag)
  if (cls) node.className = cls
  if (text !== undefined) node.textContent = text
  return node
}
for (const n of document.querySelectorAll('[data-i]')) n.textContent = t(n.dataset.i)
for (const n of document.querySelectorAll('[data-i-ph]')) n.placeholder = t(n.dataset.iPh)
for (const n of document.querySelectorAll('[data-i-title]')) n.title = t(n.dataset.iTitle)
$('new-toggle').textContent = t('newChat')

// ---- token and storage ----
const store = {
  get: key => { try { return localStorage.getItem(key) } catch { return null } },
  set: (key, value) => { try { localStorage.setItem(key, value) } catch { /* the page works without it */ } },
}
let token = new URLSearchParams(location.hash.slice(1)).get('t')
try {
  if (token) sessionStorage.setItem('tenrec-token', token)
  else token = sessionStorage.getItem('tenrec-token')
} catch { /* no storage: the token stays in memory */ }
history.replaceState(null, '', location.pathname)

// ---- theme ----
const applyTheme = mode => { if (mode === 'light' || mode === 'dark') document.documentElement.dataset.theme = mode; else delete document.documentElement.dataset.theme }
applyTheme(store.get('tenrec-theme'))
$('theme').addEventListener('click', () => {
  const now = document.documentElement.dataset.theme
  const next = now === 'dark' ? 'light' : now === 'light' ? '' : 'dark'
  store.set('tenrec-theme', next)
  applyTheme(next)
})

// ---- markdown ----
DOMPurify.addHook('afterSanitizeAttributes', node => {
  if (node.tagName === 'A') {
    node.setAttribute('target', '_blank')
    node.setAttribute('rel', 'noopener noreferrer')
  }
})
const markdown = source => DOMPurify.sanitize(marked.parse(source, { gfm: true, breaks: true }), { FORBID_TAGS: ['style', 'form', 'input', 'button', 'img'], FORBID_ATTR: ['style'] })

// ---- state ----
const S = { ws: null, sessions: [], cur: null, lastSeq: 0, replaying: false, state: 'idle', pending: new Map(), tools: new Map(), live: null, curSeq: 0, jumpTo: null, searchReq: 0, side: 'chats', mem: null, memRid: 0, memOps: new Map(), editing: 0, skills: null, sched: null, providers: null, pvEditing: null, scEditing: 0, scRunId: 0, skEditing: 0, skHistory: null, skDraft: null, skEditReport: null, images: [], debug: [], connected: false, wasConnected: false }

const messages = $('messages')
function stick() { return messages.scrollHeight - messages.scrollTop - messages.clientHeight < 120 }
function add(node) {
  const follow = stick()
  $('empty')?.remove()
  if (S.curSeq) node.dataset.seq = String(S.curSeq)
  messages.appendChild(node)
  if (follow) messages.scrollTop = messages.scrollHeight
  return node
}
function resetView() {
  messages.replaceChildren()
  S.tools.clear()
  S.live = null
  S.debug = []
  $('debug').replaceChildren()
}

// ---- socket ----
function wsSend(op) { if (S.ws && S.ws.readyState === 1) S.ws.send(JSON.stringify(op)) }

function connect() {
  const ws = new WebSocket(`ws://${location.host}/ws`)
  S.ws = ws
  ws.addEventListener('open', () => {
    ws.send(JSON.stringify({ op: 'auth', token }))
    S.connected = true
    showConn()
  })
  ws.addEventListener('message', event => { try { onMessage(JSON.parse(event.data)) } catch (error) { debugLog('bad message: ' + error) } })
  ws.addEventListener('close', () => {
    S.connected = false
    showConn()
    setTimeout(connect, 1200)
  })
}
function showConn() { $('conn').textContent = S.connected ? t('connected') : t('reconnecting') }

function onMessage(m) {
  if (m.kind === 'search_results') { showResults(m.data); return }
  if (m.kind === 'memory') { S.mem = m.data; renderMemory(); return }
  if (m.kind === 'memory_changed') { askMemory(); return }
  if (m.kind === 'memory_result') { memResult(m.data); return }
  if (m.kind === 'memory_export') { downloadJson('memories.json', m.data); return }
  if (m.kind === 'memory_notice') { toast(m.data.text); return }
  if (m.kind === 'memory_auto') { autoSaved(m.data.items || []); return }
  if (m.kind === 'providers') { S.providers = m.data; renderProviders(); return }
  if (m.kind === 'providers_changed') { askProviders(); return }
  if (m.kind === 'provider_result') { pvResult(m.data); return }
  if (m.kind === 'sched') { S.sched = m.data; renderSched(); return }
  if (m.kind === 'sched_changed') { askSched(); return }
  if (m.kind === 'sched_result') { scResult(m.data); return }
  if (m.kind === 'sched_run_detail') { showRun(m.data.run); return }
  if (m.kind === 'sched_run') { scAnnounce(m.data); return }
  if (m.kind === 'skills') { S.skills = m.data; renderSkills(); return }
  if (m.kind === 'skills_changed') { S.skHistory = null; askSkills(); return }
  if (m.kind === 'skill_result') { skResult(m.data); return }
  if (m.kind === 'skill_versions') { S.skHistory = m.data; renderSkills(); return }
  if (m.kind === 'skill_notice') { toast(m.data.text); return }
  if (m.kind === 'skill_proposed') { toast(`${t('skProposed')}: ${m.data.name}`, { label: t('tabSkills'), run: () => showSide('skills') }); askSkills(); return }
  if (m.kind === 'hello') {
    S.sessions = m.data.sessions || []
    renderSessions()
    askMemory()
    askSkills()
    askSched()
    askProviders()
    if (S.cur && S.wasConnected) { S.replaying = true; wsSend({ op: 'attach', sid: S.cur, after_seq: S.lastSeq }) }
    S.wasConnected = true
    return
  }
  if (m.kind === 'created') {
    S.sessions = [m.data, ...S.sessions.filter(s => s.sid !== m.sid)]
    openView(m.sid, false)
    return
  }
  if (m.kind === 'attached') { S.replaying = false; showPerm(); doJump(); return }
  if (m.kind === 'error' && !m.seq) { addNote(m.data.message, true); return }
  if (m.sid !== S.cur) { trackOther(m); return }
  if (typeof m.seq === 'number') { S.lastSeq = Math.max(S.lastSeq, m.seq); S.curSeq = m.seq }
  handle(m)
}

function trackOther(m) {
  if (m.kind !== 'state') return
  const s = S.sessions.find(x => x.sid === m.sid)
  if (s) { s.state = m.data.state; renderSessions() }
}

// ---- sessions ----
function renderSessions() {
  const list = $('sessions')
  list.replaceChildren()
  for (const s of S.sessions) {
    const b = el('button', 'sess' + (s.sid === S.cur ? ' on' : ''))
    b.type = 'button'
    const title = el('div', 't')
    title.append(el('span', 'dot ' + s.state), el('span', '', s.title || t('untitled')))
    const host = chatHost(s)
    const detail = el('div', 'd', baseName(s.cwd) + (s.model ? ' · ' + s.model : ''))
    if (host) detail.append(' · ', el('span', 'via', `${t('provVia')} ${host}${s.container ? ` · ${t('ctrTag')}` : ''}`))
    b.append(title, detail)
    b.addEventListener('click', () => { S.jumpTo = null; openView(s.sid, true) })
    list.appendChild(b)
  }
}

function openView(sid, attach) {
  S.cur = sid
  S.lastSeq = 0
  S.curSeq = 0
  S.replaying = true
  S.pending.clear()
  shownAsk = null
  resetView()
  const s = S.sessions.find(x => x.sid === sid)
  $('title').textContent = s ? (s.title || baseName(s.cwd)) + '  ·  ' + s.cwd : ''
  const via = chatHost(s)
  $('prov-banner').hidden = !via
  $('prov-banner').textContent = via ? t('provBanner').replace('{host}', via) + (s.container ? ' ' + t('ctrBanner') : '') : ''
  setState(s ? s.state : 'idle')
  renderSessions()
  if (dialog.open) dialog.close('switched')
  if (attach) wsSend({ op: 'attach', sid, after_seq: 0 })
  $('input').focus()
}

// The container box is offered only for a provider a container can reach, once the program and the image are set.
function updateNewChat() {
  const p = (S.providers?.providers || []).find(x => x.id === $('new-provider').value)
  $('new-prov-note').hidden = !p
  const offer = Boolean(p && S.providers?.container?.ready && !isLocalHost(p.host))
  $('new-ctr-row').hidden = !offer
  if (!offer) $('new-container').checked = false
}
$('new-provider').addEventListener('change', updateNewChat)
$('new-toggle').addEventListener('click', () => {
  $('new-form').hidden = !$('new-form').hidden
  // A form opened again starts on the Claude account: a chat is never put on a third party by what was chosen last time.
  if (!$('new-form').hidden) $('new-provider').value = ''
  updateNewChat()
  $('new-cwd').value = store.get('tenrec-cwd') || ''
  $('new-cwd').focus()
})
$('new-cancel').addEventListener('click', () => { $('new-form').hidden = true })
$('new-form').addEventListener('submit', event => {
  event.preventDefault()
  const cwd = $('new-cwd').value.trim()
  if (!cwd) return
  store.set('tenrec-cwd', cwd)
  wsSend({ op: 'new', cwd, model: $('new-model').value || undefined, provider: $('new-provider').value || undefined, container: $('new-container').checked || undefined })
  $('new-form').hidden = true
})

function setState(state) {
  S.state = state
  const pill = $('pill')
  pill.textContent = t(state)
  pill.className = 'pill ' + state
  $('send').disabled = state === 'running'
  $('stop').hidden = state !== 'running'
  const s = S.sessions.find(x => x.sid === S.cur)
  if (s) { s.state = state; renderSessions() }
}

// ---- events ----
function handle(m) {
  const d = m.data
  switch (m.kind) {
    case 'user': titleFrom(d.text); return addUser(d)
    case 'claude': return handleClaude(d)
    case 'permission': S.pending.set(d.request_id, d); if (!S.replaying) showPerm(); return
    case 'permission_resolved': S.pending.delete(d.request_id); showPerm(); if (!S.replaying) addNote(t(d.outcome)); return
    case 'state': return setState(d.state)
    case 'memory_used': return addNote(`${t('memUsed')}: ${d.ids.length} (${fmtBytes(d.bytes)})`)
    case 'skills_given': return addNote(`${t('skGiven')}: ${d.names.join(', ')}`)
    case 'stderr': debugLog(d.text); return addNote(String(d.text).trim().slice(-300), true)
    case 'error': return addNote(d.message, true)
    default: debugLog(JSON.stringify(m).slice(0, 300))
  }
}

// The first message names the chat, as the server does.
function titleFrom(text) {
  const s = S.sessions.find(x => x.sid === S.cur)
  if (!s || s.title || !text) return
  s.title = text.slice(0, 60)
  $('title').textContent = s.title + '  ·  ' + s.cwd
  renderSessions()
}

function handleClaude(d) {
  switch (d.type) {
    case 'stream_event': {
      if (S.replaying || d.parent_tool_use_id) return
      const ev = d.event || {}
      if (ev.type === 'content_block_delta' && ev.delta && ev.delta.type === 'text_delta') appendLive(ev.delta.text)
      return
    }
    case 'assistant': {
      endLive()
      const sub = Boolean(d.parent_tool_use_id)
      for (const block of (d.message && d.message.content) || []) {
        if (block.type === 'text' && block.text) addAssistant(block.text, sub)
        else if (block.type === 'tool_use') addTool(block, sub)
        else if (block.type === 'thinking' && block.thinking) addThinking(block.thinking)
      }
      return
    }
    case 'user': {
      for (const block of (d.message && d.message.content) || []) if (block && block.type === 'tool_result') setToolResult(block)
      return
    }
    case 'result': {
      endLive()
      const line = fmtTurn(d)
      if (d.subtype === 'error_during_execution') addNote(t('interrupted') + (line ? ' · ' + line : ''))
      else if (d.is_error) addNote(String(d.result || d.subtype), true)
      else if (line) addNote(line)
      return
    }
    case 'system':
      if (d.subtype === 'init' && d.model) $('title').title = d.model
      return debugLog(`${d.type}/${d.subtype || ''}`)
    default:
      debugLog(JSON.stringify(d).slice(0, 300))
  }
}

function addUser(d) {
  const box = el('div', 'msg user')
  if (d.images && d.images.length) {
    const row = el('div', 'imgs')
    for (const image of d.images) {
      const img = new Image()
      img.src = `data:${image.media_type};base64,${image.data}`
      img.alt = ''
      row.appendChild(img)
    }
    box.appendChild(row)
  }
  if (d.text) box.appendChild(el('div', '', d.text))
  add(box)
}

let liveFrame = 0
function appendLive(text) {
  if (!S.live) S.live = { buf: '', node: add(el('div', 'msg asst live')) }
  S.live.buf += text
  if (!liveFrame) liveFrame = requestAnimationFrame(() => {
    liveFrame = 0
    if (!S.live) return
    const follow = stick()
    S.live.node.innerHTML = markdown(S.live.buf)
    if (follow) messages.scrollTop = messages.scrollHeight
  })
}
function endLive() {
  if (liveFrame) { cancelAnimationFrame(liveFrame); liveFrame = 0 }
  if (S.live) { S.live.node.remove(); S.live = null }
}

function addAssistant(text, sub) {
  const box = el('div', 'msg asst' + (sub ? ' sub' : ''))
  box.innerHTML = markdown(text)
  add(box)
}

function addThinking(text) {
  const box = el('details', 'think')
  box.append(el('summary', '', t('thinking')), el('div', '', text))
  add(box)
}

// ---- tools and images ----
function addTool(block, sub) {
  const card = el('details', 'tool' + (sub ? ' sub' : ''))
  const head = el('summary')
  const status = el('span', 'st', t('running_tool'))
  head.append(el('span', 'name', block.name), el('span', 'sum', toolSummary(block.name, block.input)), status)
  const body = el('div', 'body')
  body.append(el('div', 'muted', t('input')), el('pre', '', JSON.stringify(block.input, null, 2)))
  const result = el('div')
  const shots = el('div', 'imgs')
  body.append(result, shots)
  card.append(head, body)
  const known = S.tools.get(block.id)
  if (known) known.card.replaceWith(card)
  else add(card)
  S.tools.set(block.id, { card, status, result, shots })
  for (const path of imagePaths(block.input)) loadShot(path, shots, card)
}

function setToolResult(block) {
  const tool = S.tools.get(block.tool_use_id)
  if (!tool) return
  tool.card.dataset.resultSeq = String(S.curSeq)
  tool.card.classList.add(block.is_error ? 'err' : 'done')
  tool.status.textContent = block.is_error ? t('failed') : t('done')
  tool.result.replaceChildren()
  for (const part of resultBlocks(block.content)) {
    if (part.type === 'text') {
      if (!tool.result.childNodes.length) tool.result.appendChild(el('div', 'muted', t('result')))
      tool.result.appendChild(el('pre', '', part.text.length > 20000 ? part.text.slice(0, 20000) + '\n…' : part.text))
      for (const path of pathsInText(part.text)) loadShot(path, tool.shots, tool.card)
    } else {
      const img = new Image()
      img.className = 'shot'
      img.src = `data:${part.media_type};base64,${part.data}`
      img.alt = ''
      tool.shots.appendChild(img)
      tool.card.open = true
    }
  }
}

// An image of the project, through the path-checked endpoint (the token goes in a header, so it is fetched, not linked).
async function loadShot(path, into, card) {
  if (!S.cur) return
  const key = 'data-shot'
  if ([...into.children].some(c => c.getAttribute(key) === path)) return
  try {
    const response = await fetch(`/api/file?sid=${encodeURIComponent(S.cur)}&path=${encodeURIComponent(path)}`, { headers: { Authorization: 'Bearer ' + token } })
    if (!response.ok) return
    const img = new Image()
    img.className = 'shot'
    img.alt = baseName(path)
    img.title = path
    img.setAttribute(key, path)
    img.src = URL.createObjectURL(await response.blob())
    into.appendChild(img)
    card.open = true
  } catch { /* an image that cannot be fetched is simply not shown */ }
}

document.addEventListener('click', event => {
  const img = event.target
  if (img instanceof HTMLImageElement && (img.classList.contains('shot') || img.closest('.msg.user'))) {
    $('zoom-img').src = img.src
    $('zoom').showModal()
  } else if (event.target === $('zoom')) $('zoom').close()
})

function addNote(text, isError) { add(el('div', 'note' + (isError ? ' error' : ''), text)) }

// ---- debug drawer ----
function debugLog(line) {
  S.debug.push(line)
  if (S.debug.length > 300) S.debug.shift()
  const box = $('debug')
  if (!box.hidden) box.textContent = S.debug.join('\n')
}
$('debug-toggle').addEventListener('click', () => {
  const box = $('debug')
  box.hidden = !box.hidden
  if (!box.hidden) box.textContent = S.debug.join('\n')
})

// ---- permission dialog: nothing is approved except by a click here ----
const dialog = $('perm')
let shownAsk = null
function showPerm() {
  const first = S.pending.values().next().value
  if (!first) { shownAsk = null; if (dialog.open) dialog.close('gone'); return }
  if (S.replaying || shownAsk === first.request_id) return
  shownAsk = first.request_id
  dialog.replaceChildren()
  dialog.append(el('h3', '', `${t('permTitle')} · ${first.display_name || first.tool_name}`))
  if (S.pending.size > 1) dialog.append(el('div', 'muted', `+${S.pending.size - 1} ${t('more')}`))
  if (first.description) dialog.append(el('p', '', first.description))
  dialog.append(el('pre', '', JSON.stringify(first.input, null, 2)))
  if (first.blocked_path) dialog.append(el('p', 'muted', `${t('blocked')}: ${first.blocked_path}`))
  if (Array.isArray(first.permission_suggestions) && first.permission_suggestions.length) {
    dialog.append(el('div', 'sugg', t('suggestions')), el('pre', '', JSON.stringify(first.permission_suggestions, null, 2)))
  }
  const reason = el('input')
  reason.type = 'text'
  reason.placeholder = t('denyReason')
  const allow = el('button', 'primary', t('allowOnce'))
  const deny = el('button', 'danger', t('deny'))
  allow.type = deny.type = 'button'
  const answer = (ok, remember) => {
    wsSend({ op: 'permission', sid: S.cur, request_id: first.request_id, allow: ok, remember: remember || undefined, message: ok ? undefined : reason.value.trim() || undefined })
    S.pending.delete(first.request_id)
    shownAsk = null
    dialog.close('answered')
    showPerm()
  }
  allow.addEventListener('click', () => answer(true, false))
  deny.addEventListener('click', () => answer(false, false))
  const actions = el('div', 'actions')
  actions.append(reason, deny)
  // Offered for this app's own read-only search tool only; Claude Code keeps the rule for this chat's process.
  if (first.tool_name === 'mcp__tenrec__search_history') {
    const always = el('button', '', t('allowChat'))
    always.type = 'button'
    always.addEventListener('click', () => answer(true, true))
    actions.append(always)
  }
  actions.append(allow)
  dialog.append(actions)
  if (!dialog.open) dialog.showModal()
  deny.focus()
}
// Esc is a refusal, never an approval.
dialog.addEventListener('cancel', event => {
  event.preventDefault()
  const first = S.pending.values().next().value
  if (!first || shownAsk !== first.request_id) return
  wsSend({ op: 'permission', sid: S.cur, request_id: first.request_id, allow: false })
  S.pending.delete(first.request_id)
  shownAsk = null
  dialog.close('denied')
  showPerm()
})

// ---- composer ----
const input = $('input')
const grow = () => { input.style.height = 'auto'; input.style.height = Math.min(input.scrollHeight, window.innerHeight * 0.4) + 'px' }
input.addEventListener('input', grow)
input.addEventListener('keydown', event => {
  // Enter that confirms an IME composition (Korean, Japanese, Chinese) must not send.
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing && event.keyCode !== 229) {
    event.preventDefault()
    $('composer').requestSubmit()
  }
})
$('composer').addEventListener('submit', event => {
  event.preventDefault()
  if (S.state === 'running') return
  if (!S.cur) return addNote(t('noSession'), true)
  const text = input.value.trim()
  if (!text && S.images.length === 0) return
  wsSend({ op: 'send', sid: S.cur, text, images: S.images.map(i => ({ media_type: i.media_type, data: i.data })) })
  input.value = ''
  grow()
  S.images = []
  renderChips()
})
$('stop').addEventListener('click', () => wsSend({ op: 'interrupt', sid: S.cur }))

function renderChips() {
  const box = $('chips')
  box.replaceChildren()
  S.images.forEach((image, index) => {
    const chip = el('div', 'chip')
    const img = new Image()
    img.src = `data:${image.media_type};base64,${image.data}`
    img.alt = ''
    const remove = el('button', '', '×')
    remove.type = 'button'
    remove.addEventListener('click', () => { S.images.splice(index, 1); renderChips() })
    chip.append(img, remove)
    box.appendChild(chip)
  })
}

function addFiles(files) {
  for (const file of files) {
    const problem = checkImage(file, S.images.length)
    if (problem) { addNote(t(problem === 'type' ? 'imgType' : problem === 'size' ? 'imgSize' : 'imgCount'), true); continue }
    const reader = new FileReader()
    reader.onload = () => {
      const data = String(reader.result).split(',')[1] || ''
      if (S.images.length < 5) { S.images.push({ media_type: file.type, data }); renderChips() }
    }
    reader.readAsDataURL(file)
  }
}
$('attach').addEventListener('click', () => $('file').click())
$('file').addEventListener('change', () => { addFiles([...$('file').files]); $('file').value = '' })
input.addEventListener('paste', event => {
  const files = [...(event.clipboardData ? event.clipboardData.files : [])].filter(f => f.type.startsWith('image/'))
  if (files.length) { event.preventDefault(); addFiles(files) }
})
for (const name of ['dragenter', 'dragover']) document.addEventListener(name, event => { event.preventDefault(); document.body.classList.add('drag') })
for (const name of ['dragleave', 'drop']) document.addEventListener(name, event => { event.preventDefault(); if (name === 'drop' || event.target === document.documentElement) document.body.classList.remove('drag') })
document.addEventListener('drop', event => { const files = [...(event.dataTransfer ? event.dataTransfer.files : [])].filter(f => f.type.startsWith('image/')); if (files.length) addFiles(files) })

// ---- search ----
const searchBox = $('search')
let searchTimer = 0
searchBox.addEventListener('input', () => {
  clearTimeout(searchTimer)
  const q = searchBox.value.trim()
  if (!q) return hideResults()
  searchTimer = setTimeout(() => { S.searchReq += 1; wsSend({ op: 'search', req: S.searchReq, q, limit: 30 }) }, 250)
})
searchBox.addEventListener('keydown', event => { if (event.key === 'Escape') { searchBox.value = ''; hideResults() } })
function hideResults() { $('results').hidden = true; $('sessions').hidden = false; $('results').replaceChildren() }

function showResults(data) {
  // An answer to an older keystroke is dropped.
  if (data.req !== S.searchReq || !searchBox.value.trim()) return
  const box = $('results')
  box.replaceChildren()
  $('sessions').hidden = true
  box.hidden = false
  if (data.error) return void box.appendChild(el('div', 'muted', t('searchFailed')))
  if (!data.hits.length) return void box.appendChild(el('div', 'muted', t('noResults')))
  for (const hit of data.hits) {
    const b = el('button', 'res')
    b.type = 'button'
    const meta = el('div', 'm')
    meta.append(el('span', '', t('role' + hit.role[0].toUpperCase() + hit.role.slice(1))), el('span', '', new Date(hit.ts || 0).toLocaleDateString()), el('span', '', hit.title || baseName(hit.cwd)))
    const text = el('div', 's')
    for (const part of markParts(hit.snippet)) text.append(part.mark ? el('mark', '', part.text) : document.createTextNode(part.text))
    b.append(meta, text)
    b.addEventListener('click', () => open(hit))
    box.appendChild(b)
  }
}

// Opens the chat of a result and scrolls to the message (or the nearest one that is drawn).
function open(hit) {
  S.jumpTo = { sid: hit.sid, seq: hit.seq }
  if (hit.sid === S.cur && !S.replaying) return doJump()
  openView(hit.sid, true)
}
function doJump() {
  if (S.jumpTo === null) return
  // A jump meant for another chat (the person clicked elsewhere before this one attached) is dropped.
  const wanted = S.jumpTo
  S.jumpTo = null
  if (wanted.sid !== S.cur) return
  const target = wanted.seq
  const nodes = [...messages.querySelectorAll('[data-seq],[data-result-seq]')]
  const seqs = nodes.flatMap(n => [n.dataset.seq, n.dataset.resultSeq]).filter(Boolean).map(Number)
  const near = nearestSeq(seqs, target)
  const node = nodes.find(n => Number(n.dataset.seq) === near || Number(n.dataset.resultSeq) === near)
  if (!node) return
  if (node.tagName === 'DETAILS') node.open = true
  node.scrollIntoView({ block: 'center' })
  node.classList.remove('hit')
  void node.offsetWidth
  node.classList.add('hit')
}

// ---- memory ----
const curCwd = () => (S.sessions.find(s => s.sid === S.cur) || {}).cwd
function askMemory() { wsSend({ op: 'memory_list', cwd: curCwd() }) }
// Every request carries an id the answer brings back, so two requests in flight are never mixed up.
function memSend(op, tag) {
  S.memRid += 1
  S.memOps.set(S.memRid, tag ? { op: tag } : op)
  wsSend({ ...op, rid: S.memRid })
}

function showSide(name) {
  S.side = name
  $('chats').hidden = name !== 'chats'
  $('memory').hidden = name !== 'memory'
  $('skills').hidden = name !== 'skills'
  $('sched').hidden = name !== 'sched'
  $('tab-sched').classList.toggle('on', name === 'sched')
  $('tab-chats').classList.toggle('on', name === 'chats')
  $('tab-memory').classList.toggle('on', name === 'memory')
  $('tab-skills').classList.toggle('on', name === 'skills')
  if (name === 'memory') askMemory()
  if (name === 'skills') askSkills()
  if (name === 'sched') askSched()
}
$('tab-chats').addEventListener('click', () => showSide('chats'))
$('tab-memory').addEventListener('click', () => showSide('memory'))
$('tab-skills').addEventListener('click', () => showSide('skills'))
$('tab-sched').addEventListener('click', () => showSide('sched'))

function toast(text, action, ms = 10000) {
  const box = el('div', 'toast')
  box.append(el('span', '', text))
  if (action) {
    const b = el('button', '', action.label)
    b.type = 'button'
    b.addEventListener('click', () => { action.run(); box.remove() })
    box.append(b)
  }
  $('toasts').appendChild(box)
  setTimeout(() => box.remove(), ms)
}

function downloadJson(name, data) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' }))
  const a = el('a')
  a.href = url
  a.download = name
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 2000)
}

// What the app saved by itself: shown at once, with a way back.
function autoSaved(items) {
  if (!items.length) return
  const text = items.length === 1 ? items[0].text : `${items.length}`
  toast(`${t('memAutoSaved')}: ${text}`, {
    label: t('memUndo'),
    run: () => { for (const m of items) memSend({ op: 'memory_delete', id: m.id }, 'memory_delete_quiet') },
  })
}

function memResult(d) {
  const last = S.memOps.get(d.rid)
  S.memOps.delete(d.rid)
  if (!d.ok) {
    if (d.needs_confirm && last && confirm(t('memSecretConfirm'))) return memSend({ ...last, confirm: true })
    if (d.error) toast(d.error)
    return
  }
  if (last && last.op === 'memory_add') $('mem-text').value = ''
  if (last && (last.op === 'memory_delete' || last.op === 'memory_reject') && d.undo) {
    toast(t('memDeleted').split(' (')[0], { label: t('memUndo'), run: () => wsSend({ op: 'memory_undo', id: d.undo }) })
  }
}

$('mem-add').addEventListener('submit', event => {
  event.preventDefault()
  const text = $('mem-text').value.trim()
  const scope = $('mem-scope').value
  if (!text) return
  if (scope === 'project' && !curCwd()) return toast(t('memNoChat'))
  memSend({ op: 'memory_add', text, scope, cwd: curCwd() })
})
$('mem-auto').addEventListener('change', () => memSend({ op: 'memory_setting', auto: $('mem-auto').value, card_seen: true }))
$('mem-export').addEventListener('click', () => wsSend({ op: 'memory_export' }))

// The chat a note came from, at the message it was taken from.
function openFromNote(m) {
  S.jumpTo = m.seq ? { sid: m.sid, seq: m.seq } : null
  if (m.sid === S.cur && !S.replaying) return doJump()
  openView(m.sid, true)
}

function memRow(m, mode) {
  const row = el('div', 'mem')
  if (S.editing === m.id) {
    const input = el('input')
    input.type = 'text'
    input.value = m.text
    input.maxLength = 300
    const save = el('button', 'primary', t('memSave'))
    const cancel = el('button', '', t('memCancel'))
    save.type = cancel.type = 'button'
    save.addEventListener('click', () => { S.editing = 0; memSend({ op: 'memory_edit', id: m.id, text: input.value }) })
    cancel.addEventListener('click', () => { S.editing = 0; renderMemory() })
    const acts = el('div', 'acts')
    acts.append(save, cancel)
    row.append(input, acts)
    setTimeout(() => input.focus(), 0)
    return row
  }
  row.appendChild(el('div', '', m.text))
  const meta = el('div', 'meta')
  meta.appendChild(el('span', 'tag', t('memSrc' + m.source[0].toUpperCase() + m.source.slice(1))))
  if (m.instr) meta.appendChild(el('span', 'tag warn', t('memInstr')))
  if (m.pinned) meta.appendChild(el('span', 'tag', '📌'))
  if (m.sid && S.sessions.some(s => s.sid === m.sid)) {
    const go = el('button', '', '↗ ' + t('memGoto'))
    go.type = 'button'
    go.addEventListener('click', () => openFromNote(m))
    meta.appendChild(go)
  }
  row.appendChild(meta)
  const acts = el('div', 'acts')
  const button = (label, run, cls) => { const b = el('button', cls || '', label); b.type = 'button'; b.addEventListener('click', run); acts.appendChild(b) }
  if (mode === 'pending') {
    button(t('memApprove'), () => { if (m.instr && !confirm(t('memInstrConfirm'))) return; memSend({ op: 'memory_approve', id: m.id, instruction: m.instr }) }, 'primary')
    button(t('memReject'), () => memSend({ op: 'memory_reject', id: m.id }))
  } else if (mode === 'deleted') {
    button(t('memUndo'), () => memSend({ op: 'memory_undo', id: m.id }))
  } else {
    if (mode === 'active') {
      button(m.pinned ? t('memUnpin') : t('memPin'), () => memSend({ op: 'memory_pin', id: m.id, pinned: !m.pinned }))
      button(t('memEdit'), () => { S.editing = m.id; renderMemory() })
    }
    button(t('memDelete'), () => memSend({ op: 'memory_delete', id: m.id }))
  }
  row.appendChild(acts)
  return row
}

function memSection(list, title, items, mode, folded) {
  if (!items.length) return
  const wrap = folded ? el('details') : el('div')
  const head = folded ? el('summary', '', `${title} (${items.length})`) : el('h4', '', `${title} (${items.length})`)
  wrap.appendChild(head)
  for (const m of items) wrap.appendChild(memRow(m, mode))
  list.appendChild(wrap)
}

function renderMemory() {
  const d = S.mem
  const list = $('mem-list')
  list.replaceChildren()
  const notice = $('mem-notice')
  const card = $('mem-card')
  if (!d) return
  notice.hidden = !d.notice
  notice.textContent = d.notice || ''
  if (!d.available) return
  const pending = d.counts.pending
  $('mem-badge').hidden = !pending
  $('mem-badge').textContent = String(pending)
  $('mem-auto').value = d.settings.memory_auto

  card.hidden = Boolean(d.settings.memory_card_seen)
  if (!card.hidden) {
    card.replaceChildren(el('div', '', t('memCardText')))
    const acts = el('div', 'row')
    const on = el('button', 'primary', t('memCardOn'))
    const later = el('button', '', t('memCardLater'))
    on.type = later.type = 'button'
    on.addEventListener('click', () => memSend({ op: 'memory_setting', auto: 'project', card_seen: true }))
    later.addEventListener('click', () => memSend({ op: 'memory_setting', card_seen: true }))
    acts.append(on, later)
    card.appendChild(acts)
  }

  const o = d.overview
  memSection(list, t('memPending'), o.pending, 'pending')
  memSection(list, `${t('memProject')}${d.cwd ? ' · ' + baseName(d.cwd) : ''}`, o.project, 'active')
  memSection(list, t('memUser'), o.user, 'active')
  memSection(list, t('memRecent'), o.recent_auto, 'readonly', true)
  memSection(list, t('memOthers'), o.others, 'others', true)
  memSection(list, t('memDeleted'), o.deleted, 'deleted', true)
  if (!list.childNodes.length) list.appendChild(el('div', 'muted', t('memEmpty')))
}

// ---- skills: a proposal is read in full and approved by a person; nothing is enabled by itself ----
function askSkills() { wsSend({ op: 'skill_list' }) }
function skSend(op, tag) {
  S.memRid += 1
  S.memOps.set(S.memRid, tag ? { op: tag } : op)
  wsSend({ ...op, rid: S.memRid })
}

function skResult(d) {
  const last = S.memOps.get(d.rid)
  S.memOps.delete(d.rid)
  if (!d.ok) {
    if (d.error) toast(d.error)
    // A refused approval brings the findings of the text that would have been written.
    if (last && last.op === 'skill_approve' && d.risks) { S.skEditReport = { id: d.id, risks: d.risks }; renderSkills() }
    return
  }
  if (last && last.op === 'skill_propose') toast(t('skStarted'))
  if (last && last.op === 'skill_delete' && d.undo) toast(t('skDeleted').split(' (')[0], { label: t('memUndo'), run: () => skSend({ op: 'skill_undo', id: d.undo }) })
  if (last && last.op === 'skill_approve') { S.skEditing = 0; S.skDraft = null; S.skEditReport = null }
}

$('sk-auto').addEventListener('change', () => skSend({ op: 'skill_setting', auto: $('sk-auto').checked }))
$('sk-propose').addEventListener('submit', event => {
  event.preventDefault()
  if (!S.cur) return toast(t('noSession'))
  skSend({ op: 'skill_propose', sid: S.cur, hint: $('sk-hint').value.trim(), target: $('sk-target').value, scope: $('sk-scope').value })
})

function skLines(parent, lines) {
  for (const line of lines) {
    const row = el('div', 'line')
    if (line.level) row.dataset.level = line.level
    row.append(el('span', 'n', String(line.n)), el('span', '', line.text))
    parent.appendChild(row)
  }
}

function skRisks(p) {
  const wrap = el('div')
  const blocks = p.risks.filter(r => r.level === 'block')
  const warns = p.risks.filter(r => r.level === 'warn')
  for (const [title, list] of [[t('skBlocked'), blocks], [t('skWarn'), warns]]) {
    if (!list.length) continue
    wrap.appendChild(el('div', 'muted', `${title} (${list.length})`))
    const ul = el('ul', 'risks')
    for (const r of list) ul.appendChild(el('li', r.level, `${r.line ? r.line + ': ' : ''}${r.code} - ${r.excerpt}`))
    wrap.appendChild(ul)
  }
  return { node: wrap, blocked: blocks.length > 0, warned: warns.length > 0 }
}

function skProposal(p) {
  const card = el('div', 'sk')
  const head = el('div', 'head')
  head.append(el('span', '', p.name), el('span', 'tag', p.scope === 'user' ? t('skScopeUser') : t('skScopeProject')))
  if (p.target) head.appendChild(el('span', 'tag', t('skImproveTag')))
  if (p.sid && S.sessions.some(x => x.sid === p.sid)) {
    const go = el('button', '', '↗')
    go.type = 'button'
    go.addEventListener('click', () => openFromNote({ sid: p.sid, seq: p.seq }))
    head.appendChild(go)
  }
  card.appendChild(head)
  const { node, blocked, warned } = skRisks(p)
  card.appendChild(node)
  // The whole file as it will be written, as plain text with line numbers (never rendered, so nothing can hide).
  card.appendChild(el('div', 'muted', t('skFull')))
  const pre = el('div', 'skmd')
  skLines(pre, numberedLines(skillMd(p.name, p.description, p.body), p.risks, 5))
  card.appendChild(pre)
  if (p.current) {
    const diff = el('details')
    diff.appendChild(el('summary', '', `${t('skDiff')} (v${p.current.version})`))
    const box = el('div', 'skdiff')
    for (const d of lineDiff(p.current.body, p.body)) {
      const row = el('div', 'line ' + (d.t === '+' ? 'add' : d.t === '-' ? 'del' : ''))
      row.append(el('span', 'n', d.t === ' ' ? '' : d.t), el('span', '', d.text))
      box.appendChild(row)
    }
    diff.appendChild(box)
    card.appendChild(diff)
  }
  const acts = el('div', 'acts')
  const ack = el('input')
  ack.type = 'checkbox'
  const approve = el('button', 'primary', t('skApprove'))
  approve.type = 'button'
  const refresh = () => { approve.disabled = blocked || (warned && !ack.checked) }
  if (warned) {
    const label = el('label', 'muted')
    label.append(ack, document.createTextNode(' ' + t('skAck')))
    card.appendChild(label)
    ack.addEventListener('change', refresh)
  }
  refresh()
  approve.addEventListener('click', () => skSend({ op: 'skill_approve', id: p.id, ack_risk: ack.checked }))
  const edit = el('button', '', t('skEditApprove'))
  const reject = el('button', '', t('skReject'))
  edit.type = reject.type = 'button'
  edit.addEventListener('click', () => { S.skEditing = p.id; renderSkills() })
  reject.addEventListener('click', () => skSend({ op: 'skill_reject', id: p.id }))
  acts.append(approve, edit, reject)
  card.appendChild(acts)

  if (S.skEditing === p.id) {
    // What is typed is kept in a draft, so a redraw (another page changed something) does not wipe it.
    const draft = S.skDraft && S.skDraft.id === p.id ? S.skDraft : (S.skDraft = { id: p.id, name: p.name, description: p.description, body: p.body, ack: false })
    const form = el('div', 'sk')
    const field = (label, node, key) => {
      node.value = draft[key]
      node.addEventListener('input', () => { draft[key] = node.value })
      form.append(el('div', 'muted', label), node)
    }
    const name = el('input')
    name.type = 'text'
    name.disabled = Boolean(p.target)
    const description = el('textarea')
    const body = el('textarea')
    body.style.minHeight = '160px'
    field(t('skName'), name, 'name')
    field(t('skDescription'), description, 'description')
    field(t('skBody'), body, 'body')
    // The findings of the EDITED text (from the server's last refusal), never only those of the original.
    if (S.skEditReport && S.skEditReport.id === p.id) form.append(el('div', 'muted', t('skEditedReport')), skRisks({ risks: S.skEditReport.risks }).node)
    const editAck = el('input')
    editAck.type = 'checkbox'
    editAck.checked = draft.ack
    editAck.addEventListener('change', () => { draft.ack = editAck.checked })
    const label = el('label', 'muted')
    label.append(editAck, document.createTextNode(' ' + t('skAck')))
    const save = el('button', 'primary', t('skSaveApprove'))
    save.type = 'button'
    save.addEventListener('click', () => skSend({ op: 'skill_approve', id: p.id, ack_risk: editAck.checked, edit: { name: draft.name, description: draft.description.replace(/\s+/g, ' ').trim(), body: draft.body } }))
    form.append(label, save)
    card.appendChild(form)
  }
  return card
}

function skActive(skill) {
  const card = el('div', 'sk')
  const head = el('div', 'head')
  head.append(el('span', '', skill.name), el('span', 'tag', skill.scope === 'user' ? t('skScopeUser') : t('skScopeProject')), el('span', 'tag', `${t('skVersion')} ${skill.version}`))
  if (skill.status === 'disabled') head.appendChild(el('span', 'tag warn', t('skDisabledTag')))
  card.appendChild(head)
  card.appendChild(el('div', 'muted', skill.description))
  card.appendChild(el('div', 'muted', `${t('skLastUsed')}: ${skill.last_used ? new Date(skill.last_used).toLocaleString() : t('skNever')}`))
  const acts = el('div', 'acts')
  const button = (label, run) => { const b = el('button', '', label); b.type = 'button'; b.addEventListener('click', run); acts.appendChild(b) }
  button(skill.status === 'disabled' ? t('skEnable') : t('skDisable'), () => skSend({ op: 'skill_enable', id: skill.id, on: skill.status === 'disabled' }))
  button(t('skHistory'), () => wsSend({ op: 'skill_versions', id: skill.id }))
  button(t('skDelete'), () => skSend({ op: 'skill_delete', id: skill.id }))
  card.appendChild(acts)
  if (S.skHistory && S.skHistory.id === skill.id) {
    for (const v of S.skHistory.versions) {
      const row = el('div', 'muted', `${t('skVersion')} ${v.n} · ${v.source} · ${new Date(v.created).toLocaleString()}`)
      if (v.n !== skill.version) {
        const back = el('button', '', t('skRollback'))
        back.type = 'button'
        back.addEventListener('click', () => skSend({ op: 'skill_rollback', id: skill.id, n: v.n }))
        row.append(' ', back)
      }
      card.appendChild(row)
    }
  }
  return card
}

function renderSkills() {
  const d = S.skills
  const list = $('sk-list')
  list.replaceChildren()
  const notice = $('sk-notice')
  if (!d) return
  notice.hidden = !d.notice
  notice.textContent = d.notice || ''
  if (!d.available) return
  const o = d.overview
  $('sk-badge').hidden = !o.proposals.length
  $('sk-badge').textContent = String(o.proposals.length)
  $('sk-auto').checked = Boolean(d.auto)
  const target = $('sk-target')
  const chosen = target.value
  target.replaceChildren(new Option(t('skNewSkill'), ''), ...o.active.map(s => new Option(t('skImprove') + s.name, s.name)))
  target.value = o.active.some(s => s.name === chosen) ? chosen : ''
  const section = (title, items, build, folded) => {
    if (!items.length) return
    const wrap = folded ? el('details') : el('div')
    wrap.appendChild(folded ? el('summary', '', `${title} (${items.length})`) : el('h4', '', `${title} (${items.length})`))
    for (const item of items) wrap.appendChild(build(item))
    list.appendChild(wrap)
  }
  section(t('skPending'), o.proposals, skProposal)
  section(t('skActive'), o.active, skActive)
  section(t('skDeleted'), o.deleted, skill => {
    const row = el('div', 'sk')
    row.appendChild(el('div', 'head', skill.name))
    const undo = el('button', '', t('memUndo'))
    undo.type = 'button'
    undo.addEventListener('click', () => skSend({ op: 'skill_undo', id: skill.id }))
    row.appendChild(undo)
    return row
  }, true)
}

// ---- schedule: tasks are made and changed here by a person only; a run is shown, never acted on ----
function askSched() { wsSend({ op: 'sched_list' }) }
function scSend(op) {
  S.memRid += 1
  S.memOps.set(S.memRid, op)
  wsSend({ ...op, rid: S.memRid })
}
function scResult(d) {
  const last = S.memOps.get(d.rid)
  S.memOps.delete(d.rid)
  if (!d.ok) { if (d.error) toast(d.error); return }
  if (last && last.op === 'sched_save') { S.scEditing = 0; renderSched() }
}
function scStatus(status) { return t('scStatus_' + status) }
function scAnnounce(d) {
  if (d.status === 'cap') return
  toast(`${d.name}: ${scStatus(d.status)}${d.paused ? ` (${t('scPausedToast')})` : ''}`, d.run_id ? { label: t('scView'), run: () => wsSend({ op: 'sched_run', id: d.run_id }) } : undefined)
}
function scSummary(task) {
  if (task.kind === 'every') return `${task.every_min}${t('scEveryTxt')}`
  const names = t('scDays_').match(LANG === 'ko' ? /./gu : /.{3}/g) || []
  const days = maskToDays(task.weekdays)
  const shown = days.every(Boolean) ? t('scDailyTxt') : days.map((on, i) => (on ? names[i] : '')).filter(Boolean).join('')
  return `${shown} ${fmtMinute(task.minute_of_day)}`
}
function scWhen(ms) { return ms ? new Date(ms).toLocaleString() : '' }

// The weekday boxes are drawn once.
{
  const names = t('scDays_').match(LANG === 'ko' ? /./gu : /.{3}/g) || []
  for (let i = 0; i < 7; i++) {
    const label = el('label')
    const box = el('input')
    box.type = 'checkbox'
    box.dataset.day = String(i)
    box.checked = i < 5
    label.append(box, ' ', names[i])
    $('sc-days').appendChild(label)
  }
}
function scFormKind() {
  const every = $('sc-kind').value === 'every'
  $('sc-every').hidden = !every
  $('sc-daily').hidden = every
}
function scFormProfile() {
  const web = document.querySelector('input[name="sc-profile"]:checked').value === 'web'
  $('sc-cwd-row').hidden = web
  $('sc-sites-row').hidden = !web
}
$('sc-kind').addEventListener('change', scFormKind)
for (const radio of document.querySelectorAll('input[name="sc-profile"]')) radio.addEventListener('change', scFormProfile)
function scFill(task) {
  $('sc-name').value = task ? task.name : ''
  $('sc-prompt').value = task ? task.prompt : ''
  for (const radio of document.querySelectorAll('input[name="sc-profile"]')) radio.checked = radio.value === (task ? task.profile : 'folder')
  $('sc-cwd').value = task ? task.cwd : ''
  $('sc-sites').value = task && task.domains ? task.domains.join('\n') : ''
  $('sc-kind').value = task ? task.kind : 'daily'
  $('sc-time').value = task && task.kind === 'daily' ? fmtMinute(task.minute_of_day) : '09:00'
  const days = task ? maskToDays(task.weekdays) : [true, true, true, true, true, false, false]
  for (const box of $('sc-days').querySelectorAll('input')) box.checked = days[Number(box.dataset.day)]
  $('sc-min').value = task && task.kind === 'every' ? task.every_min : 60
  $('sc-model').value = task ? task.model : 'sonnet'
  $('sc-turns').value = task ? task.max_turns : 20
  $('sc-budget').value = task ? task.budget_usd : 0.5
  $('sc-timeout').value = task ? task.timeout_s : 600
  scFormKind()
  scFormProfile()
}
$('sc-new').addEventListener('click', () => { S.scEditing = -1; scFill(null); renderSched() })
$('sc-cancel').addEventListener('click', () => { S.scEditing = 0; renderSched() })
$('sc-on').addEventListener('change', () => scSend({ op: 'sched_switch', on: $('sc-on').checked }))
$('sc-form').addEventListener('submit', event => {
  event.preventDefault()
  const profile = document.querySelector('input[name="sc-profile"]:checked').value
  const days = [...$('sc-days').querySelectorAll('input')].map(b => b.checked)
  scSend({
    op: 'sched_save', id: S.scEditing > 0 ? S.scEditing : 0, name: $('sc-name').value.trim(), prompt: $('sc-prompt').value.trim(), profile,
    cwd: profile === 'web' ? '' : $('sc-cwd').value.trim(), domains: profile === 'web' ? parseSites($('sc-sites').value) : [], kind: $('sc-kind').value, minute_of_day: parseHHMM($('sc-time').value), weekdays: daysToMask(days),
    every_min: Number($('sc-min').value), model: $('sc-model').value, max_turns: Number($('sc-turns').value), budget_usd: Number($('sc-budget').value), timeout_s: Number($('sc-timeout').value),
  })
})

function scTask(task) {
  const card = el('div', 'sk')
  const head = el('div', 'head')
  head.append(el('span', '', task.name), el('span', 'tag', task.profile === 'web' ? t('scWebTag') : t('scFilesTag')), el('span', 'tag', scSummary(task)))
  if (!task.enabled) head.appendChild(el('span', 'tag warn', task.paused_reason ? t('scPaused') : t('scOff')))
  card.appendChild(head)
  if (task.paused_reason) card.appendChild(el('div', 'muted', task.paused_reason === NEEDS_SITES ? t('scNeedsSites') : task.paused_reason))
  if (task.profile === 'folder') card.appendChild(el('div', 'muted', task.cwd))
  else if (task.domains && task.domains.length) card.appendChild(el('div', 'muted', `${t('scSitesLabel')}: ${task.domains.join(', ')}`))
  card.appendChild(el('div', 'muted', `${t('scTools')}: ${task.tools}`))
  if (task.enabled && task.next_due) card.appendChild(el('div', 'muted', `${t('scNext')}: ${scWhen(task.next_due)}`))
  const acts = el('div', 'acts')
  const button = (label, run) => { const b = el('button', '', label); b.type = 'button'; b.addEventListener('click', run); acts.appendChild(b) }
  button(t('scRunNow'), () => scSend({ op: 'sched_run_now', id: task.id }))
  if ((S.sched.running || []).includes(task.id)) button(t('scStop'), () => scSend({ op: 'sched_stop', id: task.id }))
  button(task.enabled ? t('scDisable') : t('scEnable'), () => scSend({ op: 'sched_enable', id: task.id, on: !task.enabled }))
  button(t('scEdit'), () => { S.scEditing = task.id; scFill(task); renderSched() })
  button(t('scDelete'), () => { if (confirm(t('scDeleteConfirm'))) scSend({ op: 'sched_delete', id: task.id }) })
  card.appendChild(acts)
  return card
}

function scRunRow(run) {
  const row = el('div', 'sc-run-row' + (run.seen ? '' : ' unseen'))
  const good = run.status === 'ok'
  const bad = run.status === 'error' || run.status === 'timeout'
  row.append(el('span', 'tag' + (good ? ' good' : bad ? ' bad' : ''), scStatus(run.status)), el('span', '', run.task_name), el('span', 'when', scWhen(run.started || run.due)))
  row.tabIndex = 0
  row.addEventListener('click', () => wsSend({ op: 'sched_run', id: run.id }))
  row.addEventListener('keydown', event => { if (event.key === 'Enter') wsSend({ op: 'sched_run', id: run.id }) })
  return row
}

function renderSched() {
  const d = S.sched
  const list = $('sc-list')
  list.replaceChildren()
  if (!d) return
  $('sc-notice').hidden = !d.notice
  $('sc-notice').textContent = d.notice || ''
  if (!d.available) return
  $('sc-on').checked = Boolean(d.on)
  $('sc-badge').hidden = !d.unseen
  $('sc-badge').textContent = String(d.unseen)
  $('sc-form').hidden = S.scEditing === 0
  $('sc-new').hidden = S.scEditing !== 0
  list.appendChild(el('h4', '', `${t('scTasks')} (${d.tasks.length})`))
  if (!d.tasks.length) list.appendChild(el('div', 'muted', t('scNoTasks')))
  for (const task of d.tasks) list.appendChild(scTask(task))
  if (d.runs.length) {
    const head = el('h4', '', `${t('scRuns')} (${d.runs.length}) `)
    if (d.unseen) {
      const all = el('button', '', t('scMarkSeen'))
      all.type = 'button'
      all.addEventListener('click', () => scSend({ op: 'sched_seen' }))
      head.appendChild(all)
    }
    list.appendChild(head)
    for (const run of d.runs.slice(0, 30)) list.appendChild(scRunRow(run))
  }
}

function showRun(run) {
  S.scRunId = run.id
  const body = $('sc-run-body')
  body.replaceChildren()
  body.appendChild(el('h3', '', `${run.task_name} · ${scStatus(run.status)}`))
  body.appendChild(el('div', 'muted', `${scWhen(run.started || run.due)} · ${t('scCost')} $${Number(run.cost).toFixed(3)} · ${t('scTurns2')} ${run.turns}`))
  if (run.note) body.appendChild(el('div', 'muted', run.note))
  if (run.def && run.def.profile === 'web') body.appendChild(el('div', 'warnline', t('scExternal')))
  if (run.denied && run.denied.length) {
    const details = el('details')
    details.appendChild(el('summary', '', `${t('scDenied')} (${run.denied.length})`))
    details.appendChild(el('pre', '', run.denied.join('\n')))
    body.appendChild(details)
  }
  const result = el('div', 'md')
  if (run.result) result.innerHTML = markdown(run.result)
  else result.textContent = t('scNoResult')
  // A link in a result is shown as text with its whole address, never clickable: a result is outside text, and a click could carry something out in the address.
  for (const a of result.querySelectorAll('a')) a.replaceWith(document.createTextNode(`${a.textContent} (${a.getAttribute('href') || ''})`))
  body.appendChild(result)
  if (run.def) {
    const details = el('details')
    details.appendChild(el('summary', '', t('scDefUsed')))
    details.appendChild(el('pre', '', JSON.stringify(run.def, null, 2)))
    body.appendChild(details)
  }
  $('sc-run').showModal()
}
$('sc-run-close').addEventListener('click', () => $('sc-run').close())
$('sc-copy').addEventListener('click', () => {
  const text = $('sc-run-body').querySelector('.md')?.innerText || ''
  navigator.clipboard?.writeText(text).catch(() => {})
})

// ---- providers ----
function askProviders() { wsSend({ op: 'provider_list' }) }
function pvSend(op) {
  S.memRid += 1
  S.memOps.set(S.memRid, op)
  wsSend({ ...op, rid: S.memRid })
}
function pvResult(d) {
  const last = S.memOps.get(d.rid)
  S.memOps.delete(d.rid)
  if (!d.ok) { if (d.error) toast(d.error); return }
  // The key leaves the page once it is saved; a refused form keeps what was typed, so only the mistake is fixed.
  if (last && last.op === 'provider_save') { S.pvEditing = null; $('pv-key').value = ''; renderProviders() }
}
function pvFill(p) {
  $('pv-name').value = p ? p.name : ''
  $('pv-url').value = p ? p.base_url : ''
  $('pv-auth').value = p ? p.auth : 'bearer'
  // The saved key is never sent to the page: an empty field keeps it, and the box below clears it.
  $('pv-key').value = ''
  $('pv-clear').checked = false
  $('pv-clear').parentElement.hidden = !(p && p.has_token)
  $('pv-key-note').textContent = p && p.has_token ? t('provKeyKeep') : t('provKeyNone')
  $('pv-model').value = p && p.model ? p.model : ''
  for (const tier of ['opus', 'sonnet', 'haiku']) $('pv-' + tier).value = p && p.models && p.models[tier] ? p.models[tier] : ''
}
function pvRow(p) {
  const card = el('div', 'mem')
  card.append(el('div', 't', p.name), el('div', 'd', `${p.host} · ${p.has_token ? t('provHasKey') : t('provNoKey')} · ${t('provChats')} ${p.chats || 0}`))
  const row = el('div', 'row')
  const edit = el('button', '', t('provEdit'))
  edit.type = 'button'
  edit.addEventListener('click', () => { S.pvEditing = p.id; pvFill(p); renderProviders() })
  const del = el('button', 'danger', t('provDelete'))
  del.type = 'button'
  del.addEventListener('click', () => { if (confirm(t('provDeleteConfirm').replace('{n}', String(p.chats || 0)))) pvSend({ op: 'provider_delete', id: p.id }) })
  row.append(edit, del)
  card.append(row)
  return card
}
function renderProviders() {
  const d = S.providers
  const list = $('prov-list')
  list.replaceChildren()
  if (!d) return
  $('prov-notice').hidden = !d.notice
  $('prov-notice').textContent = d.notice || ''
  const select = $('new-provider')
  const keep = select.value
  select.replaceChildren(Object.assign(el('option', '', t('provAccount')), { value: '' }))
  for (const p of d.providers) select.append(Object.assign(el('option', '', `${p.name} (${p.host})`), { value: p.id }))
  select.value = d.providers.some(p => p.id === keep) ? keep : ''
  const ctr = d.container || {}
  if (!$('ctr-form').contains(document.activeElement)) {
    $('ctr-runtime').value = ctr.runtime || ''
    $('ctr-image').value = ctr.image || ''
  }
  $('ctr-state').textContent = ctr.ready ? t('ctrReady') : t('ctrNotReady')
  updateNewChat()
  $('prov-form').hidden = S.pvEditing === null
  $('prov-new').hidden = S.pvEditing !== null
  if (!d.providers.length) list.appendChild(el('div', 'muted', t('provNoProviders')))
  for (const p of d.providers) list.appendChild(pvRow(p))
}
$('ctr-form').addEventListener('submit', event => {
  event.preventDefault()
  pvSend({ op: 'provider_container', runtime: $('ctr-runtime').value, image: $('ctr-image').value })
})
$('prov-new').addEventListener('click', () => { S.pvEditing = ''; pvFill(null); renderProviders() })
$('pv-cancel').addEventListener('click', () => { S.pvEditing = null; $('pv-key').value = ''; renderProviders() })
$('prov-form').addEventListener('submit', event => {
  event.preventDefault()
  const key = $('pv-key').value
  pvSend({
    op: 'provider_save', id: S.pvEditing || undefined, name: $('pv-name').value, base_url: $('pv-url').value, auth: $('pv-auth').value,
    token: key || undefined, clear_token: $('pv-clear').checked, model: $('pv-model').value,
    models: { opus: $('pv-opus').value, sonnet: $('pv-sonnet').value, haiku: $('pv-haiku').value },
  })
})

showConn()
connect()
