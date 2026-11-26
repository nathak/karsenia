import { marked } from './vendor/marked.js'
import DOMPurify from './vendor/purify.js'
import { initFiles } from './files.js'
import { initTerm } from './term.js'
import { initWt } from './wt.js'
import { systemNote, guardKind, initChips, thinkSummary, codeLang, foldInfo, foldText, editDiff, todoItems, toolHead, toolBadge, isAgentCall, wtText, accountName, accountOf, forAccount, inContext, viewOf, attn, permChoices, permNeedsConfirm, permOf, formFill, unfill, fillDiffers, justFilled, listed, grantButtons, modelChoices, withAccount, baseName, chatHost, checkImage, isLocalHost, daysToMask, fmtBytes, fmtMinute, maskToDays, parseHHMM, fmtTurn, imagePaths, lineDiff, markParts, nearestSeq, numberedLines, parseSites, pathsInText, resultBlocks, skillMd, toolSummary, dayKey, clock, ago, modelLabel, turnLine, codeLines, tailPath } from './util.js'
// The reason the host gives a web task made before sites were listed (src/sched_store.rs NEEDS_DOMAINS); shown translated.
const NEEDS_SITES = 'needs the sites it may open: edit the task and list them'

// ---- text ----
const TEXT = {
  ko: {
    newChat: '＋ 새 대화', folder: '프로젝트 폴더 (절대 경로)', model: '모델', create: '만들기', cancel: '취소', ncDefaultsUsed: '이 프로젝트의 마지막 대화 설정을 채웠습니다.', ncDefaultsReset: '기본값으로', ncRecent: '최근 프로젝트', ncTrustRemembered: '이 폴더의 프로젝트 설정(훅·명령)을 새 대화에서 묻지 않고 허용하도록 기억해 두었습니다.', ncTrustForget: '신뢰 잊기', ncTrustForgotten: '이 폴더의 신뢰를 잊었습니다. 새 대화는 다시 묻습니다.', ncDefaultsHold: '이 프로젝트의 설정을 방금 채웠습니다. 확인한 뒤 다시 누르세요.', empty: '왼쪽에서 대화를 고르거나 새 대화를 만드세요.',
    placeholder: '메시지를 입력하세요 (Enter 전송, Shift+Enter 줄바꿈). 이미지는 붙여넣기·끌어놓기', send: '보내기', stop: '중단', attach: '이미지 첨부',
    idle: '대기', running: '작업 중', exited: '종료됨', connected: '연결됨', reconnecting: '다시 연결하는 중…', untitled: '(새 대화)', acctNoChats: '이 계정에는 대화가 없습니다.',
    plugTitle: '플러그인', plugIntro: '기본 계정에 설치된 플러그인 중 이 계정에 없는 것입니다. 계정마다 설정 폴더가 따로라 직접 설치해야 합니다.', plugNone: '설치할 플러그인이 없습니다.', plugInstall: '설치', plugBusy: '설치 중…', plugConfirm: '{p} 플러그인을 이 계정에 설치합니다. 플러그인은 코드를 실행할 수 있습니다. 계속할까요?', plugDone: '설치했습니다', plugFailed: '설치하지 못했습니다',
    chatName: '대화 이름 (선택)', renameBtn: '이름 변경', archiveBtn: '보관', unarchiveBtn: '보관 해제', archiveShown: '보관함', renamePrompt: '새 이름 (60자까지)', deleteBtn: '삭제', deleteConfirm: '이 대화를 삭제합니다. 기록이 모두 지워지고 되돌릴 수 없습니다. 계속할까요?',
    modelDefault: '기본 모델', modelNow: '모델 변경', modelNote: '다음 메시지부터 이어서 이 모델로 실행합니다',
    perm: '권한 모드', permTag: '권한',
    permM_default: '기본 (읽기만 자동 허용)', permN_default: '읽기만 묻지 않고 실행합니다. 나머지는 매번 묻습니다.',
    permM_acceptEdits: '편집 자동 허용 (acceptEdits)', permN_acceptEdits: '읽기·파일 수정·mkdir/touch/mv/cp는 묻지 않습니다.',
    permM_plan: '계획만 (plan)', permN_plan: '읽기 전용으로 계획만 세웁니다.',
    permM_auto: '자동 검토 (auto)', permN_auto: '분류기가 동작을 검토합니다. Claude Code가 지원하는 모델·요금제가 필요하며, 아니면 기본처럼 동작합니다.',
    permM_dontAsk: '묻지 않고 거부 (dontAsk)', permN_dontAsk: '미리 허용된 도구만 실행하고, 나머지는 묻지 않고 거부합니다.',
    permM_bypassPermissions: '모든 확인 건너뛰기 (--dangerously-skip-permissions)', permN_bypassPermissions: '모든 동작을 묻지 않고 실행합니다. deny/ask 규칙, 훅, 보호 경로 삭제 확인은 남습니다. root/sudo로 실행하면 Claude Code가 거부합니다.',
    permBypassConfirm: '이 대화는 파일 수정과 명령 실행을 포함해 모든 도구를 묻지 않고 실행합니다. 되돌릴 수 없는 작업도 포함됩니다. 대화를 만든 뒤에는 권한 모드를 바꿀 수 없습니다. 계속할까요?', acctAttn: '확인 필요',
    allowSession: '이번 실행 동안 허용', allowAlways: '이 프로젝트에서 항상 허용', grantSessionNote: 'Claude Code가 이 프로세스에서 기억합니다. 다시 시작하거나 모델을 바꾸면 사라질 수 있습니다.', grantAlwaysNote: 'Claude Code가 프로젝트의 .claude/settings.local.json에 규칙을 씁니다. 이 폴더의 모든 대화에 적용되며 앱에서 지우는 기능은 아직 없습니다.', grantExact: '정확히 이 명령만 허용합니다.', grantWild: '이 규칙은 와일드카드(*)를 포함합니다.', grantRules: 'Claude가 제안한 규칙',
    allowOnce: '이번만 허용', deny: '거절', denyReason: '거절 사유 (선택)', permTitle: '도구 사용 승인', blocked: '막힌 경로', suggestions: 'Claude가 제안한 규칙 (참고용, 적용하지 않음)',
    allowed: '허용했습니다', denied: '거절했습니다', cancelled: '승인 요청이 취소되었습니다', running_tool: '실행 중', done: '완료', failed: '오류', more: '더 있음',
    imgType: '이미지는 png, jpeg, gif, webp만 됩니다', imgSize: '이미지는 장당 5MB까지입니다', imgCount: '이미지는 5장까지입니다', thinking: '생각', interrupted: '중단했습니다',
    noSession: '먼저 대화를 만들거나 고르세요.', input: '입력', result: '결과',
    cvThinkSecs: '{s}초', cvThinkTok: '약 {n}토큰', cvThinkSum: '생각 · {s}초 · {n}자', cvThinkSumNoTime: '생각 · {n}자', cvThinkRedacted: '생각 (가려짐)', cvThinking: '생각하는 중…',
    cvCopy: '복사', cvCopied: '복사했습니다', cvShowAll: '전체 보기 ({n}줄)', cvLines: '{n}줄',
    cvHookCode: '훅 {name}: 종료 코드 {code}', cvHookOutcome: '훅 {name}: {outcome}', cvVhdbInject: 'VHDB 기록이 프롬프트에 붙었습니다', cvRate: '사용량 {window} {pct}% · {at} 재설정', cvRateState: '사용량 {window}: {status}', cvWin_five_hour: '5시간', cvWin_seven_day: '7일',
    cvGuardTeam: '팀 모드가 막음', cvGuardVhdb: 'VHDB가 한 번 막음', cvGuardSwbAsk: '가드가 물었지만 답할 곳이 없어 거부됨', cvGuardTip: '사용자가 거부한 것이 아니라 플러그인 규칙이 막았습니다', cvChipTeam: '팀 플러그인', cvChipVhdb: 'VHDB', cvChipTip: '이 대화에 플러그인이 연결되어 있습니다. 켜져 있는지는 이 화면에서 알 수 없습니다',
    cvRawInput: '원본 입력', cvDiffTooBig: '너무 커서 변경 내용을 비교하지 않았습니다', cvCutChars: '… {n}자에서 잘렸습니다', cvMs: '{n}ms', cvSubTools: '도구 {n}개',
    cvSessions: '대화 목록', cvYou: '나', cvHint: 'Enter 전송 · Shift+Enter 줄바꿈', cvLastTurn: '마지막 결과의 비용과 토큰', cvThemeDark: '다크 테마 (누르면 라이트)', cvThemeLight: '라이트 테마 (누르면 시스템 설정)', cvThemeAuto: '시스템 설정 따름 (누르면 다크)', cvAttach: '이미지', cvDefaultModel: '기본 모델', cvProvAnthropic: 'Claude 로그인으로 실행', cvJustNow: '방금', cvToday: '오늘',
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
    skHint: '무엇에 대한 스킬인가요? (선택)', skPropose: '이 대화로 스킬 제안', skNext: '새 스킬은 새 대화나 다시 시작한 대화부터 적용됩니다.', skStarted: '스킬 제안을 만드는 중입니다. 잠시 걸립니다.', skGatewayConfirm: '이 대화는 게이트웨이를 거쳤습니다. 제안을 만들려면 이 대화에서 내가 쓴 메시지와 성공한 명령의 한 줄 요약(게이트웨이 쪽 모델이 고른 명령 포함)을 내 Claude 계정으로 Anthropic에 보냅니다. 모델의 답변과 도구 결과는 보내지 않습니다. 제안을 승인하기 전에 꼼꼼히 읽어 주세요. 보낼까요?', skGatewayTag: '게이트웨이 대화에서 나옴',
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
    provAuthLogin: '계정 로그인 (기본 계정의 로그인이 이 주소로 전송됩니다)', provAuthHeaders: '헤더만 (키는 요청 헤더의 {key} 자리에만)', provHeaders: '요청 헤더 (선택): 한 줄에 하나, 이름: 값. 키 자리는 {key}', provKey: '키', provKeyClear: '저장된 키 지우기', provKeyKeep: '비워 두면 저장된 키를 그대로 둡니다.', provKeyNone: '키 없이 쓰는 주소는 비워 두세요.', provModel: '기본 모델 (비우면 Claude Code 기본)', provAliases: '모델 이름 바꾸기', provAliasesNote: 'Claude Code가 opus, sonnet, haiku로 부르는 모델을 이 주소의 모델 이름으로 바꿉니다. 백그라운드 작업은 haiku를 쓰므로 비워 두면 그 주소에 없는 이름을 요청할 수 있습니다.',
    provSave: '저장', provEdit: '수정', provDelete: '삭제', provDeleteConfirm: '이 제공자를 삭제할까요? 이 제공자의 대화 {n}개는 기록만 남고 더 이어 갈 수 없습니다.', provHasKey: '키 있음', provNoKey: '키 없음', provChats: '대화', provNoProviders: '아직 제공자가 없습니다.',
    provBanner: '이 대화의 내용은 {host} 로 갑니다 (Claude 계정이 아님). 기억, 스킬, 기록 검색, 자동 저장은 이 대화에 쓰이지 않습니다.', provVia: '→',
    ctrUse: '컨테이너 안에서 실행 (실험)', ctrTitle: '컨테이너 (실험)', ctrIntro: '제공자를 쓰는 대화의 claude를 docker 또는 podman 컨테이너 안에서 실행합니다. 컨테이너는 프로젝트 폴더(읽기·쓰기)와 이 대화의 설정 폴더만 보고, 홈 폴더, 데이터 폴더, Claude 계정 인증 정보는 보지 못합니다. 프로젝트 폴더 자체는 보호하지 않고 네트워크는 열려 있으며, 이 컴퓨터의 주소(localhost)는 컨테이너에서 닿지 않습니다. 이미지에는 claude가 들어 있어야 하고, 실제 docker·podman으로는 아직 검증하지 못했습니다.',
    fsOffline: '서버와 연결이 끊겨 있습니다. 다시 연결된 뒤에 시도하세요.', fsSaving: '저장하는 중입니다. 끝난 뒤에 옮기세요.', fsRealPath: '실제 파일',
    tabFiles: '파일', fsIntro: '이 대화의 프로젝트 폴더 안에서만 보고 고칩니다.', fsRefresh: '새로 고침', fsNoChat: '먼저 왼쪽 대화 탭에서 대화를 고르세요.', fsLoading: '불러오는 중…', fsEmpty: '(비어 있음)', fsTruncated: '항목이 너무 많아 일부만 보입니다.', fsUnreadable: '이름을 읽을 수 없는 항목 {n}개는 숨겼습니다.', fsOther: '열 수 없는 항목입니다 (프로젝트 밖으로 나가는 링크, 소켓 등).',
    termOpen: '터미널 (이 대화의 프로젝트 폴더)', termNoChat: '먼저 대화를 고르세요.', termTooLong: '한 번에 보낼 수 있는 입력이 너무 깁니다.', termEnded: '셸이 끝났습니다 (코드 {code}). 닫고 다시 열면 새로 시작합니다.', termOffline: '연결이 끊겨 터미널이 끝났습니다. 닫고 다시 여세요.', termLeave: '이 대화의 터미널이 끝납니다 (실행 중인 명령도 멈춥니다). 계속할까요?', termPaste: '여러 줄을 붙여 넣었습니다. 각 줄이 명령으로 바로 실행됩니다. 보낼까요?', termInt: '중단 (Ctrl+C)', termEof: '입력 끝 (Ctrl+D)', termHide: '채팅으로', termClose: '터미널 끝내기', termSend: '보내기',
    fsTag_secret: '비밀', fsTag_guarded: '확인', fsTag_blocked: '읽기 전용', fsTag_heavy: '큼', fsTag_trash: '휴지통', fsTip_trash: '삭제한 항목이 여기에 있습니다. 항목의 ⋯ 메뉴에서 복원합니다. 이 폴더 안에서 지우면 영구 삭제입니다. 이 프로젝트의 채팅이 읽을 수 있습니다.', fsTip_secret: '비밀이 들어 있을 수 있는 파일입니다. 내용은 이 화면에만 보이고 어디에도 저장되지 않습니다.', fsTip_guarded: '나중에 Claude Code 대화가 읽거나 실행하는 파일입니다. 저장하기 전에 한 번 더 확인합니다.', fsTip_blocked: '이 안의 파일은 저장할 수 없습니다 (git이 프로그램을 실행할 수 있는 곳). 보기만 됩니다.', fsTip_heavy: '큰 폴더입니다. 누를 때만 펼칩니다.',
    fsDiskReloaded: '디스크의 파일이 바뀌어 다시 불러왔습니다.', fsDiskChanged: '디스크의 파일이 바뀌었습니다. 지금 저장하면 어떻게 할지 묻습니다.',
    fsNewFile: '새 파일', fsNewFolder: '새 폴더', fsNewFileTitle: '새 파일', fsNewFolderTitle: '새 폴더', fsNamePh: '이름', fsCreate: '만들기', fsCreated: '만들었습니다.', fsMore: '이름 바꾸기 · 삭제', fsEntryIntro: '새 이름이나 새 경로를 쓰거나 삭제하세요. 폴더가 다르면 옮깁니다. 이미 있는 폴더로만 옮길 수 있고, 같은 이름이 있으면 하지 않습니다.', fsPathPh: '새 경로 (프로젝트 안)', fsTrashAsk: "'{path}'을(를) .tenrec-trash로 옮길까요? 그곳에서 복원할 수 있습니다. 휴지통에서 지울 때까지 디스크에 남아 있고, 이 프로젝트의 채팅이 읽을 수 있습니다.", fsPurgeAsk: "휴지통의 '{path}'을(를) 영구 삭제할까요? 되돌릴 수 없습니다.", fsPurge: '영구 삭제', fsPurgeTitle: '영구 삭제', fsPurgeIntro: '아래 내용을 지웁니다. 되돌릴 수 없습니다.', fsPurgeBody: "'{path}'을(를) 영구 삭제합니다: 항목 {items}개, {size}. 링크는 따라가지 않고 링크만 지웁니다.", fsEmptyTrash: '휴지통 비우기', fsEmptyBody: '휴지통의 항목 {entries}개(안의 {items}개, {size})를 영구 삭제합니다. 링크는 따라가지 않고 링크만 지웁니다.', fsPurgeLeft: '이 중 {left}개는 지울 수 없어 남습니다 (쓸 수 없는 폴더나 다른 사용자의 것).', fsPurgeGo: '영구 삭제', fsPurged: '영구 삭제했습니다.', fsPurgedLeft: '{removed}개를 지웠고 {left}개는 남겼습니다 (쓸 수 없는 폴더나 다른 사용자의 것).', fsTipTrashMenu: '휴지통 폴더는 지우지 않고 안을 비웁니다.', fsTrashed: '휴지통으로 옮겼습니다.', fsUndo: '되돌리기', fsRestore: '복원', fsRestored: '복원했습니다.', fsFrom: '원래 위치', fsRename: '이름 바꾸기', fsRenamed: '이름을 바꿨습니다.', fsDelete: '삭제', fsDeleted: '삭제했습니다.', fsDeleteAsk: "'{path}'을(를) 삭제할까요? 되돌릴 수 없습니다.", fsDeleteDirAsk: "빈 폴더 '{path}'을(를) 삭제할까요?", fsChangeConfirmBody: '이 위치는 나중에 Claude Code 대화가 읽거나 실행합니다. 그래도 바꿀까요?', fsChangeConfirm: '바꾸기',
    fsSave: '저장', fsSaved: '저장했습니다.', fsReload: '다시 불러오기', fsBack: '대화로', fsDirty: '바뀜', fsDiscard: '저장하지 않은 변경이 있습니다. 버리고 계속할까요?', fsReadonlyMixed: '줄바꿈이 섞여 있어 그대로 저장할 수 없습니다. 보기만 됩니다.', fsReadonlyBlocked: '이 파일은 저장할 수 없습니다. 보기만 됩니다.', fsGuardedNote: '이 파일은 나중에 Claude Code 대화가 읽거나 실행합니다. 저장하면 한 번 더 확인합니다.',
    fsConfirmTitle: '이 파일을 저장할까요?', fsConfirmBody: '나중에 이 폴더에서 연 Claude Code 대화가 이 파일을 읽거나 실행합니다 (설정, 훅, 환경 변수 등). 바뀌는 내용을 읽고 정하세요.', fsConfirmSave: '확인하고 저장',
    fsConflictTitle: '파일이 그사이 바뀌었습니다', fsConflictBody: 'Claude나 다른 프로그램이 이 파일을 고쳤습니다. 아래는 디스크의 내용(빨강)과 내가 쓴 내용(초록)의 차이입니다.', fsConflictReload: '디스크 내용 불러오기 (내 변경 버림)', fsConflictOverwrite: '내 내용으로 덮어쓰기', fsDiffTooBig: '파일이 커서 차이를 보여 줄 수 없습니다.',
    ctrRuntime: '실행 파일 (절대 경로, docker 또는 podman)', ctrImage: '이미지 (미리 받아 둔 것, 자동으로 받지 않습니다)', ctrReady: '컨테이너 대화를 만들 수 있습니다.', ctrNotReady: '실행 파일과 이미지를 모두 적어야 컨테이너 대화를 만들 수 있습니다.', ctrBanner: '이 대화는 컨테이너 안에서 실행됩니다.', ctrTag: '컨테이너',
    acct: '계정', acctDefault: '기본 계정 (개인)', acctNote: '고른 계정의 로그인으로 실행되고, 그 계정의 메모·스킬·대화 기록만 씁니다. 계정은 새 대화를 만들 때만 고를 수 있고 바꿀 수 없습니다.', acctView: '보는 계정', acctTag: '계정',
    acctTitle: '계정 (개인·회사 분리)', acctIntro: '로그인을 계정별로 나눕니다. 기본 계정은 이 컴퓨터의 claude가 이미 쓰는 로그인이고, 추가한 계정은 설정 폴더와 메모·스킬·대화 기록 색인이 따로 생겨 서로 보이지 않습니다. 격리가 아니라 용도 분리입니다. 같은 사용자가 실행하는 명령은 두 계정의 폴더를 모두 읽을 수 있습니다. 예약 실행은 기본 계정으로만 돕니다.',
    acctNew: '＋ 새 계정', acctKind: '종류', acctName: '이름 (예: 회사)', acctAdd: '추가', acctCheck: '로그인 상태 확인', acctLogin: '로그인 명령 복사', acctHow: '로그인 방법', acctLoginNote: '아래 명령을 터미널에서 직접 실행해 로그인하세요. 이 앱은 토큰이나 인증 코드를 보지도 저장하지도 않습니다.', acctCopied: '복사했습니다.',
    acctRename: '이름 변경', acctDelete: '삭제', acctRenamePrompt: '새 이름', acctDeleteConfirmCodex: '이 Codex 계정을 삭제할까요? 이 계정의 폴더와 로그인이 지워집니다.', acctDeleteConfirm: '이 계정을 삭제할까요? 이 계정의 로그인, 메모, 스킬, 기록 색인이 지워집니다. 이 계정의 대화 {n}개는 기록만 남고 더 이어 갈 수 없습니다.',
    acctLoggedIn: '로그인됨', acctLoggedOut: '로그인 안 됨', acctUnknown: '상태를 확인하지 못했습니다', acctChats: '대화', acctNone: '추가한 계정이 없습니다.', acctClosed: '열리지 않음 (폴더를 확인하세요)', acctDefaultRow: '기본 계정', acctChecking: '확인하는 중…',
    acctW_logout_failed: '로그아웃을 확인하지 못했습니다. claude에서 직접 로그아웃해 두세요.', acctW_folder_left: '일부 폴더를 지우지 못했습니다. 데이터 폴더에서 직접 지워야 합니다.', acctNeedsLogin: '이 계정은 아직 로그인되어 있지 않습니다. 계정 패널의 명령으로 로그인하세요.',
    wtUse: '격리된 worktree에서 시작 (새 git 브랜치)', wtUseNote: '원본 폴더의 현재 커밋에서 새 브랜치를 만듭니다. 커밋 안 된 변경과 .env 같은 무시된 파일은 따라오지 않습니다.', wtTag: 'worktree', wtGone: 'worktree 정리됨', wtBelowTop: '프로젝트 안의 하위 폴더가 아니라 worktree의 맨 위 폴더에서 시작했습니다.',
    wtBanner: 'worktree · 브랜치 {branch} ← {base} · 원본 {repo}', wtHint: '원본 폴더를 고치라는 요청은 거절하세요. 변경은 이 worktree에서만 하고, 끝나면 병합하세요.', wtCheck: '병합 점검', wtRemoveBtn: '정리', wtRemoved: 'worktree가 정리된 채팅입니다 (기록만 볼 수 있음).',
    wtSummary: '커밋 {n}개 · 파일 {f}개 +{a} −{d} · {base}가 {b}커밋 앞서 있음', wtOk: '병합할 수 있습니다.', wtMerge: '병합', wtMerged: '병합했습니다.',
    wtB_uncommitted: '이 worktree에 커밋 안 된 변경이 있습니다. 먼저 커밋하세요.', wtB_nothing: '병합할 커밋이 없습니다.', wtB_wrong_branch: '원본 폴더가 {base}가 아닌 {main} 브랜치에 있습니다. {base}로 되돌리세요.', wtB_main_dirty: '원본 폴더에 커밋 안 된 변경이 있습니다. 거기서 먼저 커밋하거나 보관하세요.', wtB_mid_merge: '원본 폴더가 병합 도중입니다. 먼저 끝내거나 취소하세요.', wtB_conflict: '충돌: {files}', wtB_missing: 'worktree 폴더가 없습니다. 정리하세요.',
    wtMergeConfirm: '{branch}의 커밋 {n}개를 {repo}의 {base} 브랜치에 병합 커밋으로 합칩니다. 원본 폴더를 편집기에서 열어 두었다면 파일 내용이 바뀝니다. 계속할까요?',
    wtRemove1: 'worktree 폴더 {path}를 지웁니다. 이 채팅은 더 이어갈 수 없습니다. 계속할까요?', wtRemoveLoss: ' 커밋 안 된 변경 {n}건이 사라집니다.', wtRemove2: '{base}에 없는 커밋 {n}개가 있습니다. 브랜치 {branch}도 지울까요? 취소하면 브랜치는 남습니다.', wtDone: 'worktree를 정리했습니다.', wtKept: ' 브랜치 {branch}는 남겼습니다 (병합되지 않은 커밋이 있음).',
    wtE_stale: '그사이 브랜치가 바뀌었습니다. 다시 점검했습니다.', wtE_blocked: '병합할 수 없는 상태입니다. 다시 점검했습니다.', wtE_conflict: '병합하면 충돌합니다. 원본 폴더는 그대로입니다.', wtE_not_owned: '이 앱이 만든 worktree가 아니어서 아무것도 건드리지 않았습니다.', wtE_busy: '턴이 실행 중입니다. 먼저 멈추세요.', wtE_uncommitted: '커밋 안 된 변경이 있습니다.', wtE_removed: '이미 정리된 worktree입니다.', wtE_abort_failed: '병합을 되돌리지 못했습니다. 원본 폴더에서 git merge --abort 를 실행하세요.',
    wtCfgIntro: '이 프로젝트의 git 설정 중 프로그램을 실행할 수 있는 것(필터, 병합·diff 드라이버, include, attributes 등)이 이 채팅을 만든 뒤 바뀌었습니다. 이 채팅의 명령이 바꿨을 수 있습니다. 확인하기 전에는 점검도 병합도 하지 않습니다.', wtCfgAccept: '이 설정을 받아들임', wtCfgConfirm: '위 설정을 읽어 보았고, 이 설정으로 git이 실행되어도 괜찮습니까?', wtCfgDone: '설정을 받아들였습니다.',
    wtRemove2b: '{base}에 같은 내용이 이미 들어 있는 것으로 보이지만 (squash 등) 커밋 {n}개는 기록에 없습니다. 브랜치 {branch}도 지울까요? 취소하면 브랜치는 남습니다.', wtW_folder_left: '폴더 일부를 지우지 못했습니다. 직접 지우세요.', wtW_branch_not_deleted: '브랜치를 지우지 못해 남겼습니다.', wtW_forgotten: '(폴더와 git 등록이 이미 없었습니다.)',
    wtE_config_changed: '프로젝트의 git 설정이 바뀌었습니다.', wtE_unexpected_result: '병합 결과가 점검한 것과 달라 취소했습니다.',
  },
  en: {
    newChat: '＋ New chat', folder: 'Project folder (absolute path)', model: 'Model', create: 'Create', cancel: 'Cancel', ncDefaultsUsed: 'Filled in from the last chat in this project.', ncDefaultsReset: 'Use defaults', ncRecent: 'Recent projects', ncTrustRemembered: 'This folder\'s project settings (hooks, commands) are remembered as trusted for new chats.', ncTrustForget: 'Forget', ncTrustForgotten: 'Forgot the trust for this folder. New chats ask again.', ncDefaultsHold: 'This project\'s settings were just filled in. Check them, then press again.', empty: 'Pick a chat on the left or start a new one.',
    placeholder: 'Message (Enter to send, Shift+Enter for a new line). Paste or drop images', send: 'Send', stop: 'Stop', attach: 'Attach image',
    idle: 'idle', running: 'working', exited: 'ended', connected: 'connected', reconnecting: 'reconnecting…', untitled: '(new chat)', acctNoChats: 'No chats under this account.',
    plugTitle: 'Plugins', plugIntro: 'Plugins the default account has and this account lacks. Each account has its own settings folder, so they are installed here separately.', plugNone: 'Nothing to install.', plugInstall: 'Install', plugBusy: 'Installing…', plugConfirm: 'Install the plugin {p} into this account? A plugin can run code. Continue?', plugDone: 'Installed', plugFailed: 'Could not install',
    chatName: 'Chat name (optional)', renameBtn: 'Rename', archiveBtn: 'Archive', unarchiveBtn: 'Unarchive', archiveShown: 'Archive', renamePrompt: 'New name (60 characters at most)', deleteBtn: 'Delete', deleteConfirm: 'Delete this chat? Its whole history is erased and this cannot be undone. Continue?',
    modelDefault: 'default model', modelNow: 'Model changed', modelNote: 'the next message continues the chat with this model',
    perm: 'Permission mode', permTag: 'permission',
    permM_default: 'Default (only reads run without asking)', permN_default: 'Only reads run without asking. Everything else asks each time.',
    permM_acceptEdits: 'Accept edits (acceptEdits)', permN_acceptEdits: 'Reads, file edits and mkdir/touch/mv/cp run without asking.',
    permM_plan: 'Plan only (plan)', permN_plan: 'Read-only planning.',
    permM_auto: 'Auto review (auto)', permN_auto: 'A classifier reviews each action. It needs a model and plan Claude Code supports; otherwise it acts as the default.',
    permM_dontAsk: 'Deny without asking (dontAsk)', permN_dontAsk: 'Only pre-approved tools run; everything else is denied without asking.',
    permM_bypassPermissions: 'Skip every check (--dangerously-skip-permissions)', permN_bypassPermissions: 'Everything runs without asking (deny/ask rules, hooks and protected-path delete checks still apply). Claude Code refuses it when run as root/sudo.',
    permBypassConfirm: 'This chat runs every tool, including file edits and commands, without asking. That includes things that cannot be undone. The permission mode cannot be changed after the chat is made. Continue?', acctAttn: 'needs you',
    allowSession: 'Allow for this run', allowAlways: 'Always allow in this project', grantSessionNote: 'Claude Code remembers it in this process. It may be gone after a restart or a model change.', grantAlwaysNote: 'Claude Code writes the rule into the project\'s .claude/settings.local.json. It applies to every chat of this folder, and this app cannot remove it yet.', grantExact: 'Allows exactly this command only.', grantWild: 'This rule contains a wildcard (*).', grantRules: 'Rule Claude suggested',
    allowOnce: 'Allow once', deny: 'Deny', denyReason: 'Reason (optional)', permTitle: 'Approve tool use', blocked: 'Blocked path', suggestions: 'Rules Claude suggested (for information, not applied)',
    allowed: 'allowed', denied: 'denied', cancelled: 'the approval request was cancelled', running_tool: 'running', done: 'done', failed: 'error', more: 'more',
    imgType: 'Only png, jpeg, gif and webp images', imgSize: 'An image can be 5 MB at most', imgCount: 'Up to 5 images', thinking: 'thinking', interrupted: 'stopped',
    noSession: 'Create or pick a chat first.', input: 'input', result: 'result',
    cvThinkSecs: '{s}s', cvThinkTok: '~{n} tokens', cvThinkSum: 'thinking · {s}s · {n} chars', cvThinkSumNoTime: 'thinking · {n} chars', cvThinkRedacted: 'thinking (hidden)', cvThinking: 'thinking…',
    cvCopy: 'copy', cvCopied: 'copied', cvShowAll: 'show all ({n} lines)', cvLines: '{n} lines',
    cvHookCode: 'hook {name}: exit code {code}', cvHookOutcome: 'hook {name}: {outcome}', cvVhdbInject: 'VHDB history was added to the prompt', cvRate: 'usage {window} {pct}% · resets {at}', cvRateState: 'usage {window}: {status}', cvWin_five_hour: '5 hours', cvWin_seven_day: '7 days',
    cvGuardTeam: 'held by team mode', cvGuardVhdb: 'held once by VHDB', cvGuardSwbAsk: 'guard asked, nobody to answer: declined', cvGuardTip: 'a plug-in rule held this, not the user', cvChipTeam: 'team plug-in', cvChipVhdb: 'VHDB', cvChipTip: 'plug-in connected to this chat; whether it is switched on is not known here',
    cvRawInput: 'raw input', cvDiffTooBig: 'too large to compare the changes', cvCutChars: '… cut at {n} characters', cvMs: '{n} ms', cvSubTools: '{n} tools',
    cvSessions: 'Chats', cvYou: 'you', cvHint: 'Enter to send · Shift+Enter for a new line', cvLastTurn: 'cost and tokens of the last result', cvThemeDark: 'dark theme (click for light)', cvThemeLight: 'light theme (click for system)', cvThemeAuto: 'follows the system (click for dark)', cvAttach: 'image', cvDefaultModel: 'default model', cvProvAnthropic: 'runs on your Claude login', cvJustNow: 'now', cvToday: 'today',
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
    skHint: 'What is the skill about? (optional)', skPropose: 'Propose a skill from this chat', skNext: 'A new skill applies from a new or restarted chat.', skStarted: 'Making a skill proposal. It takes a moment.', skGatewayConfirm: 'This chat went through a gateway. To make a proposal, your messages in it and one-line summaries of the commands that succeeded (including ones the gateway\'s model chose) are sent to Anthropic under your Claude account; replies and tool results are not. Read the proposal carefully before approving it. Send?', skGatewayTag: 'From a gateway chat',
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
    provAuthLogin: 'Account login (the default account\'s login token is sent to this address)', provAuthHeaders: 'Headers only (the key goes only where a header line says {key})', provHeaders: 'Request headers (optional): one per line, Name: value. Write {key} where the key goes', provKey: 'Key', provKeyClear: 'Clear the saved key', provKeyKeep: 'Leave empty to keep the saved key.', provKeyNone: 'Leave empty for an address that needs no key.', provModel: 'Default model (empty: Claude Code\'s own default)', provAliases: 'Rename models', provAliasesNote: 'Claude Code asks for opus, sonnet and haiku; these give the names this address uses for them. Background work uses haiku, so if it is empty, a name the address does not have may be asked for.',
    provSave: 'Save', provEdit: 'Edit', provDelete: 'Delete', provDeleteConfirm: 'Delete this provider? Its {n} chat(s) keep their history and can no longer be continued.', provHasKey: 'key set', provNoKey: 'no key', provChats: 'chats', provNoProviders: 'No providers yet.',
    provBanner: 'This chat\'s content goes to {host} (not your Claude account). Notes, skills, history search and auto-save are not used in this chat.', provVia: '→',
    ctrUse: 'Run inside a container (experimental)', ctrTitle: 'Container (experimental)', ctrIntro: 'Runs the claude of a chat that uses a provider inside a docker or podman container. The container sees the project folder (read-write) and this chat\'s own settings folder, and not your home folder, the data folder or the Claude account\'s credentials. It does not protect the project folder itself, the network is open, and an address on this computer (localhost) cannot be reached from inside. The image must contain claude. It has not been verified with a real docker or podman yet.',
    fsOffline: 'The connection to the host is down. Try again once it is back.', fsSaving: 'Saving is in progress. Move on when it is done.', fsRealPath: 'Real file',
    tabFiles: 'Files', fsIntro: 'View and edit inside this chat\'s project folder only.', fsRefresh: 'Refresh', fsNoChat: 'Pick a chat in the Chats tab first.', fsLoading: 'Loading…', fsEmpty: '(empty)', fsTruncated: 'Too many entries: only some are shown.', fsUnreadable: '{n} entries whose names cannot be read are hidden.', fsOther: 'Cannot be opened (a link that leaves the project, a socket, …).',
    termOpen: 'Terminal (this chat\'s project folder)', termNoChat: 'Pick a chat first.', termTooLong: 'That is too much input to send at once.', termEnded: 'The shell ended (code {code}). Close and open again to start a new one.', termOffline: 'The connection dropped and the terminal ended. Close and open again.', termLeave: 'This chat\'s terminal ends (a running command stops with it). Continue?', termPaste: 'Several lines were pasted. Each line runs as a command at once. Send them?', termInt: 'Interrupt (Ctrl+C)', termEof: 'End of input (Ctrl+D)', termHide: 'Back to chat', termClose: 'End terminal', termSend: 'Send',
    fsTag_secret: 'secret', fsTag_guarded: 'confirm', fsTag_blocked: 'read-only', fsTag_heavy: 'big', fsTag_trash: 'trash', fsTip_trash: 'Deleted entries are here. Restore one from its ⋯ menu. Deleting inside this folder is for good. A chat of this project can read it.', fsTip_secret: 'May hold secrets. The content is shown on this screen only and kept nowhere.', fsTip_guarded: 'A later Claude Code chat reads or runs this file. Saving asks once more.', fsTip_blocked: 'Files in here cannot be saved (git can run programs from here). View only.', fsTip_heavy: 'A big folder. Opened only when you click it.',
    fsDiskReloaded: 'The file changed on disk and was read again.', fsDiskChanged: 'The file changed on disk. Saving now asks what to do.',
    fsNewFile: 'New file', fsNewFolder: 'New folder', fsNewFileTitle: 'New file', fsNewFolderTitle: 'New folder', fsNamePh: 'Name', fsCreate: 'Create', fsCreated: 'Created.', fsMore: 'Rename · delete', fsEntryIntro: 'Type a new name or a new path, or delete it. A different folder moves it. It can only go into a folder that already exists, and never onto an existing name.', fsPathPh: 'New path (inside the project)', fsTrashAsk: "Move '{path}' to .tenrec-trash? You can restore it from there. It stays on disk until it is deleted from the trash, and a chat of this project can read it.", fsPurgeAsk: "Delete '{path}' from the trash for good? This cannot be undone.", fsPurge: 'Delete for good', fsPurgeTitle: 'Delete for good', fsPurgeIntro: 'This is deleted. It cannot be undone.', fsPurgeBody: "Deletes '{path}' for good: {items} entries, {size}. A link is removed as a link, never followed.", fsEmptyTrash: 'Empty trash', fsEmptyBody: 'Deletes {entries} entries of the trash for good ({items} items inside, {size}). A link is removed as a link, never followed.', fsPurgeLeft: '{left} of them cannot be deleted and stay (a folder that cannot be written to, or another user\'s).', fsPurgeGo: 'Delete for good', fsPurged: 'Deleted for good.', fsPurgedLeft: '{removed} deleted, {left} left (a folder that cannot be written to, or another user\'s).', fsTipTrashMenu: 'The trash folder itself is not deleted; what is in it is.', fsTrashed: 'Moved to the trash.', fsUndo: 'Undo', fsRestore: 'Restore', fsRestored: 'Restored.', fsFrom: 'Came from', fsRename: 'Rename', fsRenamed: 'Renamed.', fsDelete: 'Delete', fsDeleted: 'Deleted.', fsDeleteAsk: "Delete '{path}'? This cannot be undone.", fsDeleteDirAsk: "Delete the empty folder '{path}'?", fsChangeConfirmBody: 'A later Claude Code chat reads or runs what is in this place. Change it anyway?', fsChangeConfirm: 'Change',
    fsSave: 'Save', fsSaved: 'Saved.', fsReload: 'Reload', fsBack: 'Back to chat', fsDirty: 'changed', fsDiscard: 'There are unsaved changes. Discard them and continue?', fsReadonlyMixed: 'The line endings are mixed, so this file cannot be saved as it is. View only.', fsReadonlyBlocked: 'This file cannot be saved. View only.', fsGuardedNote: 'A later Claude Code chat reads or runs this file. Saving asks once more.',
    fsConfirmTitle: 'Save this file?', fsConfirmBody: 'A Claude Code chat opened later in this folder reads or runs this file (settings, hooks, environment variables, …). Read what changes and decide.', fsConfirmSave: 'Confirm and save',
    fsConflictTitle: 'The file changed meanwhile', fsConflictBody: 'Claude or another program changed this file. Below, the difference between what is on disk (red) and what you wrote (green).', fsConflictReload: 'Load the disk version (drop my changes)', fsConflictOverwrite: 'Overwrite with mine', fsDiffTooBig: 'The file is too big to show the difference.',
    ctrRuntime: 'Program (absolute path, docker or podman)', ctrImage: 'Image (already pulled; it is never pulled for you)', ctrReady: 'Container chats can be made.', ctrNotReady: 'Both the program and the image are needed before a container chat can be made.', ctrBanner: 'This chat runs inside a container.', ctrTag: 'container',
    acct: 'Account', acctDefault: 'Default account (personal)', acctNote: 'The chat runs with the chosen account\'s login and uses only that account\'s notes, skills and chat history. An account can only be chosen when a chat is made and cannot be changed.', acctView: 'Account shown', acctTag: 'account',
    acctTitle: 'Accounts (personal and work apart)', acctIntro: 'Logins are kept apart. The default account is the login the claude on this computer already uses; an added account gets a settings folder, notes, skills and a history index of its own, and none of one is visible from another. This keeps uses apart; it is not isolation: a command run by the same user can read both accounts\' folders. Scheduled runs use the default account only.',
    acctNew: '＋ New account', acctKind: 'Kind', acctName: 'Name (for example: Work)', acctAdd: 'Add', acctCheck: 'Check login', acctLogin: 'Copy login command', acctHow: 'How to log in', acctLoginNote: 'Run the command below in your own terminal to log in. This app never sees or stores a token or a code.', acctCopied: 'Copied.',
    acctRename: 'Rename', acctDelete: 'Delete', acctRenamePrompt: 'New name', acctDeleteConfirmCodex: 'Delete this Codex account? Its folder and login are removed.', acctDeleteConfirm: 'Delete this account? Its login, notes, skills and history index are removed. Its {n} chats stay readable and cannot be continued.',
    acctLoggedIn: 'Logged in', acctLoggedOut: 'Not logged in', acctUnknown: 'The status could not be read', acctChats: 'chats', acctNone: 'No account was added.', acctClosed: 'Not open (check its folder)', acctDefaultRow: 'Default account', acctChecking: 'Checking…',
    acctW_logout_failed: 'The logout could not be confirmed. Log out in claude yourself.', acctW_folder_left: 'Some folders could not be removed. Remove them from the data folder yourself.', acctNeedsLogin: 'This account is not logged in yet. Use the command in the Accounts panel.',
    wtUse: 'Start in an isolated worktree (new git branch)', wtUseNote: 'A new branch is made from the folder\'s current commit. Uncommitted changes and ignored files such as .env are not carried over.', wtTag: 'worktree', wtGone: 'worktree removed', wtBelowTop: 'The chat started at the top of the worktree, not in the folder inside the project.',
    wtBanner: 'worktree · branch {branch} ← {base} · original {repo}', wtHint: 'Refuse requests to change the original folder. Make changes only in this worktree, then merge.', wtCheck: 'Check merge', wtRemoveBtn: 'Clean up', wtRemoved: 'This chat\'s worktree was removed (history only).',
    wtSummary: '{n} commits · {f} files +{a} −{d} · {base} is {b} commits ahead', wtOk: 'It can be merged.', wtMerge: 'Merge', wtMerged: 'Merged.',
    wtB_uncommitted: 'This worktree has uncommitted changes. Commit them first.', wtB_nothing: 'There is nothing to merge.', wtB_wrong_branch: 'The original folder is on {main}, not {base}. Switch it back to {base}.', wtB_main_dirty: 'The original folder has uncommitted changes. Commit or stash them there first.', wtB_mid_merge: 'The original folder is in the middle of a merge. Finish or abort it first.', wtB_conflict: 'Conflicts: {files}', wtB_missing: 'The worktree folder is gone. Clean it up.',
    wtMergeConfirm: 'Merge {n} commits of {branch} into the {base} branch of {repo} as a merge commit. If you have the original folder open in an editor, its files change. Continue?',
    wtRemove1: 'The worktree folder {path} will be deleted. This chat cannot continue. Continue?', wtRemoveLoss: ' {n} uncommitted changes will be lost.', wtRemove2: '{n} commits are not in {base}. Delete the branch {branch} too? Cancel keeps the branch.', wtDone: 'The worktree was removed.', wtKept: ' The branch {branch} was kept (it has commits that were not merged).',
    wtE_stale: 'The branches changed meanwhile. Checked again.', wtE_blocked: 'The merge is blocked. Checked again.', wtE_conflict: 'The merge would conflict. The original folder is unchanged.', wtE_not_owned: 'This is not a worktree this app made, so nothing was touched.', wtE_busy: 'A turn is running. Stop it first.', wtE_uncommitted: 'There are uncommitted changes.', wtE_removed: 'That worktree was already removed.', wtE_abort_failed: 'The merge could not be undone. Run git merge --abort in the original folder.',
    wtCfgIntro: 'Settings of this project\'s git that can run a program (filters, merge and diff drivers, includes, attributes, ...) changed since this chat was made. A command run in this chat may have changed them. Nothing is checked or merged until you have looked.', wtCfgAccept: 'Accept these settings', wtCfgConfirm: 'You have read the settings above and are fine with git running with them?', wtCfgDone: 'The settings were accepted.',
    wtRemove2b: '{base} seems to hold the same content already (a squash, for example), but {n} commits are not in its history. Delete the branch {branch} too? Cancel keeps the branch.', wtW_folder_left: 'Part of the folder could not be deleted. Delete it by hand.', wtW_branch_not_deleted: 'The branch could not be deleted and was kept.', wtW_forgotten: '(The folder and git\'s record of it were already gone.)',
    wtE_config_changed: 'The project\'s git settings changed.', wtE_unexpected_result: 'The merge did not give the checked result and was undone.',
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
const SVGNS = 'http://www.w3.org/2000/svg'
/** An icon from the sprite in index.html (`<symbol id="i-name">`), drawn with DOM calls only. */
const icon = (name, cls) => {
  const svg = document.createElementNS(SVGNS, 'svg')
  svg.setAttribute('class', 'ic' + (cls ? ' ' + cls : ''))
  svg.setAttribute('aria-hidden', 'true')
  const use = document.createElementNS(SVGNS, 'use')
  use.setAttribute('href', '#i-' + name)
  svg.appendChild(use)
  return svg
}
for (const n of document.querySelectorAll('[data-i]')) n.textContent = t(n.dataset.i)
for (const n of document.querySelectorAll('[data-i-ph]')) n.placeholder = t(n.dataset.iPh)
for (const n of document.querySelectorAll('[data-i-title]')) n.title = t(n.dataset.iTitle)
$('new-toggle').setAttribute('aria-label', t('newChat'))

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
// Dark unless the person picked light or "follow the system" (the design is dark first).
const THEMES = ['dark', 'light', 'auto']
const applyTheme = mode => {
  const m = THEMES.includes(mode) ? mode : 'dark'
  document.documentElement.dataset.theme = m
  $('theme').replaceChildren(icon(m === 'dark' ? 'moon' : m === 'light' ? 'sun' : 'half'))
  $('theme').title = t(m === 'dark' ? 'cvThemeDark' : m === 'light' ? 'cvThemeLight' : 'cvThemeAuto')
  return m
}
let theme = applyTheme(store.get('tenrec-theme'))
$('theme').addEventListener('click', () => {
  theme = applyTheme(THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length])
  store.set('tenrec-theme', theme)
})

// ---- markdown ----
DOMPurify.addHook('afterSanitizeAttributes', node => {
  if (node.tagName === 'A') {
    node.setAttribute('target', '_blank')
    node.setAttribute('rel', 'noopener noreferrer')
  }
})
const markdown = source => DOMPurify.sanitize(marked.parse(source, { gfm: true, breaks: true }), { FORBID_TAGS: ['style', 'form', 'input', 'button', 'img'], FORBID_ATTR: ['style', 'id', 'name'] })

// ---- state ----
const S = { ws: null, sessions: [], cur: null, lastSeq: 0, replaying: false, state: 'idle', pending: new Map(), tools: new Map(), live: null, liveThink: null, thinkMs: null, sysTurn: new Set(), sysWin: new Map(), curSeq: 0, jumpTo: null, searchReq: 0, side: 'chats', mem: null, memRid: 0, memOps: new Map(), editing: 0, skills: null, sched: null, providers: null, pvEditing: null, scEditing: 0, scRunId: 0, skEditing: 0, skHistory: null, skDraft: null, skEditReport: null, images: [], debug: [], connected: false, wasConnected: false, acct: null, showArchived: false, accounts: null, acctStatus: new Map(), acctRid: 0, acctHow: new Set(), acctPlug: new Set(), acctPlugins: new Map(), acctPlugBusy: new Set(), acctOps: new Map(), acctReq: 0, curTs: 0, day: '', asstMeta: false, model: null }

const messages = $('messages')
const files = initFiles({ $, el, t, send: op => wsSend(op), toast: (text, action) => toast(text, action), sid: () => S.cur })
const term = initTerm({ $, t, send: op => wsSend(op), toast: text => toast(text), sid: () => S.cur })
const wt = initWt({ $, el, t, send: op => wsSend(op), toast: text => toast(text), sid: () => S.cur, beforeRemove: () => files.leave() && term.leave(null), removed: sid => markRemoved(sid) })
// A removed worktree: the chat stays to be read, and what was open on its folder is closed.
function markRemoved(sid) {
  const s = S.sessions.find(x => x.sid === sid)
  if (s?.worktree) s.worktree = { ...s.worktree, removed: true }
  renderSessions()
  if (sid === S.cur) { files.reset(); term.reset(); wt.show(s) }
}
/** True when the editor and the terminal may both be left (called before another chat is opened). */
function leaveView(target) { return files.leave() && term.leave(target) }
// Coming back to the page: a file open in the editor may have been changed while the page was away.
window.addEventListener('focus', () => files.checkDisk())
document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') files.checkDisk() })
function stick() { return messages.scrollHeight - messages.scrollTop - messages.clientHeight < 120 }
function add(node, opts = {}) {
  const follow = stick()
  $('empty')?.remove()
  // A day line or a name line is not a message: a search jump must land on the message itself.
  if (S.curSeq && opts.seq !== false) node.dataset.seq = String(S.curSeq)
  messages.appendChild(node)
  if (follow) messages.scrollTop = messages.scrollHeight
  return node
}
// A line with the day when the next message is on another day than the last one shown.
function dayMark() {
  const key = dayKey(S.curTs)
  if (!key || key === S.day) return
  S.day = key
  add(el('div', 'day', (key === dayKey(Date.now()) ? t('cvToday') : key) + ' · ' + clock(S.curTs)), { seq: false })
}
// The name line above the first block of an answer (thinking, a tool or text): who answers, and when.
function asstMeta() {
  if (S.asstMeta) return
  S.asstMeta = true
  dayMark()
  const meta = el('div', 'mmeta asst')
  const av = el('span', 'av')
  av.append(icon('bolt'))
  meta.append(av, el('span', 'who', modelLabel(S.model || curSession()?.model || '') || 'Claude'))
  if (clock(S.curTs)) meta.append(el('span', '', clock(S.curTs)))
  add(meta, { seq: false })
}
function resetView() {
  S.day = ''
  S.asstMeta = false
  S.model = null
  $('turn-pill').hidden = true
  messages.replaceChildren()
  S.tools.clear()
  S.live = null
  S.liveThink = null
  S.thinkMs = null
  S.sysTurn.clear()
  S.sysWin.clear()
  showChips({ team: false, vhdb: false })
  S.debug = []
  $('debug').replaceChildren()
}

// ---- socket ----
/** True when the message went out (a closed socket drops it, and the caller may need to know). */
function wsSend(op) {
  if (!S.ws || S.ws.readyState !== 1) return false
  // A message about notes, skills or history names the account the panels show (the host refuses one that does not).
  S.ws.send(JSON.stringify(withAccount(op, S.acct)))
  return true
}

function connect() {
  const ws = new WebSocket(`ws://${location.host}/ws`)
  ws.binaryType = 'arraybuffer'
  S.ws = ws
  ws.addEventListener('open', () => {
    ws.send(JSON.stringify({ op: 'auth', token }))
    S.connected = true
    showConn()
  })
  ws.addEventListener('message', event => {
    // A binary frame is terminal output: it goes to the terminal and nowhere else (not the debug drawer, not a log).
    if (event.data instanceof ArrayBuffer) { term.onBinary(event.data); return }
    try { onMessage(JSON.parse(event.data)) } catch (error) { debugLog('bad message: ' + error) }
  })
  ws.addEventListener('close', () => {
    S.connected = false
    showConn()
    files.disconnected()
    term.disconnected()
    setTimeout(connect, 1200)
  })
}
function showConn() {
  const c = $('conn')
  c.classList.toggle('off', !S.connected)
  c.replaceChildren(el('span', 'cdot'), el('span', 'chost', location.host), el('span', 'cstate', S.connected ? t('connected') : t('reconnecting')))
}

function onMessage(m) {
  if (typeof m.kind === 'string' && m.kind.startsWith('fs_')) { files.onMessage(m); return }
  if (typeof m.kind === 'string' && m.kind.startsWith('term_')) { term.onMessage(m); return }
  if (typeof m.kind === 'string' && m.kind.startsWith('wt_')) { wt.onMessage(m); return }
  if (m.kind === 'search_results') { showResults(m.data); return }
  if (m.kind === 'memory') { if (viewing(m.data)) { S.mem = m.data; renderMemory() } return }
  if (m.kind === 'memory_changed') { if (m.data.gone && m.data.account === S.acct) setAcct(null); else if (forAccount(m.data, S.acct)) askMemory(); return }
  if (m.kind === 'memory_result') { memResult(m.data); return }
  if (m.kind === 'memory_export') { downloadJson('memories.json', m.data); return }
  if (m.kind === 'memory_notice') { toast(acctPrefix(m.data) + m.data.text); return }
  if (m.kind === 'memory_auto') { autoSaved(m.data); return }
  if (m.kind === 'accounts') { S.accounts = m.data; renderAccounts(); return }
  if (m.kind === 'accounts_changed') { askAccounts(); return }
  if (m.kind === 'account_result') { acctResult(m.data); return }
  if (m.kind === 'account_status') { acctStatusIn(m.data); return }
  if (m.kind === 'account_plugins') { S.acctPlugins.set(m.data.account, m.data); renderAccounts(); return }
  if (m.kind === 'providers') { S.providers = m.data; renderProviders(); return }
  if (m.kind === 'providers_changed') { askProviders(); return }
  if (m.kind === 'provider_result') { pvResult(m.data); return }
  if (m.kind === 'sched') { S.sched = m.data; renderSched(); return }
  if (m.kind === 'sched_changed') { askSched(); return }
  if (m.kind === 'sched_result') { scResult(m.data); return }
  if (m.kind === 'sched_run_detail') { showRun(m.data.run); return }
  if (m.kind === 'sched_run') { scAnnounce(m.data); return }
  if (m.kind === 'skills') { if (viewing(m.data)) { S.skills = m.data; renderSkills() } return }
  if (m.kind === 'skills_changed') { if (forAccount(m.data, S.acct)) { S.skHistory = null; askSkills() } return }
  if (m.kind === 'skill_result') { skResult(m.data); return }
  if (m.kind === 'skill_versions') { S.skHistory = m.data; renderSkills(); return }
  if (m.kind === 'skill_notice') { toast(acctPrefix(m.data) + m.data.text); return }
  if (m.kind === 'skill_proposed') {
    // Shown at once, whichever account it is about; the button takes the person to that account's skills.
    toast(`${acctPrefix(m.data)}${t('skProposed')}: ${m.data.name}`, { label: t('tabSkills'), run: () => { setAcct(m.data.account ?? null); showSide('skills') } })
    if (forAccount(m.data, S.acct)) askSkills()
    return
  }
  if (m.kind === 'hello') {
    S.sessions = m.data.sessions || []
    term.available(!!m.data.terminal)
    renderSessions()
    askMemory()
    askSkills()
    askSched()
    askProviders()
    askAccounts()
    if (S.cur && S.wasConnected) { S.replaying = true; wsSend({ op: 'attach', sid: S.cur, after_seq: S.lastSeq }) }
    S.wasConnected = true
    return
  }
  if (m.kind === 'deleted') {
    S.sessions = S.sessions.filter(s => s.sid !== m.sid)
    if (S.cur === m.sid) { S.cur = null; resetView(); showHead(undefined); showAcctPill(undefined) }
    renderSessions()
    return
  }
  if (m.kind === 'project_defaults') {
    applyDefaults(m.data)
    return
  }
  if (m.kind === 'recent_projects') {
    // The folders chats were lately made in: a click fills the folder and asks what is remembered for it.
    const box = $('recent-projects')
    box.replaceChildren(...(m.data.folders || []).map(folder => {
      const b = el('button', 'ghost', baseName(folder) || folder)
      b.type = 'button'
      b.title = folder
      b.addEventListener('click', () => { $('new-cwd').value = folder; askDefaults() })
      return b
    }))
    box.hidden = !box.children.length || $('new-form').hidden
    box.setAttribute('aria-label', t('ncRecent'))
    return
  }
  if (m.kind === 'project_trust_forgot') {
    if (m.data.forgot && m.data.cwd === $('new-cwd').value.trim()) { $('new-trust-note').hidden = true; toast(t('ncTrustForgotten')) }
    return
  }
  if (m.kind === 'created') {
    S.sessions = [m.data, ...S.sessions.filter(s => s.sid !== m.sid)]
    openView(m.sid, false)
    if (m.data.notice === 'below_top') toast(t('wtBelowTop'))
    return
  }
  if (m.kind === 'attached') { S.replaying = false; syncPending(); showPerm(); doJump(); files.checkDisk(); return }
  if (m.kind === 'error' && !m.seq) {
    addNote(m.data.message, true)
    // A refused rule leaves the request waiting: its buttons come back.
    if (dialog.open) for (const b of dialog.querySelectorAll('button')) b.disabled = false
    return
  }
  if (m.kind === 'worktree_removed') { markRemoved(m.sid); if (m.sid !== S.cur) return }
  if (m.sid !== S.cur) { trackOther(m); return }
  if (typeof m.seq === 'number') { S.lastSeq = Math.max(S.lastSeq, m.seq); S.curSeq = m.seq; S.curTs = typeof m.ts === 'number' ? m.ts : 0 }
  handle(m)
}

/** The open chat's open questions, kept in its list entry (an account shown instead of its own counts it). */
function syncPending() {
  const s = curSession()
  if (s && s.pending !== S.pending.size) { s.pending = S.pending.size; renderAcctBar() }
}

function trackOther(m) {
  const s = S.sessions.find(x => x.sid === m.sid)
  if (!s) return
  if (m.kind === 'state') { s.state = m.data.state; renderSessions(); renderAcctBar() }
  // A question of a chat that is not on screen: the badge of its account shows it.
  if (m.kind === 'permission') { s.pending = (s.pending || 0) + 1; renderAcctBar() }
  if (m.kind === 'permission_resolved') { s.pending = Math.max(0, (s.pending || 0) - 1); renderAcctBar() }
}

// ---- accounts: which account the panels show ----
/** A change or a listing that names another account than the panel shows is not for this panel (a listing without the key is an error text). */
const viewing = data => !('account' in data) || forAccount(data, S.acct)
/** `[name] ` before a text about an account that is not the default one. */
const acctPrefix = data => (data && data.account ? `[${accountName(data.account, S.accounts?.accounts)}] ` : '')
const curSession = () => S.sessions.find(x => x.sid === S.cur)

/** The badge in the header of a chat that runs as an extra account: which login this chat is on, in plain sight. */
/** The badge in the header of a chat whose permission mode is not the default: the mode it runs with, in plain sight. */
function showPermPill(s) {
  const pill = $('perm-pill')
  const mode = permOf(s)
  pill.hidden = !mode
  pill.textContent = mode ? `${t('permTag')}: ${mode}` : ''
  pill.classList.toggle('danger', mode === 'bypassPermissions')
}

function showAcctPill(s) {
  showPermPill(s)
  const pill = $('acct-pill')
  pill.hidden = !(s && s.account)
  pill.textContent = s && s.account ? accountName(s.account.id, S.accounts?.accounts, s.account.name) : ''
}

/** The account whose notes, skills and history the panels and the search are about. */
function setAcct(id) {
  const changed = (id ?? null) !== (S.acct ?? null)
  S.acct = id ?? null
  S.mem = null
  S.skills = null
  if (changed) {
    // What belongs to the account that was shown is dropped: an answer to its search that is still on the way, and a draft or an
    // edit in progress (an id is only a number in each account's own file, so another account could have the same one).
    S.searchReq += 1
    S.editing = 0
    S.skEditing = 0
    S.skDraft = null
    S.skEditReport = null
    S.skHistory = null
  }
  renderAcctBar()
  if (changed) {
    renderSessions()
    askMemory()
    askSkills()
    askAcctStatus(S.acct)
    if (searchBox.value.trim()) searchBox.dispatchEvent(new Event('input'))
  }
}

// ---- sessions ----
/** Every account id the host listed (open or not), or null before the list came. */
const knownAccounts = () => (S.accounts ? S.accounts.accounts.filter(isClaudeAcct).map(a => a.id) : null)

function renderSessions() {
  const list = $('sessions')
  list.replaceChildren()
  const known = knownAccounts()
  // Only the chats of the account being shown are listed: nothing is shown merged (the badge on the others says if one needs you).
  const shown = S.sessions.filter(s => inContext(s, S.acct, known) && listed(s, S.showArchived, s.sid === S.cur))
  if (!shown.length && known && known.length) list.appendChild(el('div', 'muted', t('acctNoChats')))
  for (const s of shown) {
    const b = el('button', 'sess' + (s.sid === S.cur ? ' on' : ''))
    b.type = 'button'
    const title = el('div', 't')
    title.append(el('span', 'dot ' + s.state), el('span', 'tn', baseName(s.cwd) || s.cwd), el('span', 'when', ago(s.created, Date.now(), t('cvJustNow'))))
    const host = chatHost(s)
    const detail = el('div', 'd', (s.title || t('untitled')) + (s.model ? ' · ' + modelLabel(s.model) : ''))
    const tags = el('div', 'tags')
    if (host) tags.append(el('span', 'via', `${t('provVia')} ${host}${s.container ? ` · ${t('ctrTag')}` : ''}`))
    if (s.worktree) tags.append(el('span', 'via', s.worktree.removed ? t('wtGone') : t('wtTag')))
    // A chat under its own account needs no tag; one whose account was deleted sits under the default account and says whose it was.
    const perm = permOf(s)
    if (perm) tags.append(el('span', 'via' + (perm === 'bypassPermissions' ? ' danger' : ''), `${t('permTag')}: ${perm}`))
    if (s.account && viewOf(s, known) === null) tags.append(el('span', 'via', `${t('acctTag')}: ${accountName(s.account.id, S.accounts?.accounts, s.account.name)}`))
    b.append(title, detail)
    if (tags.childNodes.length) b.append(tags)
    b.addEventListener('click', () => { if (!leaveView(s.sid)) return; S.jumpTo = null; openView(s.sid, true) })
    list.appendChild(b)
  }
}

function openView(sid, attach) {
  // What the chat that is left was waiting for is kept in its list entry: the badge of its account counts it.
  const left = curSession()
  if (left) left.pending = S.pending.size
  S.cur = sid
  S.lastSeq = 0
  S.curSeq = 0
  S.replaying = true
  S.pending.clear()
  shownAsk = null
  resetView()
  const s = S.sessions.find(x => x.sid === sid)
  showHead(s)
  showAcctPill(s)
  // The panels follow the chat: its account's notes, skills and history are what is shown.
  setAcct(viewOf(s, knownAccounts()))
  const via = chatHost(s)
  $('prov-banner').hidden = !via
  $('prov-banner').textContent = via ? t('provBanner').replace('{host}', via) + (s.container ? ' ' + t('ctrBanner') : '') : ''
  wt.show(s)
  setState(s ? s.state : 'idle')
  renderSessions()
  if (dialog.open) dialog.close('switched')
  files.reset()
  term.reset()
  if (S.side === 'files') files.shown()
  if (attach) wsSend({ op: 'attach', sid, after_seq: 0 })
  $('input').focus()
}

// The container box is offered only for a provider a container can reach, once the program and the image are set.
function updateNewChat() {
  const p = (S.providers?.providers || []).find(x => x.id === $('new-provider').value)
  $('new-prov-note').hidden = !p
  // A chat is of an account or of a provider, not both: choosing one turns the other off.
  const picked = Boolean($('new-account').value)
  if (p) $('new-account').value = ''
  $('new-account').disabled = Boolean(p)
  $('new-provider').disabled = picked
  $('new-acct-note').hidden = !($('new-account').value)
  const offer = Boolean(p && S.providers?.container?.ready && !isLocalHost(p.host))
  $('new-ctr-row').hidden = !offer
  if (!offer) $('new-container').checked = false
  fillPerm()
}

/** The permission modes the new chat may start with: the list follows the kind of chat, and a choice that is no longer offered goes back to the default. */
function fillPerm() {
  const select = $('new-perm')
  const keep = select.value || 'default'
  const choices = permChoices()
  select.replaceChildren(...choices.map(m => Object.assign(el('option', '', t('permM_' + m)), { value: m })))
  select.value = choices.includes(keep) ? keep : 'default'
  $('new-perm-note').textContent = t('permN_' + select.value)
}
$('new-perm').addEventListener('change', () => { $('new-perm-note').textContent = t('permN_' + $('new-perm').value) })
$('new-provider').addEventListener('change', updateNewChat)
$('new-account').addEventListener('change', updateNewChat)
$('new-toggle').addEventListener('click', () => {
  $('new-form').hidden = !$('new-form').hidden
  // A form opened again starts on the Claude account: a chat is never put on a third party by what was chosen last time.
  // The account shown is the one a new chat starts on (when it can be chosen).
  // The permission mode starts on the default every time; the project's remembered settings (askDefaults, below) may fill in a mode, never
  // the one that skips every check: a chat never skips its checks because the last one did.
  for (const field of unfill(newFilled, newTouched)) newStart[field]()
  newTouched.clear()
  newFilled = new Set()
  newFilledAt = 0
  $('new-trust-note').hidden = true
  if (!$('new-form').hidden) wsSend({ op: 'recent_projects' })
  $('new-defaults-note').hidden = true
  $('new-perm').replaceChildren()
  $('new-name').value = ''
  if (!$('new-form').hidden) { $('new-provider').value = ''; $('new-account').value = [...$('new-account').options].some(o => o.value === (S.acct || '')) ? (S.acct || '') : ''; $('new-worktree').checked = false }
  updateNewChat()
  $('new-cwd').value = store.get('tenrec-cwd') || ''
  $('new-cwd').focus()
  askDefaults()
})
// What the form takes from what the core remembers for the folder: the fields the person set by hand in this form stay as they are.
const newTouched = new Set()
// The fields the form filled in itself for the folder it last asked about, and how each goes back to where the form starts.
let newFilled = new Set()
// When the form last changed fields by itself (ms; 0: not since it was opened): a create right after is held back, the person has not seen it.
let newFilledAt = 0
const newStart = { model: () => { $('new-model').value = '' }, account: () => { if (!$('new-provider').value) $('new-account').value = [...$('new-account').options].some(o => o.value === (S.acct || '')) ? (S.acct || '') : '' }, permission: () => { $('new-perm').value = 'default' }, worktree: () => { $('new-worktree').checked = false } }
for (const [id, field] of [['new-model', 'model'], ['new-account', 'account'], ['new-perm', 'permission'], ['new-worktree', 'worktree']]) $(id).addEventListener('change', () => newTouched.add(field))
function askDefaults() {
  const cwd = $('new-cwd').value.trim()
  // The account selected now is the one whose remembered trust the answer reports.
  if (cwd && !$('new-form').hidden) wsSend({ op: 'project_defaults', cwd, account: $('new-account').value || null })
}
$('new-cwd').addEventListener('change', askDefaults)
// Another account may have its own remembered trust for the folder.
$('new-account').addEventListener('change', askDefaults)
function applyDefaults(answer) {
  // An answer for a folder that is not the one in the field any more is a late one: it changes nothing.
  if ($('new-form').hidden || answer.cwd !== $('new-cwd').value.trim()) return
  const accountBefore = $('new-account').value
  // A trust remembered for the folder is said, with the way to forget it.
  $('new-trust-note').hidden = answer.trusted !== true
  // What was filled in for the folder before goes back to the start first, so a folder with nothing remembered does not keep the last one's settings.
  const current = { model: $('new-model').value, account: $('new-account').value, permission: $('new-perm').value, worktree: $('new-worktree').checked }
  const reverted = unfill(newFilled, newTouched)
  for (const field of reverted) newStart[field]()
  const have = { models: [...$('new-model').options].map(o => o.value), accounts: [...$('new-account').options].map(o => o.value), perms: permChoices() }
  const fill = formFill(answer, $('new-cwd').value.trim(), newTouched, have)
  // A chat is of an account or of a provider: an account is only filled in while no provider is chosen.
  if (fill.account !== undefined && !$('new-provider').value) $('new-account').value = fill.account
  updateNewChat()
  if (fill.worktree !== undefined) { $('new-worktree').checked = fill.worktree; if (fill.worktree) $('new-container').checked = false }
  fillPerm()
  if (fill.permission !== undefined) { $('new-perm').value = fill.permission; $('new-perm-note').textContent = t('permN_' + fill.permission) }
  if (fill.model !== undefined) $('new-model').value = fill.model
  newFilled = new Set(Object.keys(fill))
  fillPerm()
  if (reverted.length || fillDiffers(fill, current)) newFilledAt = Date.now()
  $('new-defaults-note').hidden = newFilled.size === 0
  // Another account was filled in: its own remembered trust is what the note should say.
  if ($('new-account').value !== accountBefore) askDefaults()
}
$('new-trust-forget').addEventListener('click', () => {
  const cwd = $('new-cwd').value.trim()
  if (cwd) wsSend({ op: 'project_trust_forget', cwd, account: $('new-account').value || null })
})
$('new-defaults-reset').addEventListener('click', () => {
  $('new-model').value = ''
  $('new-account').value = [...$('new-account').options].some(o => o.value === (S.acct || '')) ? (S.acct || '') : ''
  $('new-worktree').checked = false
  updateNewChat()
  $('new-perm').value = 'default'
  $('new-perm-note').textContent = t('permN_default')
  for (const field of ['model', 'account', 'permission', 'worktree']) newTouched.add(field)
  newFilled = new Set()
  newFilledAt = 0
  $('new-defaults-note').hidden = true
})
$('new-worktree').addEventListener('change', () => { if ($('new-worktree').checked) $('new-container').checked = false; fillPerm() })
$('new-container').addEventListener('change', () => { if ($('new-container').checked) { $('new-worktree').checked = false; newTouched.add('worktree') } fillPerm() })
$('new-cancel').addEventListener('click', () => { $('new-form').hidden = true })
$('new-form').addEventListener('submit', event => {
  event.preventDefault()
  const cwd = $('new-cwd').value.trim()
  const mode = $('new-perm').value || 'default'
  // Settings the form filled in a moment ago have not been seen yet (a press on this button takes the focus from the folder field, which fills the form).
  if (cwd && justFilled(newFilledAt, Date.now())) { newFilledAt = 0; toast(t('ncDefaultsHold')); return }
  // Skipping every check is only sent after the person said yes to it (asked before anything is left), and says so in the request too.
  if (cwd && permNeedsConfirm(mode) && !confirm(t('permBypassConfirm'))) return
  if (!cwd || !leaveView()) return
  store.set('tenrec-cwd', cwd)
  wsSend({ op: 'new', cwd, name: $('new-name').value.trim() || undefined, permission: mode === 'default' ? undefined : mode, confirm_bypass: permNeedsConfirm(mode) ? true : undefined, model: $('new-model').value || undefined, provider: $('new-provider').value || undefined, account: $('new-account').value || undefined, container: $('new-container').checked || undefined, worktree: $('new-worktree').checked || undefined })
  $('new-form').hidden = true
})

function setState(state) {
  S.state = state
  const pill = $('pill')
  pill.textContent = t(state)
  pill.className = 'pill ' + state
  $('send').disabled = state === 'running'
  $('chat-model').disabled = state === 'running'
  $('stop').hidden = state !== 'running'
  const s = S.sessions.find(x => x.sid === S.cur)
  if (s) { s.state = state; renderSessions() }
  showFoot(s)
}

// ---- events ----
function handle(m) {
  const d = m.data
  switch (m.kind) {
    case 'user': S.sysTurn.clear(); titleFrom(d.text); return addUser(d)
    case 'claude': return handleClaude(d)
    case 'permission': S.pending.set(d.request_id, d); syncPending(); if (!S.replaying) showPerm(); return
    case 'permission_resolved': S.pending.delete(d.request_id); syncPending(); showPerm(); if (!S.replaying) addNote(t(d.outcome)); return
    case 'state': return setState(d.state)
    case 'renamed': {
      const cur = curSession()
      if (cur) { cur.title = d.title; $('title').textContent = d.title; renderSessions() }
      return
    }
    case 'archived': {
      const cur = curSession()
      if (cur) { cur.archived = d.archived || undefined; showModelPicker(cur); renderSessions() }
      return
    }
    case 'model': {
      const cur = curSession()
      if (cur) { cur.model = d.model || undefined; showModelPicker(cur) }
      return addNote(`${t('modelNow')}: ${d.model || t('modelDefault')} — ${t('modelNote')}`)
    }
    case 'memory_used': return addNote(`${t('memUsed')}: ${d.ids.length} (${fmtBytes(d.bytes)})`)
    case 'skills_given': return addNote(`${t('skGiven')}: ${d.names.join(', ')}`)
    case 'stderr': debugLog(d.text); return addNote(String(d.text).trim().slice(-300), true)
    case 'error': return addNote(d.message, true)
    default: debugLog(JSON.stringify(m).slice(0, 300))
  }
}

// The header: the project folder in a PWD pill, then the chat's title. The sidebar's foot and the composer show its model.
/** The model picker in the header: for a chat of the Claude account (a provider chat's models are its provider's), only between turns. */
function showModelPicker(s) {
  const pick = $('chat-model')
  pick.hidden = !s || Boolean(s.provider)
  $('chat-rename').hidden = $('chat-delete').hidden = $('chat-archive').hidden = !s
  if (!s) return
  pick.replaceChildren(...modelChoices(s.model || '').map(m => Object.assign(el('option', '', m || t('modelDefault')), { value: m })))
  pick.value = s.model || ''
  pick.disabled = S.state === 'running'
  pick.title = t('modelNote')
  $('chat-rename').textContent = '✎ ' + t('renameBtn')
  $('chat-archive').textContent = s.archived ? t('unarchiveBtn') : t('archiveBtn')
  $('chat-archive').disabled = !s.archived && S.state === 'running'
  $('chat-delete').textContent = '🗑 ' + t('deleteBtn')
}
$('chat-rename').addEventListener('click', () => {
  const s = curSession()
  const name = s && prompt(t('renamePrompt'), s.title || '')
  if (name && name.trim()) wsSend({ op: 'rename', sid: S.cur, title: name.trim() })
})
// Put the open chat away from the list, or bring it back (the chat refuses to be put away while a turn runs or a question waits).
$('chat-archive').addEventListener('click', () => {
  const s = curSession()
  if (s) wsSend({ op: s.archived ? 'unarchive' : 'archive', sid: S.cur })
})
$('archive-toggle').addEventListener('click', () => {
  S.showArchived = !S.showArchived
  $('archive-toggle').setAttribute('aria-pressed', String(S.showArchived))
  $('archive-toggle').classList.toggle('on', S.showArchived)
  renderSessions()
})
$('chat-delete').addEventListener('click', () => {
  if (S.cur && S.state !== 'running' && confirm(t('deleteConfirm'))) wsSend({ op: 'delete', sid: S.cur })
})
$('chat-model').addEventListener('change', () => {
  if (S.cur) wsSend({ op: 'set_model', sid: S.cur, model: $('chat-model').value || 'default' })
})

function showHead(s) {
  $('title').textContent = s ? (s.title || baseName(s.cwd)) : ''
  $('pwd').hidden = !s
  $('pwd-sep').hidden = !s
  $('pwd-path').textContent = s ? tailPath(s.cwd) : ''
  $('pwd').title = s ? s.cwd : ''
  showModelPicker(s)
  showFoot(s)
}
function showFoot(s) {
  const model = s ? (S.model || s.model) : null
  const name = model ? modelLabel(model) : t('cvDefaultModel')
  $('foot-model').textContent = name
  const host = s ? chatHost(s) : null
  $('foot-prov').textContent = host ? `${t('provVia')} ${host}` : s && s.account ? `${t('acctTag')}: ${accountName(s.account.id, S.accounts?.accounts, s.account.name)}` : t('cvProvAnthropic')
  $('foot-tag').hidden = !s || s.state !== 'running'
  $('foot-tag').textContent = 'ACTIVE'
  $('model-chip').hidden = !s
  $('model-chip').textContent = name
}

// The first message names the chat, as the server does.
function titleFrom(text) {
  const s = S.sessions.find(x => x.sid === S.cur)
  if (!s || s.title || !text) return
  s.title = text.slice(0, 60)
  $('title').textContent = s.title
  renderSessions()
}

function handleClaude(d) {
  switch (d.type) {
    case 'stream_event': {
      if (S.replaying || d.parent_tool_use_id) return
      const ev = d.event || {}
      const delta = ev.delta || {}
      if (ev.type === 'content_block_start' && ev.content_block && ev.content_block.type === 'thinking') startLiveThink()
      else if (ev.type === 'content_block_delta' && delta.type === 'thinking_delta' && typeof delta.thinking === 'string') appendLiveThink(delta.thinking, delta.estimated_tokens)
      else if (ev.type === 'content_block_stop') stopLiveThink()
      else if (ev.type === 'content_block_delta' && delta.type === 'text_delta') appendLive(delta.text)
      return
    }
    case 'assistant': {
      const parentId = d.parent_tool_use_id || ''
      // A sub-agent's event leaves the main answer's live text alone (and has no thinking time of the main one to take).
      if (!parentId) endLive()
      const thinkMs = parentId ? null : S.thinkMs
      if (!parentId) S.thinkMs = null
      for (const block of (d.message && d.message.content) || []) {
        if (block.type === 'text' && block.text) addAssistant(block.text, parentId)
        else if (block.type === 'tool_use') addTool(block, parentId)
        else if (block.type === 'thinking') addThinking(block.thinking || '', thinkMs, parentId)
        else if (block.type === 'redacted_thinking') addThinking(null, null, parentId)
      }
      return
    }
    case 'user': {
      for (const block of (d.message && d.message.content) || []) if (block && block.type === 'tool_result') setToolResult(block)
      return
    }
    case 'result': {
      if (!S.replaying) files.checkDisk()
      endLive()
      S.thinkMs = null
      S.asstMeta = false
      const turn = turnLine(d)
      if (turn) {
        const pill = $('turn-pill')
        pill.replaceChildren()
        if (turn.cost) pill.append(el('span', 'cost', turn.cost))
        if (turn.cost && turn.tokens) pill.append(el('span', 'dim', '·'))
        if (turn.tokens) pill.append(el('span', '', turn.tokens + ' tkn'))
        pill.title = t('cvLastTurn')
        pill.hidden = false
      }
      const line = fmtTurn(d)
      if (d.subtype === 'error_during_execution') addNote(t('interrupted') + (line ? ' · ' + line : ''))
      else if (d.is_error) addNote(String(d.result || d.subtype), true)
      else if (line) addNote(line)
      return
    }
    case 'system':
      if (d.subtype === 'init') {
        if (d.model) { $('title').title = d.model; S.model = d.model; showFoot(curSession()) }
        showChips(initChips(d))
      }
      sysNote(d)
      return debugLog(`${d.type}/${d.subtype || ''}`)
    case 'rate_limit_event':
      sysNote(d)
      return debugLog(d.type)
    default:
      debugLog(JSON.stringify(d).slice(0, 300))
  }
}

function addUser(d) {
  S.asstMeta = false
  dayMark()
  const meta = el('div', 'mmeta user')
  meta.append(el('span', 'who', t('cvYou')))
  if (clock(S.curTs)) meta.append(el('span', '', clock(S.curTs)))
  add(meta, { seq: false })
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
  // Other attached files: a line each (name and size; the log never holds their content).
  if (d.files && d.files.length) {
    for (const f of d.files) box.appendChild(el('div', 'filechip', `\u{1F4CE} ${String(f.name || '').slice(0, 100)} (${f.size >= 1048576 ? (f.size / 1048576).toFixed(1) + ' MB' : f.size >= 1024 ? (f.size / 1024).toFixed(1) + ' KB' : f.size + ' B'})`))
  }
  if (d.text) box.appendChild(el('div', '', d.text))
  add(box)
}

let liveFrame = 0
function appendLive(text) {
  if (!S.live) { asstMeta(); S.live = { buf: '', node: add(el('div', 'msg asst live')) } }
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
  endLiveThink()
}

// Thinking as it streams: the text only ever goes in as textContent (it is the model's, not ours).
let thinkFrame = 0
const thinkLabel = (chars, ms) => {
  const s = thinkSummary(chars, ms)
  // The CLI may send the block with no text (only a token estimate): the line then says it was hidden, and how long it took.
  if (!s.chars) return t('cvThinkRedacted') + (s.secs == null ? '' : ' · ' + wtText(t('cvThinkSecs'), { s: s.secs }))
  return s.secs == null ? wtText(t('cvThinkSumNoTime'), { n: s.chars }) : wtText(t('cvThinkSum'), { s: s.secs, n: s.chars })
}
function startLiveThink() {
  if (S.liveThink) return
  asstMeta()
  const node = el('details', 'think live')
  node.open = true
  const body = el('div', 'tbody')
  node.append(el('summary', '', t('cvThinking')), body)
  S.liveThink = { buf: '', node, body, t0: Date.now(), ms: null }
  add(node)
}
function appendLiveThink(text, tokens) {
  if (!S.liveThink) startLiveThink()
  S.liveThink.buf += text
  if (Number.isFinite(tokens) && tokens > 0) S.liveThink.tokens = tokens
  if (!thinkFrame) thinkFrame = requestAnimationFrame(() => {
    thinkFrame = 0
    const lt = S.liveThink
    if (!lt || lt.ms != null) return
    const follow = stick()
    lt.node.firstChild.textContent = t('cvThinking') + (lt.tokens ? ' · ' + wtText(t('cvThinkTok'), { n: lt.tokens }) : '')
    lt.body.textContent = lt.buf
    lt.body.scrollTop = lt.body.scrollHeight
    if (follow) messages.scrollTop = messages.scrollHeight
  })
}
function stopLiveThink() {
  const lt = S.liveThink
  if (!lt || lt.ms != null) return
  lt.ms = Date.now() - lt.t0
  lt.node.classList.remove('live')
  lt.node.open = false
  lt.node.firstChild.textContent = thinkLabel(lt.buf.length, lt.ms)
  lt.body.textContent = lt.buf
}
function endLiveThink() {
  if (thinkFrame) { cancelAnimationFrame(thinkFrame); thinkFrame = 0 }
  const lt = S.liveThink
  if (!lt) return
  S.thinkMs = lt.ms ?? Date.now() - lt.t0
  lt.node.remove()
  S.liveThink = null
}

function addAssistant(text, parentId) {
  if (!parentId) asstMeta()
  const box = el('div', 'msg asst')
  box.innerHTML = markdown(text)
  decorateCode(box)
  place(box, parentId)
}

// A language tag, a copy button and a fold for long blocks, put on after the text was sanitised (DOM calls only, no markup is parsed here).
function decorateCode(box) {
  for (const code of box.querySelectorAll('pre > code')) {
    const pre = code.parentElement
    const info = foldInfo(code.textContent)
    const head = el('div', 'cb-head')
    const copy = el('button', 'cb-copy', t('cvCopy'))
    copy.type = 'button'
    copy.onclick = async () => {
      try { await navigator.clipboard.writeText(code.textContent); toast(t('cvCopied'), undefined, 2000) } catch { /* no clipboard here */ }
    }
    const lang = codeLang(code.className)
    const ic = el('span', 'cb-ic')
    ic.append(icon('code'))
    copy.prepend(icon('copy'))
    head.append(ic, el('span', 'cb-lang', lang), el('span', 'cb-lines', wtText(t('cvLines'), { n: info.lines })), copy)
    // One frame for the bar and the code; the lines are numbered by CSS, and the text (what is copied) stays the same.
    const frame = el('div', 'cb')
    pre.before(frame)
    frame.append(head, pre)
    const lines = codeLines(code.textContent, lang)
    if (lines) {
      code.replaceChildren(...lines.map(l => el('span', 'ln' + (l.kind ? ' ' + l.kind : ''), l.text + '\n')))
      pre.classList.add('lined')
    }
    if (info.folded) {
      pre.classList.add('cb-fold')
      const more = el('button', 'cb-more', wtText(t('cvShowAll'), { n: info.lines }))
      more.type = 'button'
      more.onclick = () => { pre.classList.remove('cb-fold'); more.remove() }
      pre.after(more)
    }
  }
}

// Inside the card of the sub-agent that made it, else on its own with an indent (an old history, or the parent card is not there).
function place(node, parentId) {
  const parent = parentId ? S.tools.get(parentId) : null
  if (!parent || !parent.kids) {
    if (parentId) node.classList.add('sub')
    return add(node)
  }
  const follow = stick()
  if (S.curSeq) node.dataset.seq = String(S.curSeq)
  parent.kids.appendChild(node)
  if (follow) messages.scrollTop = messages.scrollHeight
  return node
}

// No text (hidden by the model, or not sent by the CLI): only the fact is shown, never any data.
function addThinking(text, ms, parentId) {
  if (!parentId) asstMeta()
  const box = el('details', 'think')
  if (!text) box.append(el('summary', '', thinkLabel(0, ms)))
  else box.append(el('summary', '', thinkLabel(text.length, ms)), el('div', 'tbody', text))
  place(box, parentId)
}

// ---- tools and images ----
function addTool(block, parentId) {
  const known = S.tools.get(block.id)
  if (!parentId && !known) asstMeta()
  const card = el('details', 'tool')
  const head = el('summary')
  const status = el('span', 'st', t('running_tool'))
  const badge = toolBadge(block.name, block.input)
  const th = toolHead(block.name, block.input)
  head.append(el('span', 'name', block.name))
  if (badge) head.append(el('span', 'tbadge ' + badge.kind, badge.text))
  head.append(el('span', 'sum', th ? th.text || th.extra : toolSummary(block.name, block.input)))
  if (th && th.text && th.extra) head.append(el('span', 'extra', th.extra))
  const diff = editDiff(block.name, block.input)
  if (diff) {
    const stat = el('span', 'dstat')
    stat.append(el('span', 'a', `+${diff.add}`), ' ', el('span', 'd', `−${diff.del}`))
    head.append(stat)
  }
  let kids = null
  let kcount = null
  if (isAgentCall(block.name)) { kids = el('div', 'kids'); kcount = el('span', 'kcount'); head.append(kcount) }
  head.append(status)
  const body = el('div', 'body')
  const raw = el('pre', '', JSON.stringify(block.input, null, 2))
  if (diff) {
    body.append(diff.rows ? diffRows(diff.rows) : el('div', 'muted', t('cvDiffTooBig')))
    const more = el('details', 'raw')
    more.append(el('summary', '', t('cvRawInput')), raw)
    body.append(more)
  } else if (block.name === 'TodoWrite' && todoItems(block.input).length) {
    const list = el('ul', 'todo')
    for (const it of todoItems(block.input)) list.append(el('li', 'st-' + it.status, it.text))
    body.append(list)
  } else body.append(el('div', 'muted', t('input')), raw)
  const result = el('div')
  const shots = el('div', 'imgs')
  body.append(result, shots)
  if (kids) body.append(kids)
  card.append(head, body)
  if (known) {
    if (kids && known.kids) kids.append(...known.kids.childNodes)
    known.card.replaceWith(card)
  } else place(card, parentId)
  const count = known ? known.count : 0
  S.tools.set(block.id, { card, status, result, shots, kids, kcount, count, t0: known ? known.t0 : S.replaying ? null : Date.now() })
  if (kcount && count) kcount.textContent = wtText(t('cvSubTools'), { n: count })
  const parent = !known && parentId ? S.tools.get(parentId) : null
  if (parent && parent.kcount) {
    parent.count += 1
    parent.kcount.textContent = wtText(t('cvSubTools'), { n: parent.count })
  }
  for (const path of imagePaths(block.input)) loadShot(path, shots, card)
}

// The changed lines of an edit, drawn like a skill's diff (text only).
function diffRows(rows) {
  const box = el('div', 'skdiff tdiff')
  for (const r of rows) box.append(el('div', 'line' + (r.t === '+' ? ' add' : r.t === '-' ? ' del' : r.t === '…' ? ' gap' : ''), r.t === '…' ? '…' : r.t + ' ' + r.text))
  return box
}

function setToolResult(block) {
  const tool = S.tools.get(block.tool_use_id)
  if (!tool) return
  tool.card.dataset.resultSeq = String(S.curSeq)
  const guard = block.is_error ? guardKind(block.content) : null
  tool.card.classList.add(block.is_error ? 'err' : 'done')
  if (guard) {
    // Held by a plug-in rule (team mode or VHDB): said so, not shown as a plain failure.
    tool.card.classList.add('guard', 'guard-' + guard)
    tool.status.textContent = t(guard === 'team' ? 'cvGuardTeam' : guard === 'swb_ask' ? 'cvGuardSwbAsk' : 'cvGuardVhdb')
    tool.status.title = t('cvGuardTip')
  } else tool.status.textContent = (block.is_error ? t('failed') : t('done')) + (!S.replaying && tool.t0 && Date.now() - tool.t0 >= 100 ? ' · ' + wtText(t('cvMs'), { n: Date.now() - tool.t0 }) : '')
  tool.result.replaceChildren()
  for (const part of resultBlocks(block.content)) {
    if (part.type === 'text') {
      if (!tool.result.childNodes.length) tool.result.appendChild(el('div', 'muted', t('result')))
      const fold = foldText(part.text)
      const pre = el('pre', '', fold.folded ? fold.head : fold.full)
      tool.result.appendChild(pre)
      if (fold.folded) {
        const more = el('button', 'res-more', wtText(t('cvShowAll'), { n: fold.lines }))
        more.type = 'button'
        more.onclick = () => { pre.textContent = fold.full; more.remove() }
        tool.result.appendChild(more)
      }
      if (fold.cut) tool.result.appendChild(el('div', 'muted', wtText(t('cvCutChars'), { n: 20000 })))
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

// The few system events that matter get one thin line (see systemNote); each is shown once per turn (a hook) or per level (usage).
function sysNote(d) {
  const n = systemNote(d)
  if (!n) return
  if (n.scope === 'window') { if (S.sysWin.get(n.vars.window) === n.key) return; S.sysWin.set(n.vars.window, n.key) }
  else { if (S.sysTurn.has(n.key)) return; S.sysTurn.add(n.key) }
  const win = n.vars.window ? (TEXT[LANG]['cvWin_' + n.vars.window] ?? n.vars.window) : ''
  const at = n.vars.at ? new Date(n.vars.at * 1000).toLocaleTimeString() : ''
  const text = n.code === 'rate' ? (n.vars.pct == null ? wtText(t('cvRateState'), { window: win, status: n.vars.status }) : wtText(t('cvRate'), { window: win, pct: n.vars.pct, at }))
    : n.code === 'vhdb' ? t('cvVhdbInject')
    : n.code === 'hook' ? wtText(t('cvHookCode'), n.vars) : wtText(t('cvHookOutcome'), n.vars)
  add(el('div', 'note sys' + (n.level === 'warn' ? ' warn' : ''), text))
}

// Chips for the plug-ins the chat is wired to (from system/init; the last one wins, and a replay restores it).
function showChips(c) {
  const team = $('chip-team')
  const vhdb = $('chip-vhdb')
  team.hidden = !c.team
  vhdb.hidden = !c.vhdb
  team.textContent = t('cvChipTeam')
  vhdb.textContent = t('cvChipVhdb')
  team.title = vhdb.title = t('cvChipTip')
}

// ---- debug drawer ----
function debugLog(line) {
  S.debug.push(line)
  if (S.debug.length > 300) S.debug.shift()
  const box = $('debug')
  if (!box.hidden) box.textContent = S.debug.join('\n')
}
// The foot's links open the accounts or providers box in the chats pane.
for (const [button, box] of [['open-acct', 'acct-box'], ['open-prov', 'prov-box']]) {
  $(button).addEventListener('click', () => { showSide('chats'); $(box).open = true; $(box).scrollIntoView({ block: 'start', behavior: 'smooth' }) })
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
  // A rule to remember is answered only when the host accepts it: the request stays on the screen (buttons off) until it is resolved,
  // and a refusal turns them on again.
  const grantAnswer = grant => {
    wsSend({ op: 'permission', sid: S.cur, request_id: first.request_id, allow: true, grant })
    for (const b of dialog.querySelectorAll('button')) b.disabled = true
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
  const grants = grantButtons(first)
  if (grants.length) {
    const rows = el('div', 'grants')
    rows.append(el('div', 'sugg', t('grantRules')))
    for (const g of grants) {
      const b = el('button', '', `${g.grant === 'always' ? t('allowAlways') : t('allowSession')}: ${g.rule}`)
      b.type = 'button'
      b.dataset.grant = g.grant
      b.title = b.textContent
      b.addEventListener('click', () => grantAnswer(g.grant))
      rows.append(b, el('div', 'muted', g.grant === 'always' ? t('grantAlwaysNote') : t('grantSessionNote')))
    }
    rows.append(el('div', 'muted', grants[0].exact ? t('grantExact') : t('grantWild')))
    dialog.append(actions, rows)
  } else {
    dialog.append(actions)
  }
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
  if (!(hit.sid === S.cur && !S.replaying) && !leaveView(hit.sid)) return
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
  for (let up = node.parentElement && node.parentElement.closest('details'); up; up = up.parentElement && up.parentElement.closest('details')) up.open = true
  node.scrollIntoView({ block: 'center' })
  node.classList.remove('hit')
  void node.offsetWidth
  node.classList.add('hit')
}

// ---- memory ----
// The Memory panel's "this project": a worktree chat shares the notes of the folder it was made from (the server drops a Windows `\\?\` prefix).
const curCwd = () => { const s = S.sessions.find(x => x.sid === S.cur) || {}; return (s.worktree && s.worktree.repo) || s.cwd }
function askMemory() { wsSend({ op: 'memory_list', cwd: curCwd() }) }
// Every request carries an id the answer brings back, so two requests in flight are never mixed up.
function memSend(op, tag) {
  S.memRid += 1
  // The account is fixed when the message is made: an answer comes back to the account it was asked of, even if the panel moved on.
  const full = withAccount({ ...op, rid: S.memRid }, S.acct)
  S.memOps.set(S.memRid, tag ? { op: tag, account: full.account } : full)
  wsSend(full)
}

function showSide(name) {
  S.side = name
  $('chats').hidden = name !== 'chats'
  $('memory').hidden = name !== 'memory'
  $('skills').hidden = name !== 'skills'
  $('sched').hidden = name !== 'sched'
  $('files').hidden = name !== 'files'
  $('tab-files').classList.toggle('on', name === 'files')
  $('tab-sched').classList.toggle('on', name === 'sched')
  $('tab-chats').classList.toggle('on', name === 'chats')
  $('tab-memory').classList.toggle('on', name === 'memory')
  $('tab-skills').classList.toggle('on', name === 'skills')
  if (name === 'memory') askMemory()
  if (name === 'skills') askSkills()
  if (name === 'sched') askSched()
  if (name === 'files') files.shown()
  renderAcctBar()
}
$('tab-chats').addEventListener('click', () => showSide('chats'))
$('tab-memory').addEventListener('click', () => showSide('memory'))
$('tab-skills').addEventListener('click', () => showSide('skills'))
$('tab-sched').addEventListener('click', () => showSide('sched'))
$('tab-files').addEventListener('click', () => showSide('files'))

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
function autoSaved(data) {
  const items = data.items || []
  if (!items.length) return
  const text = items.length === 1 ? items[0].text : `${items.length}`
  toast(`${acctPrefix(data)}${t('memAutoSaved')}: ${text}`, {
    label: t('memUndo'),
    run: () => { for (const m of items) memSend({ op: 'memory_delete', id: m.id, account: data.account ?? null }, 'memory_delete_quiet') },
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
    toast(t('memDeleted').split(' (')[0], { label: t('memUndo'), run: () => wsSend({ op: 'memory_undo', id: d.undo, account: last.account ?? null }) })
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
  if (!(m.sid === S.cur && !S.replaying) && !leaveView(m.sid)) return
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
  const full = withAccount({ ...op, rid: S.memRid }, S.acct)
  S.memOps.set(S.memRid, tag ? { op: tag, account: full.account } : full)
  wsSend(full)
}

function skResult(d) {
  const last = S.memOps.get(d.rid)
  S.memOps.delete(d.rid)
  if (!d.ok) {
    // A chat that went through a gateway goes to the account's run only after the person said so: the same request is sent again.
    if (d.needs_confirm === 'gateway' && last && last.op === 'skill_propose') {
      if (confirm(t('skGatewayConfirm'))) skSend({ ...last, confirm_gateway: true })
      return
    }
    if (d.error) toast(d.error)
    // A refused approval brings the findings of the text that would have been written.
    if (last && last.op === 'skill_approve' && d.risks) { S.skEditReport = { id: d.id, risks: d.risks }; renderSkills() }
    return
  }
  if (last && last.op === 'skill_propose') toast(t('skStarted'))
  if (last && last.op === 'skill_delete' && d.undo) toast(t('skDeleted').split(' (')[0], { label: t('memUndo'), run: () => skSend({ op: 'skill_undo', id: d.undo, account: last.account ?? null }) })
  if (last && last.op === 'skill_approve') { S.skEditing = 0; S.skDraft = null; S.skEditReport = null }
}

$('sk-auto').addEventListener('change', () => skSend({ op: 'skill_setting', auto: $('sk-auto').checked }))
$('sk-propose').addEventListener('submit', event => {
  event.preventDefault()
  if (!S.cur) return toast(t('noSession'))
  // A proposal is read from the chat's own account and kept in its skills: the panel moves to that account first.
  const chatAccount = accountOf(curSession())
  setAcct(chatAccount)
  skSend({ op: 'skill_propose', account: chatAccount, sid: S.cur, hint: $('sk-hint').value.trim(), target: $('sk-target').value, scope: $('sk-scope').value })
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
  if (p.from_gateway) head.appendChild(el('span', 'tag', t('skGatewayTag')))
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
  $('pv-headers').value = p && p.headers ? p.headers : ''
  // An account chosen in the desktop app is kept as it is when the page saves the provider again.
  S.pvAccount = p && p.login_account ? p.login_account : null
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
    models: { opus: $('pv-opus').value, sonnet: $('pv-sonnet').value, haiku: $('pv-haiku').value }, headers: $('pv-headers').value, account: S.pvAccount || undefined,
  })
})

// ---- accounts: made, renamed and removed here by the person; the login is made in the person's own terminal ----
function askAccounts() { wsSend({ op: 'account_list' }) }
function acctSend(op) {
  S.acctRid += 1
  S.acctOps.set(S.acctRid, op)
  wsSend({ ...op, rid: S.acctRid })
}
const acctKey = id => id || 'default'
/** Asks the host what `claude auth status` says for an account (null: the default one). */
function askAcctStatus(id) {
  S.acctReq += 1
  S.acctStatus.set(acctKey(id), { pending: true, req: S.acctReq })
  wsSend({ op: 'account_status', req: S.acctReq, id: id ?? null })
  renderAccounts()
}
function acctStatusIn(d) {
  const key = acctKey(d.id)
  const now = S.acctStatus.get(key)
  // An answer to an older question is dropped.
  if (now && now.req && d.req !== now.req) return
  S.acctStatus.set(key, d.status ? { status: d.status } : { error: d.error || '' })
  renderAccounts()
  renderAcctBar()
}
function acctStatusText(id) {
  const st = S.acctStatus.get(acctKey(id))
  if (!st) return ''
  if (st.pending) return t('acctChecking')
  if (st.error !== undefined || (st.status && st.status.known === false)) return t('acctUnknown')
  const who = st.status.email ? ` · ${st.status.email}` : ''
  return (st.status.logged_in ? t('acctLoggedIn') : t('acctLoggedOut')) + who
}
/** Asks the host which plugins this account could get (the default account's, minus its own). */
function askPlugins(id) {
  S.acctReq += 1
  wsSend({ op: 'account_plugins', id, req: S.acctReq })
}
function acctResult(d) {
  const last = S.acctOps.get(d.rid)
  S.acctOps.delete(d.rid)
  if (last && last.op === 'account_plugin_install') {
    S.acctPlugBusy.delete(last.id + '/' + last.plugin)
    toast(d.ok ? `${t('plugDone')}: ${last.plugin}` : `${t('plugFailed')}: ${d.error || ''}`)
    askPlugins(last.id)
    renderAccounts()
    return
  }
  if (!d.ok) { if (d.error) toast(d.error); return }
  if (last && last.op === 'account_add') {
    $('acct-form').hidden = true
    $('acct-new').hidden = false
    $('ac-name').value = ''
    if (d.account) askAcctStatus(d.account.id)
  }
  if (last && last.op === 'account_delete') for (const w of d.warnings || []) toast(t('acctW_' + w))
}
/** An account with notes, skills and chats: a Claude one (an entry without a kind is one). */
const isClaudeAcct = a => (a.kind || 'claude') === 'claude'
function acctRow(a) {
  const card = el('div', 'mem')
  const chats = S.sessions.filter(x => x.account && x.account.id === a.id).length
  card.append(el('div', 't', isClaudeAcct(a) ? a.name : `${a.name} · Codex`), el('div', 'd', `${a.open ? acctStatusText(a.id) || t('acctUnknown') : t('acctClosed')} · ${t('acctChats')} ${chats}`))
  // How to log in is folded away once the account is logged in; it is open while it is not.
  const how = el('details')
  const known = S.acctStatus.get(a.id)
  how.open = S.acctHow.has(a.id) || Boolean(known && known.status && !known.status.logged_in)
  how.addEventListener('toggle', () => { if (how.open) S.acctHow.add(a.id); else S.acctHow.delete(a.id) })
  how.append(el('summary', '', t('acctHow')), el('div', 'muted', t('acctLoginNote')), el('code', 'acct-login', a.login))
  card.append(how)
  // Plugins of the default account that this account lacks, installed by a click (the host runs Claude Code's own plugin commands as the account).
  const plug = el('details')
  plug.hidden = !isClaudeAcct(a)
  plug.open = S.acctPlug.has(a.id)
  plug.addEventListener('toggle', () => {
    if (plug.open) { S.acctPlug.add(a.id); askPlugins(a.id) } else S.acctPlug.delete(a.id)
  })
  plug.append(el('summary', '', t('plugTitle')))
  const info = S.acctPlugins.get(a.id)
  if (plug.open) {
    plug.append(el('div', 'muted', t('plugIntro')))
    if (!info) plug.append(el('div', 'muted', t('acctChecking')))
    else if (info.error) plug.append(el('div', 'muted', info.error))
    else if (!info.offer.length) plug.append(el('div', 'muted', t('plugNone')))
    for (const p of info?.offer || []) {
      const line = el('div', 'row')
      const busy = S.acctPlugBusy.has(a.id + '/' + p.id)
      const b = el('button', '', busy ? t('plugBusy') : t('plugInstall'))
      b.type = 'button'
      b.disabled = busy
      b.addEventListener('click', () => {
        if (!confirm(t('plugConfirm').replace('{p}', p.id))) return
        S.acctPlugBusy.add(a.id + '/' + p.id)
        acctSend({ op: 'account_plugin_install', id: a.id, plugin: p.id })
        renderAccounts()
      })
      line.append(el('span', '', `${p.id}${p.version ? ' ' + p.version : ''}`), b)
      plug.append(line)
    }
  }
  card.append(plug)
  const row = el('div', 'row')
  const button = (label, run, cls) => {
    const b = el('button', cls || '', label)
    b.type = 'button'
    b.addEventListener('click', run)
    row.append(b)
  }
  button(t('acctCheck'), () => askAcctStatus(a.id))
  button(t('acctLogin'), () => { navigator.clipboard?.writeText(a.login).then(() => toast(t('acctCopied')), () => {}) })
  button(t('acctRename'), () => { const name = prompt(t('acctRenamePrompt'), a.name); if (name && name.trim()) acctSend({ op: 'account_rename', id: a.id, name }) })
  button(t('acctDelete'), () => { if (confirm(isClaudeAcct(a) ? t('acctDeleteConfirm').replace('{n}', String(chats)) : t('acctDeleteConfirmCodex'))) acctSend({ op: 'account_delete', id: a.id, confirm: true }) }, 'danger')
  card.append(row)
  return card
}
function renderAccounts() {
  const d = S.accounts
  if (!d) return
  const list = $('acct-list')
  list.replaceChildren()
  $('acct-notice').hidden = !d.notice
  $('acct-notice').textContent = d.notice || ''
  const base = el('div', 'mem')
  base.append(el('div', 't', t('acctDefaultRow')), el('div', 'd', acctStatusText(null) || ''))
  const baseRow = el('div', 'row')
  const check = el('button', '', t('acctCheck'))
  check.type = 'button'
  check.addEventListener('click', () => askAcctStatus(null))
  baseRow.append(check)
  base.append(baseRow)
  list.appendChild(base)
  if (!d.accounts.length) list.appendChild(el('div', 'muted', t('acctNone')))
  for (const a of d.accounts) list.appendChild(acctRow(a))
  // The chooser of a new chat: only accounts that are open can be chosen.
  const row = $('new-acct-row')
  // A chat runs in a Claude account only: a Codex account is not offered.
  const claudeAccounts = d.accounts.filter(isClaudeAcct)
  row.hidden = claudeAccounts.length === 0
  const select = $('new-account')
  const keep = select.value
  select.replaceChildren(Object.assign(el('option', '', t('acctDefault')), { value: '' }))
  for (const a of claudeAccounts) if (a.open) select.append(Object.assign(el('option', '', a.name), { value: a.id }))
  select.value = claudeAccounts.some(a => a.open && a.id === keep) ? keep : ''
  // The account a panel shows is gone: the panels go back to the default account.
  if (S.acct && !d.accounts.some(a => a.id === S.acct)) setAcct(null)
  updateNewChat()
  renderAcctBar()
  renderSessions()
  showAcctPill(curSession())
}
/** The bar over the search and the two panels: shown only when there is an account besides the default one. */
function renderAcctBar() {
  const accounts = (S.accounts?.accounts || []).filter(isClaudeAcct)
  $('acct-bar').hidden = accounts.length === 0 || !['chats', 'memory', 'skills'].includes(S.side)
  const select = $('acct-view')
  // What waits or runs under an account that is not shown is told on its entry: a chat there is not in the list.
  const seen = attn(S.sessions, S.acct, knownAccounts())
  const entries = [['', t('acctDefault')], ...accounts.map(a => [a.id, a.name])].map(([value, name]) => {
    const e = seen.get(value)
    return [value, e ? name + (e.wait ? ` · ⚠ ${e.wait} ${t('acctAttn')}` : e.run ? ' · ●' : '') : name]
  })
  const same = select.options.length === entries.length && entries.every(([value], i) => select.options[i].value === value)
  if (same) {
    // The same accounts: only the words change, in place, so an open drop-down is not closed by an event of another chat.
    entries.forEach(([, text], i) => { if (select.options[i].textContent !== text) select.options[i].textContent = text })
  } else {
    select.replaceChildren(...entries.map(([value, text]) => Object.assign(el('option', '', text), { value })))
  }
  select.value = S.acct || ''
  const st = S.acctStatus.get(acctKey(S.acct))
  $('acct-who').textContent = st && st.status ? `${st.status.email || ''}${st.status.logged_in ? '' : ' · ' + t('acctLoggedOut')}` : ''
}
$('acct-view').addEventListener('change', () => setAcct($('acct-view').value || null))
$('acct-new').addEventListener('click', () => { $('acct-form').hidden = false; $('acct-new').hidden = true; $('ac-name').focus() })
$('ac-cancel').addEventListener('click', () => { $('acct-form').hidden = true; $('acct-new').hidden = false })
$('acct-form').addEventListener('submit', event => {
  event.preventDefault()
  const name = $('ac-name').value.trim()
  if (name) acctSend({ op: 'account_add', name, kind: $('ac-kind').value })
})
$('acct-box').addEventListener('toggle', () => { if ($('acct-box').open) { askAccounts(); askAcctStatus(null); for (const a of S.accounts?.accounts || []) if (a.open) askAcctStatus(a.id) } })

showConn()
connect()
