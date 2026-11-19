'use strict'
// Tenrec live view: a read-only page. Every text from the feed goes into the page with textContent (never as markup), and the token
// is only ever sent in a header.

const T = {
  ko: {
    title: '타임라인', pause: '일시정지', resume: '재개', paused: '일시정지', waiting: '대기', live: '● 실시간', reconnecting: '◌ 다시 연결 중', down: '✕ 연결 끊김',
    last: '마지막 이벤트', ago: '전', never: '아직 없음', short: '프롬프트 축약', tiles: { rate: '분당 호출', p50: '지연 p50', p95: '지연 p95', bad: '차단·오류', asks: '대기 중인 질문', ctx: '컨텍스트', tok: '토큰 (입력·출력)', cost: '비용 (마지막 결과 기준)' },
    status: { running: '◐ 실행 중', ok: '✓ 성공', error: '▲ 오류', blocked: '✕ 차단', declined: '⊘ 거절', started: '→ 시작됨', asking: '? 질문 중' },
    turn: '턴', calls: '호출', noChats: '열려 있는 채팅이 없습니다. 앱에서 채팅을 만들면 여기에 나타납니다.', noItems: '아직 이 채팅에 기록이 없습니다.', newer: '새 항목 ↓',
    auth: '이 주소의 토큰이 맞지 않습니다. 앱에서 라이브 보기를 다시 여세요.', noToken: '앱에서 라이브 보기를 열어 주세요 (토큰이 필요합니다).', gone: '채팅이 닫혔습니다', stopped: '서버가 종료되었습니다. 앱에서 다시 시작하세요.',
    detail: '상세', tool: '도구', category: '분류', risk: '위험도', guard: '가드', result: '결과', time: '시간', input: '입력', output: '출력', hidden: '(표시하지 않음)', none: '—',
    derivedNote: '지금 설정으로 다시 계산한 값', theme: '테마', dark: '어둡게', light: '밝게', parent: '상위 호출', background: '백그라운드',
  },
  en: {
    title: 'Timeline', pause: 'Pause', resume: 'Resume', paused: 'paused', waiting: 'waiting', live: '● live', reconnecting: '◌ reconnecting', down: '✕ disconnected',
    last: 'last event', ago: 'ago', never: 'none yet', short: 'prompts cut short', tiles: { rate: 'Calls / min', p50: 'Latency p50', p95: 'Latency p95', bad: 'Blocked + errors', asks: 'Open asks', ctx: 'Context', tok: 'Tokens (in · out)', cost: 'Cost (last result)' },
    status: { running: '◐ running', ok: '✓ ok', error: '▲ error', blocked: '✕ blocked', declined: '⊘ declined', started: '→ started', asking: '? asking' },
    turn: 'Turn', calls: 'calls', noChats: 'No chat is open. A chat made in the app shows up here.', noItems: 'Nothing recorded for this chat yet.', newer: 'New items ↓',
    auth: 'The token of this address is wrong. Open the live view again from the app.', noToken: 'Open the live view from the app (a token is needed).', gone: 'the chat was closed', stopped: 'The server has stopped. Start it again in the app.',
    detail: 'Details', tool: 'Tool', category: 'Category', risk: 'Risk', guard: 'Guard', result: 'Result', time: 'Time', input: 'Input', output: 'Output', hidden: '(not shown)', none: '—',
    derivedNote: 'recomputed with the settings as they are now', theme: 'Theme', dark: 'Dark', light: 'Light', parent: 'Parent call', background: 'background',
  },
}

const $ = id => document.getElementById(id)
const h = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text !== undefined && text !== null) e.textContent = String(text); return e }
const store = { get(k) { try { return localStorage.getItem(k) } catch { return null } }, set(k, v) { try { localStorage.setItem(k, v) } catch { /* private window */ } } }
const session = { get(k) { try { return sessionStorage.getItem(k) } catch { return null } }, set(k, v) { try { sessionStorage.setItem(k, v) } catch { /* private window */ } } }

const S = {
  token: '', sid: '', lang: 'ko', chats: [], turns: new Map(), calls: new Map(), asks: new Map(), paused: false, pending: [], lastTs: 0, ctx: null,
  tokens: { input: 0, output: 0 }, cost: 0, sel: null, conn: 'wait', mode: 'full', dirty: new Set(), newCount: 0, ctrl: null, attempt: 0, ready: false, fatal: '',
}
const tr = () => T[S.lang]

