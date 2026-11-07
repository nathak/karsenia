import { marked } from './vendor/marked.js'
import DOMPurify from './vendor/purify.js'
import { baseName, checkImage, fmtBytes, fmtTurn, imagePaths, lineDiff, markParts, nearestSeq, numberedLines, pathsInText, resultBlocks, skillMd, toolSummary } from './util.js'

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
    skVersion: '버전', skGiven: 'Claude에게 스킬을 전달했습니다',
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
    skVersion: 'version', skGiven: 'Claude was given skills',
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
  if (token) sessionStorage.setItem('swb-token', token)
  else token = sessionStorage.getItem('swb-token')
} catch { /* no storage: the token stays in memory */ }
history.replaceState(null, '', location.pathname)

// ---- theme ----
const applyTheme = mode => { if (mode === 'light' || mode === 'dark') document.documentElement.dataset.theme = mode; else delete document.documentElement.dataset.theme }
applyTheme(store.get('swb-theme'))
$('theme').addEventListener('click', () => {
  const now = document.documentElement.dataset.theme
  const next = now === 'dark' ? 'light' : now === 'light' ? '' : 'dark'
  store.set('swb-theme', next)
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
const S = { ws: null, sessions: [], cur: null, lastSeq: 0, replaying: false, state: 'idle', pending: new Map(), tools: new Map(), live: null, curSeq: 0, jumpTo: null, searchReq: 0, side: 'chats', mem: null, memRid: 0, memOps: new Map(), editing: 0, skills: null, skEditing: 0, skHistory: null, images: [], debug: [], connected: false, wasConnected: false }

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
  if (m.kind === 'skills') { S.skills = m.data; renderSkills(); return }
  if (m.kind === 'skills_changed') { askSkills(); return }
  if (m.kind === 'skill_result') { skResult(m.data); return }
  if (m.kind === 'skill_versions') { S.skHistory = m.data; renderSkills(); return }
  if (m.kind === 'skill_notice') { toast(m.data.text); return }
  if (m.kind === 'skill_proposed') { toast(`${t('skProposed')}: ${m.data.name}`, { label: t('tabSkills'), run: () => showSide('skills') }); askSkills(); return }
  if (m.kind === 'hello') {
    S.sessions = m.data.sessions || []
    renderSessions()
    askMemory()
    askSkills()
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
    b.append(title, el('div', 'd', baseName(s.cwd) + (s.model ? ' · ' + s.model : '')))
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
  setState(s ? s.state : 'idle')
  renderSessions()
  if (dialog.open) dialog.close('switched')
  if (attach) wsSend({ op: 'attach', sid, after_seq: 0 })
  $('input').focus()
}

$('new-toggle').addEventListener('click', () => {
  $('new-form').hidden = !$('new-form').hidden
  $('new-cwd').value = store.get('swb-cwd') || ''
  $('new-cwd').focus()
})
$('new-cancel').addEventListener('click', () => { $('new-form').hidden = true })
$('new-form').addEventListener('submit', event => {
  event.preventDefault()
  const cwd = $('new-cwd').value.trim()
  if (!cwd) return
  store.set('swb-cwd', cwd)
  wsSend({ op: 'new', cwd, model: $('new-model').value || undefined })
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
  if (first.tool_name === 'mcp__swb_app__search_history') {
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
  $('tab-chats').classList.toggle('on', name === 'chats')
  $('tab-memory').classList.toggle('on', name === 'memory')
  $('tab-skills').classList.toggle('on', name === 'skills')
  if (name === 'memory') askMemory()
  if (name === 'skills') askSkills()
}
$('tab-chats').addEventListener('click', () => showSide('chats'))
$('tab-memory').addEventListener('click', () => showSide('memory'))
$('tab-skills').addEventListener('click', () => showSide('skills'))

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
  if (!d.ok) { if (d.error) toast(d.error); return }
  if (last && last.op === 'skill_propose') toast(t('skStarted'))
  if (last && last.op === 'skill_delete' && d.undo) toast(t('skDeleted').split(' (')[0], { label: t('memUndo'), run: () => skSend({ op: 'skill_undo', id: d.undo }) })
  S.skEditing = 0
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
    const form = el('div', 'sk')
    const name = el('input')
    name.type = 'text'
    name.value = p.name
    name.disabled = Boolean(p.target)
    const description = el('textarea')
    description.value = p.description
    const body = el('textarea')
    body.value = p.body
    body.style.minHeight = '160px'
    const save = el('button', 'primary', t('skSaveApprove'))
    save.type = 'button'
    save.addEventListener('click', () => skSend({ op: 'skill_approve', id: p.id, ack_risk: ack.checked, edit: { name: name.value, description: description.value.replace(/\s+/g, ' ').trim(), body: body.value } }))
    form.append(el('div', 'muted', t('skName')), name, el('div', 'muted', t('skDescription')), description, el('div', 'muted', t('skBody')), body, save)
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

showConn()
connect()
