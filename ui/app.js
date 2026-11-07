import { marked } from './vendor/marked.js'
import DOMPurify from './vendor/purify.js'
import { baseName, checkImage, fmtTurn, imagePaths, markParts, nearestSeq, pathsInText, resultBlocks, toolSummary } from './util.js'

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
const S = { ws: null, sessions: [], cur: null, lastSeq: 0, replaying: false, state: 'idle', pending: new Map(), tools: new Map(), live: null, curSeq: 0, jumpTo: null, searchReq: 0, images: [], debug: [], connected: false, wasConnected: false }

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
  if (m.kind === 'hello') {
    S.sessions = m.data.sessions || []
    renderSessions()
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

showConn()
connect()