// ---- start: the token comes in the address fragment and is moved out of it at once
function start() {
  const hash = new URLSearchParams(location.hash.slice(1))
  const token = hash.get('t') || session.get('live.token') || ''
  if (hash.get('t')) { session.set('live.token', token); history.replaceState(null, '', location.pathname + (hash.get('s') ? '#s=' + encodeURIComponent(hash.get('s')) : '')) }
  S.token = token
  const saved = store.get('live.lang')
  S.lang = ['ko', 'en'].includes(hash.get('lang')) ? hash.get('lang') : ['ko', 'en'].includes(saved) ? saved : ((navigator.language || '').startsWith('ko') ? 'ko' : 'en')
  S.sid = hash.get('s') || session.get('live.sid') || ''
  const theme = store.get('live.theme')
  if (theme) document.documentElement.dataset.theme = theme
  else if (window.matchMedia && matchMedia('(prefers-color-scheme: light)').matches) document.documentElement.dataset.theme = 'light'
  $('pause').addEventListener('click', togglePause)
  $('lang').addEventListener('click', () => { S.lang = S.lang === 'ko' ? 'en' : 'ko'; store.set('live.lang', S.lang); labels(); renderAll() })
  $('theme').addEventListener('click', () => { const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light'; document.documentElement.dataset.theme = next; store.set('live.theme', next); labels() })
  $('newer').addEventListener('click', () => { window.scrollTo(0, document.body.scrollHeight); S.newCount = 0; renderNewer() })
  window.addEventListener('scroll', () => { if (nearBottom()) { S.newCount = 0; renderNewer() } }, { passive: true })
  document.addEventListener('keydown', e => { if (e.key === 'p' && !e.metaKey && !e.ctrlKey && !e.altKey && !/input|textarea/i.test(document.activeElement.tagName)) togglePause() })
  labels()
  if (!S.token) return fatal(tr().noToken)
  setInterval(renderStatus, 1000)
  boot()
}

async function api(path) {
  const r = await fetch(path, { headers: { Authorization: 'Bearer ' + S.token }, cache: 'no-store' })
  if (r.status === 401) { fatal(tr().auth); throw new Error('401') }
  if (!r.ok) throw new Error(String(r.status))
  return r.json()
}

async function boot() {
  try {
    const info = await api('/api/info')
    S.mode = info.prompts
    const chats = await api('/api/chats')
    setChats(chats)
    if (!S.sid || !chats.some(c => c.sid === S.sid)) S.sid = chats.length ? chats[0].sid : ''
  } catch (e) { if (!S.fatal) schedule(); return }
  renderAll()
  if (S.sid) connect()
}

function fatal(text) { S.fatal = text; const n = $('notice'); n.textContent = text; n.hidden = false; setConn('off', true); if (S.ctrl) S.ctrl.abort() }
function setConn(state) { S.conn = state; renderStatus() }

// ---- the feed
function schedule() { S.attempt += 1; setConn('wait'); setTimeout(() => { if (S.fatal) return; S.sid ? connect() : boot() }, Math.min(10000, 500 * 2 ** Math.min(S.attempt, 5))) }

function resetFeed() {
  S.turns = new Map(); S.calls = new Map(); S.asks = new Map(); S.pending = []; S.lastTs = 0; S.ctx = null; S.tokens = { input: 0, output: 0 }; S.cost = 0
  S.sel = null; S.dirty = new Set(); S.newCount = 0; S.ready = false
  $('timeline').textContent = ''
}

async function connect() {
  if (S.ctrl) S.ctrl.abort()
  const ctrl = new AbortController(); S.ctrl = ctrl
  setConn('wait'); resetFeed(); renderAll()
  let resp
  try { resp = await fetch('/api/stream?sid=' + encodeURIComponent(S.sid), { headers: { Authorization: 'Bearer ' + S.token }, signal: ctrl.signal, cache: 'no-store' }) } catch { if (!ctrl.signal.aborted) schedule(); return }
  if (resp.status === 401) return fatal(tr().auth)
  if (resp.status === 404) { setConn('off'); await refreshChats(); return }
  if (!resp.ok || !resp.body) return schedule()
  S.attempt = 0; setConn('on')
  const reader = resp.body.getReader(); const dec = new TextDecoder(); let buf = ''
  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buf += dec.decode(value, { stream: true })
      let at
      while ((at = buf.indexOf('\n\n')) >= 0) { frame(buf.slice(0, at)); buf = buf.slice(at + 2) }
    }
  } catch { /* aborted or dropped */ }
  if (!ctrl.signal.aborted && !S.fatal) { setConn('off'); schedule() }
}

function frame(text) {
  let name = 'message'; let data = ''
  for (const line of text.split('\n')) { if (line.startsWith('event: ')) name = line.slice(7); else if (line.startsWith('data: ')) data += line.slice(6) }
  if (!data) return
  let body; try { body = JSON.parse(data) } catch { return }
  if (name === 'item') ingest(body)
  else if (name === 'tabs') { setChats(body); renderTabs() }
  else if (name === 'ready') { S.ready = true; renderAll(); window.scrollTo(0, document.body.scrollHeight) }
  else if (name === 'reset') schedule()
  else if (name === 'closed') { const n = $('notice'); n.textContent = tr().gone; n.hidden = false }
}

function ingest(item) {
  if (S.paused) { S.pending.push(item); renderStatus(); return }
  apply(item)
  if (S.ready) { schedulePaint(); if (!nearBottom()) { S.newCount += 1; renderNewer() } }
}

const turnOf = n => { let t = S.turns.get(n); if (!t) { t = { n, prompt: null, ts: 0, calls: [], notes: [], end: null, open: undefined }; S.turns.set(n, t) } return t }

function apply(item) {
  const t = turnOf(item.turn); S.dirty.add(item.turn); S.lastTs = Math.max(S.lastTs, item.ts || 0)
  switch (item.t) {
    case 'prompt': t.prompt = item; t.ts = item.ts; break
    case 'call': { const c = { ...item, status: 'running', ms: null, output: null, asking: false, turn: item.turn }; S.calls.set(c.id, c); t.calls.push(c); break }
    case 'call_end': { const c = S.calls.get(item.id); if (c) { c.status = item.status; c.ms = item.ms; c.output = item.output; c.asking = false; c.endTs = item.ts } break }
    case 'ask': { S.asks.set(item.request_id, item); const c = item.call && S.calls.get(item.call); if (c) c.asking = true; break }
    case 'ask_end': { const a = S.asks.get(item.request_id); S.asks.delete(item.request_id); const c = a && a.call && S.calls.get(a.call); if (c) { c.asking = false; if (item.outcome === 'denied') c.denied = true } break }
    case 'turn_end': { t.end = item; S.tokens.input += item.usage.input + item.usage.cache_read + item.usage.cache_create; S.tokens.output += item.usage.output; if (item.cost_usd) S.cost = item.cost_usd; if (item.context) S.ctx = item.context; break }
    case 'notice': t.notes.push(item); break
    default: break
  }
}

function togglePause() {
  S.paused = !S.paused
  if (!S.paused) { const list = S.pending; S.pending = []; for (const i of list) apply(i); schedulePaint() }
  labels(); renderStatus()
}

// ---- rendering
let painting = false
function schedulePaint() { if (painting) return; painting = true; requestAnimationFrame(() => { painting = false; renderTiles(); renderTimeline(); renderDetail() }) }
const nearBottom = () => document.documentElement.scrollHeight - window.innerHeight - window.scrollY < 90

function labels() {
  const t = tr()
  $('pause').textContent = S.paused ? t.resume : t.pause
  $('lang').textContent = S.lang === 'ko' ? 'EN' : '한국어'
  $('theme').textContent = document.documentElement.dataset.theme === 'light' ? t.dark : t.light
  $('h-timeline').textContent = t.title
  $('newer').textContent = t.newer
  document.documentElement.lang = S.lang
  const m = $('mode'); m.hidden = S.mode !== 'short'; m.textContent = t.short
}

function fmtMs(ms) { return ms === null || ms === undefined ? '' : ms < 1000 ? ms + 'ms' : (ms / 1000).toFixed(ms < 10000 ? 1 : 0) + 's' }
function fmtN(n) { return n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? (n / 1e3).toFixed(1) + 'k' : String(n) }

function renderStatus() {
  const t = tr()
  const c = $('conn'); c.className = 'conn ' + (S.conn === 'on' ? 'on' : S.conn === 'wait' ? 'wait' : 'off'); c.textContent = S.conn === 'on' ? t.live : S.conn === 'wait' ? t.reconnecting : t.down
  $('last').textContent = S.lastTs ? t.last + ' ' + ago(Date.now() - S.lastTs) : t.last + ': ' + t.never
  const p = $('paused'); p.hidden = !S.paused
  if (S.paused) { const hot = S.pending.filter(i => i.t === 'ask' || (i.t === 'call_end' && i.status === 'blocked')).length; p.textContent = '‖ ' + t.paused + ' · ' + S.pending.length + ' ' + t.waiting + (hot ? ' · ⚠ ' + hot : '') }
}
function ago(ms) { const s = Math.max(0, Math.round(ms / 1000)); return s < 60 ? s + (S.lang === 'ko' ? '초 ' : 's ') + tr().ago : Math.round(s / 60) + (S.lang === 'ko' ? '분 ' : 'm ') + tr().ago }

function setChats(list) { S.chats = Array.isArray(list) ? list : [] }
async function refreshChats() { try { setChats(await api('/api/chats')) } catch { /* retried by the stream */ } renderTabs() }

function renderTabs() {
  const box = $('tabs'); box.textContent = ''; box.setAttribute('role', 'tablist')
  for (const c of S.chats) {
    const b = h('div', 'tab' + (c.state === 'exited' ? ' gone' : '')); b.tabIndex = 0; b.setAttribute('role', 'tab'); b.setAttribute('aria-selected', String(c.sid === S.sid))
    b.append(h('span', '', c.title || c.sid.slice(0, 8)), h('span', 'proj', c.project))
    if (c.asks_open > 0) b.append(h('span', 'badge ask', '? ' + c.asks_open))
    const pick = () => { if (c.sid === S.sid) return; S.sid = c.sid; session.set('live.sid', c.sid); connect() }
    b.addEventListener('click', pick); b.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); pick() } })
    box.append(b)
  }
}

function pctl(sorted, q) { return sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] : null }

function renderTiles() {
  const t = tr(); const box = $('tiles'); box.textContent = ''
  const calls = [...S.calls.values()]
  const now = Date.now()
  const rate = calls.filter(c => c.ts && now - c.ts < 60000).length
  const timed = calls.filter(c => (c.status === 'ok' || c.status === 'error') && c.ms !== null && !c.background && !['Agent', 'Task', 'Workflow'].includes(c.tool)).map(c => c.ms).sort((a, b) => a - b)
  const bad = calls.filter(c => c.status === 'blocked' || c.status === 'error').length
  const tile = (k, v, s, gauge) => { const e = h('div', 'tile'); e.append(h('div', 'k', k), h('div', 'v', v)); if (s) e.append(h('div', 's', s)); if (gauge !== undefined) { const g = h('div', 'gauge'); const i = h('i'); i.style.width = Math.min(100, gauge) + '%'; if (gauge > 90) i.className = 'full'; else if (gauge > 70) i.className = 'hot'; g.append(i); e.append(g) } box.append(e) }
  tile(t.tiles.rate, rate)
  tile(t.tiles.p50, timed.length ? fmtMs(pctl(timed, 0.5)) : '—')
  tile(t.tiles.p95, timed.length ? fmtMs(pctl(timed, 0.95)) : '—')
  tile(t.tiles.bad, bad)
  tile(t.tiles.asks, S.asks.size)
  tile(t.tiles.ctx, S.ctx ? Math.round(100 * S.ctx.used / S.ctx.window) + '%' : '—', S.ctx ? fmtN(S.ctx.used) + ' / ' + fmtN(S.ctx.window) : '', S.ctx ? 100 * S.ctx.used / S.ctx.window : undefined)
  tile(t.tiles.tok, fmtN(S.tokens.input) + ' · ' + fmtN(S.tokens.output))
  tile(t.tiles.cost, S.cost ? '$' + S.cost.toFixed(S.cost < 1 ? 3 : 2) : '—')
}

function statusOf(c) { return c.status === 'running' && c.asking ? 'asking' : c.status }

function callRow(c, maxMs) {
  const t = tr(); const st = statusOf(c)
  const row = h('div', 'call' + (S.sel === c.id ? ' sel' : '')); row.tabIndex = 0; row.setAttribute('role', 'button')
  row.append(h('span', 'st ' + st, t.status[st] || st), h('span', 'tool', c.tool), h('span', 'sum', c.summary), h('span', 'dur', fmtMs(c.ms)))
  const tags = h('span', 'tags')
  if (c.risk !== 'low') tags.append(h('span', 'tag ' + c.risk, c.risk))
  if (c.guard && c.guard.policy !== 'allow') tags.append(h('span', 'tag ' + c.guard.policy, c.guard.policy + (c.guard.derived ? '*' : '')))
  if (c.background) tags.append(h('span', 'tag', t.background))
  row.append(tags)
  if (c.ms && maxMs) { const w = h('div', 'bar-w'); const i = h('i'); i.style.width = Math.max(1, Math.round(100 * c.ms / maxMs)) + '%'; w.append(i); row.append(w) }
  const pick = () => { S.sel = c.id; for (const n of document.querySelectorAll('.call.sel')) n.classList.remove('sel'); row.classList.add('sel'); renderDetail() }
  row.addEventListener('click', pick); row.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); pick() } })
  return row
}

function renderTurn(t, last) {
  const tt = tr()
  let d = t.el
  if (!d) { d = h('details', 'turn'); d.addEventListener('toggle', () => { t.open = d.open }); t.el = d }
  d.open = t.open === undefined ? last : t.open
  d.classList.toggle('bad', t.calls.some(c => c.status === 'blocked' || c.status === 'error'))
  d.textContent = ''
  const sum = h('summary'); sum.append(h('span', 'no', t.n ? tt.turn + ' ' + t.n : '—'), h('span', 'prompt', t.prompt ? t.prompt.text.replace(/\s+/g, ' ') : ''))
  const meta = [t.calls.length ? t.calls.length + ' ' + tt.calls : '', t.end ? fmtMs(t.end.ms) : '', t.end && t.end.cost_usd ? '$' + t.end.cost_usd.toFixed(3) : ''].filter(Boolean).join(' · ')
  sum.append(h('span', 'meta', meta)); d.append(sum)
  const maxMs = Math.max(0, ...t.calls.map(c => c.ms || 0))
  const wrap = h('div', 'calls')
  const children = new Map()
  for (const c of t.calls) if (c.parent) { if (!children.has(c.parent)) children.set(c.parent, []); children.get(c.parent).push(c) }
  const place = (c, into) => { into.append(callRow(c, maxMs)); const kids = children.get(c.id); if (kids) { const box = h('div', 'child'); for (const k of kids) place(k, box); into.append(box) } }
  for (const c of t.calls) if (!c.parent || !S.calls.has(c.parent)) place(c, wrap)
  for (const n of t.notes) wrap.append(h('div', 'note err', n.text))
  d.append(wrap)
  return d
}

function renderTimeline() {
  const box = $('timeline')
  const keep = nearBottom()
  const order = [...S.turns.keys()].sort((a, b) => a - b)
  if (!order.length) { box.textContent = ''; if (S.ready) box.append(h('div', 'empty', tr().noItems)); return }
  if (box.firstChild && !box.firstChild.classList?.contains('turn')) box.textContent = ''
  order.forEach((n, i) => {
    const t = S.turns.get(n); const last = i === order.length - 1
    if (!t.el || S.dirty.has(n)) renderTurn(t, last)
    if (t.el.parentNode !== box || box.children[i] !== t.el) box.insertBefore(t.el, box.children[i] || null)
  })
  S.dirty.clear()
  if (keep) window.scrollTo(0, document.body.scrollHeight)
}

function renderDetail() {
  const box = $('detail'); const c = S.sel && S.calls.get(S.sel); const t = tr()
  if (!c) { box.hidden = true; return }
  box.hidden = false; box.textContent = ''
  box.append(h('h3', '', t.detail + ' · ' + c.tool))
  const dl = h('dl'); const row = (k, v) => { dl.append(h('dt', '', k), h('dd', '', v)) }
  const st = statusOf(c)
  row(t.result, (t.status[st] || st) + (c.ms !== null ? ' · ' + fmtMs(c.ms) : '')); row(t.category, c.category); row(t.risk, c.risk)
  row(t.guard, c.guard ? c.guard.policy + ' · ' + c.guard.profile + (c.guard.rule ? ' · rule ' + c.guard.rule : '') + (c.guard.derived ? ' — ' + t.derivedNote : '') : t.none)
  if (c.parent) row(t.parent, c.parent)
  box.append(dl)
  const pre = (label, text) => { box.append(h('div', 'muted', label)); box.append(h('pre', '', text === null || text === undefined ? t.hidden : text)) }
  pre(t.input, c.input); pre(t.output, c.status === 'running' ? '…' : c.output)
}

function renderNewer() { const b = $('newer'); b.hidden = S.newCount === 0; if (S.newCount) b.textContent = tr().newer.replace('↓', '') + S.newCount + ' ↓' }

function renderAll() {
  labels(); renderStatus(); renderTabs(); renderTiles(); renderTimeline(); renderDetail(); renderNewer()
  if (!S.chats.length && !S.fatal) { const n = $('notice'); n.textContent = tr().noChats; n.hidden = false } else if (!S.fatal) $('notice').hidden = true
}

start()
