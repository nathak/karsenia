// Pure helpers for drawing events; no DOM here, so Node can test them.
export const IMAGE_EXT = /\.(png|jpe?g|gif|webp)$/i

/** One line that says what a tool call is about. */
export function toolSummary(name, input) {
  const o = input && typeof input === 'object' ? input : {}
  const pick = o.command ?? o.file_path ?? o.path ?? o.pattern ?? o.url ?? o.query ?? o.description ?? o.prompt
  if (typeof pick === 'string') return pick.split('\n')[0].slice(0, 160)
  const first = Object.values(o).find(v => typeof v === 'string')
  return typeof first === 'string' ? first.split('\n')[0].slice(0, 160) : ''
}

/** String values of a tool input that name an image file. */
export function imagePaths(input) {
  if (!input || typeof input !== 'object') return []
  const out = []
  for (const v of Object.values(input)) if (typeof v === 'string' && v.length < 400 && IMAGE_EXT.test(v.trim()) && !/\s{2,}|\n/.test(v)) out.push(v.trim())
  return [...new Set(out)].slice(0, 3)
}

/** Absolute image paths a result text mentions (a tool that said where it saved a picture), at most three. */
export function pathsInText(text) {
  if (typeof text !== 'string') return []
  const found = text.match(/\/[^\s"'`<>()[\]{}|]+\.(?:png|jpe?g|gif|webp)\b/gi) || []
  return [...new Set(found)].slice(0, 3)
}

/** A tool result's content as a list of text and image blocks. */
export function resultBlocks(content) {
  if (typeof content === 'string') return content ? [{ type: 'text', text: content }] : []
  if (!Array.isArray(content)) return []
  const out = []
  for (const b of content) {
    if (!b || typeof b !== 'object') continue
    if (b.type === 'text' && typeof b.text === 'string') out.push({ type: 'text', text: b.text })
    else if (b.type === 'image' && b.source && b.source.type === 'base64' && /^image\/(png|jpeg|gif|webp)$/.test(b.source.media_type)) out.push({ type: 'image', media_type: b.source.media_type, data: b.source.data })
  }
  return out
}

export function baseName(p) {
  const parts = String(p || '').split('/').filter(Boolean)
  return parts[parts.length - 1] || String(p || '')
}

export function fmtTurn(result) {
  const bits = []
  if (typeof result.duration_ms === 'number') bits.push((result.duration_ms / 1000).toFixed(1) + 's')
  if (typeof result.total_cost_usd === 'number') bits.push('$' + result.total_cost_usd.toFixed(4))
  return bits.join(' · ')
}

// The text of a search excerpt as plain pieces, with the matched ones marked (the page draws them as text nodes, never as HTML).
export const MARK_OPEN = '\u0002'
export const MARK_CLOSE = '\u0003'
export function markParts(snippet) {
  const parts = []
  let marked = false
  let buf = ''
  for (const ch of String(snippet)) {
    if (ch === MARK_OPEN || ch === MARK_CLOSE) {
      if (buf) parts.push({ text: buf, mark: marked })
      buf = ''
      marked = ch === MARK_OPEN
    } else buf += ch
  }
  if (buf) parts.push({ text: buf, mark: marked })
  return parts
}

/** The number closest to the target, preferring one at or before it (a result of a call sits after the call). */
export function nearestSeq(seqs, target) {
  let before = -1
  let after = -1
  for (const s of seqs) {
    if (s <= target && s > before) before = s
    if (s > target && (after === -1 || s < after)) after = s
  }
  return before !== -1 ? before : after
}

const TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp']
export const MAX_IMAGE = 5 * 1024 * 1024
export const MAX_IMAGES = 5

/** Whether a picked or pasted file may be attached, and why not. */
export function checkImage(file, already) {
  if (!TYPES.includes(file.type)) return 'type'
  if (file.size > MAX_IMAGE) return 'size'
  if (already >= MAX_IMAGES) return 'count'
  return ''
}

/** 1536 -> "1.5 KB" */
export function fmtBytes(n) {
  if (!Number.isFinite(n) || n < 0) return ''
  if (n < 1024) return n + ' B'
  return (n / 1024).toFixed(1) + ' KB'
}

/** The SKILL.md the host writes (the same text, so what a person reads is what is written). */
export function skillMd(name, description, body) {
  return `---\nname: ${name}\ndescription: ${JSON.stringify(description)}\n---\n\n${String(body).replace(/\s+$/, '')}\n`
}

/** Lines of a text with their numbers and the worst risk level on each: [{ n, text, level }]. */
export function numberedLines(text, risks, offset = 0) {
  const level = new Map()
  for (const r of risks || []) {
    if (!r.line) continue
    const at = r.line + offset
    if (r.level === 'block' || !level.has(at)) level.set(at, r.level)
  }
  return String(text).split('\n').map((t, i) => ({ n: i + 1, text: t, level: level.get(i + 1) || '' }))
}

/** A line diff of two texts: [{ t: ' ' | '+' | '-', text }]. Texts are small (a skill is at most 200 lines). */
export function lineDiff(before, after) {
  const a = String(before).split('\n')
  const b = String(after).split('\n')
  const table = Array.from({ length: a.length + 1 }, () => new Array(b.length + 1).fill(0))
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) table[i][j] = a[i] === b[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1])
  const out = []
  let i = 0
  let j = 0
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) { out.push({ t: ' ', text: a[i] }); i++; j++ }
    else if (table[i + 1][j] >= table[i][j + 1]) out.push({ t: '-', text: a[i++] })
    else out.push({ t: '+', text: b[j++] })
  }
  while (i < a.length) out.push({ t: '-', text: a[i++] })
  while (j < b.length) out.push({ t: '+', text: b[j++] })
  return out
}

/** Minutes after midnight as "09:05". */
export function fmtMinute(m) {
  if (!Number.isInteger(m) || m < 0 || m >= 1440) return ''
  return `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`
}

/** "09:05" (a time input's value) as minutes after midnight, or -1. */
export function parseHHMM(text) {
  const m = /^(\d{1,2}):(\d{2})$/.exec(String(text || '').trim())
  if (!m) return -1
  const h = Number(m[1])
  const min = Number(m[2])
  return h < 24 && min < 60 ? h * 60 + min : -1
}

/** A weekday mask (bit 0 Monday .. bit 6 Sunday) as seven booleans, and back. */
export const maskToDays = mask => [0, 1, 2, 3, 4, 5, 6].map(i => (mask & (1 << i)) !== 0)
export const daysToMask = days => days.reduce((mask, on, i) => (on ? mask | (1 << i) : mask), 0)

/** The host a chat's whole content is sent to, or '' for a chat on the Claude account. */
export function chatHost(session) {
  const host = session && session.provider && session.provider.host
  return typeof host === 'string' ? host : ''
}

/** True when `host[:port]` is this computer itself (an address a container cannot reach). */
export function isLocalHost(host) {
  return /^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/.test(typeof host === 'string' ? host : '')
}

/**
 * The site names typed into a web task's list: split on lines, commas and spaces, lower case, a pasted address reduced to its host,
 * no repeats. This only helps the typing; the host checks every name again and refuses what is not a plain host name.
 */
export function parseSites(text) {
  const out = []
  for (const piece of String(text || '').split(/[\s,;]+/)) {
    let name = piece.trim().toLowerCase()
    if (!name) continue
    if (name.includes('/') || name.includes('://')) {
      try { name = new URL(name.includes('://') ? name : `https://${name}`).hostname } catch { /* kept as typed: the host refuses it */ }
    }
    if (!out.includes(name)) out.push(name)
  }
  return out
}

// ---- file tree and editor ----

/** `dir` and `name` as one project-relative path with `/` ('' is the project folder). */
export const relJoin = (dir, name) => (dir ? `${dir}/${name}` : name)

/** The folder of a project-relative path ('' for a top-level entry or the project itself). */
/** True when `path` is `root` or lies inside it (`root` is a folder path; the empty root holds everything). */
export function isUnder(path, root) {
  return root === '' || path === root || String(path).startsWith(root + '/')
}

export function relParent(path) {
  const at = String(path).lastIndexOf('/')
  return at < 0 ? '' : String(path).slice(0, at)
}

/** The line numbers of an editor's gutter: "1\n2\n...n" (at least one line). */
export const gutterText = lines => Array.from({ length: Math.max(1, Math.floor(lines) || 1) }, (_, i) => i + 1).join('\n')

/** How many lines a text has (an empty text is one line). */
export const lineCount = text => String(text).split('\n').length

/** A textarea gives its text with LF only; a file that had CRLF gets them back on save. */
export const toCrlf = text => String(text).replace(/\r?\n/g, '\r\n')

/** The text a textarea shows for a file's text (a textarea turns CRLF into LF by itself; this is the same rule, for comparing). */
export const asTextarea = text => String(text).replace(/\r\n/g, '\n')

/** A line diff, or null when either text has more than `limit` lines (the diff table grows with the product of the two). */
export function diffWithin(before, after, limit = 2000) {
  if (lineCount(before) > limit || lineCount(after) > limit) return null
  return lineDiff(before, after)
}

// ---- terminal output as plain text ----
// A shell's output is turned into lines of text and nothing else: every escape sequence (colours, cursor moves, window titles, the
// clipboard, links) is dropped, and the result only ever goes into textContent. No query is ever answered.

const TERM_LINE = 1000 // a longer line wraps, so one endless line cannot grow without bound (2000 lines of it stay cheap to draw)

export function termState(max = 2000) {
  return { lines: [''], col: 0, mode: 'text', held: 0, max }
}

function termWrite(st, ch) {
  const last = st.lines.length - 1
  const line = st.lines[last]
  if (line.length >= TERM_LINE && st.col >= line.length) {
    st.lines.push(ch)
    st.col = ch.length
    return
  }
  st.lines[last] = st.col >= line.length ? line + ch : line.slice(0, st.col) + ch + line.slice(st.col + ch.length)
  st.col += ch.length
}

// A control character acts in the middle of a sequence too (a newline still ends the line); true when it was one of those.
function termControl(st, ch) {
  if (ch === '\n') {
    st.lines.push('')
    st.col = 0
    if (st.lines.length > st.max + 200) st.lines.splice(0, st.lines.length - st.max)
  } else if (ch === '\r') st.col = 0
  else if (ch === '\b') st.col = Math.max(0, st.col - 1)
  else if (ch === '\t') termWrite(st, ch)
  else return false
  return true
}

/** Feeds decoded text (any chunking: a sequence cut between chunks is carried over) into the screen `st`. */
export function termFeed(st, text) {
  for (const ch of text) {
    const c = ch.codePointAt(0)
    if (st.mode === 'esc') {
      if (ch === '[') { st.mode = 'csi'; st.held = 0; continue }
      if (ch === ']' || ch === 'P' || ch === '_' || ch === '^' || ch === 'X') { st.mode = 'str'; st.held = 0; continue }
      if (ch === '\x1b' || (c >= 0x20 && c <= 0x2f)) continue // a new ESC, or an intermediate byte: the sequence goes on
      if (c < 0x20) { termControl(st, ch); continue } // a control character inside it acts, the sequence goes on
      st.mode = 'text' // the last byte of a short sequence
      continue
    }
    if (st.mode === 'csi') {
      if (c >= 0x40 && c <= 0x7e) st.mode = 'text'
      else if (ch === '\x1b') st.mode = 'esc'
      else if (c < 0x20) termControl(st, ch)
      else if (++st.held > 256) st.mode = 'text'
      continue
    }
    if (st.mode === 'str') {
      if (ch === '\x07' || ch === '\u009c' || ch === '\x18' || ch === '\x1a') st.mode = 'text' // BEL, ST, CAN, SUB
      else if (ch === '\x1b') st.mode = 'stesc'
      else if (++st.held > 4096) st.mode = 'text'
      continue
    }
    if (st.mode === 'stesc') {
      if (ch === '\\') st.mode = 'text'
      else { st.mode = 'esc'; termFeed(st, ch) }
      continue
    }
    if (ch === '\x1b') st.mode = 'esc'
    else if (ch === '\u009b') { st.mode = 'csi'; st.held = 0 } // the one-character forms of CSI and of the string starters
    else if (ch === '\u009d' || ch === '\u0090' || ch === '\u0098' || ch === '\u009e' || ch === '\u009f') { st.mode = 'str'; st.held = 0 }
    else if (termControl(st, ch)) continue
    else if (c < 0x20 || (c >= 0x7f && c <= 0x9f)) continue // the other control characters: nothing to show
    else termWrite(st, ch)
  }
}

/** What the screen shows: the last `max` lines. */
export function termText(st) {
  return st.lines.slice(-st.max).join('\n')
}

// ---- git worktree chats: the words for what a check found and what a removal asks (pure, so they are tested) ----

/** Fills `{name}` places in a text; a place with no value is left as it is. */
export const wtText = (template, vars) => String(template).replace(/\{(\w+)\}/g, (whole, key) => (key in vars ? String(vars[key]) : whole))

/** One sentence for a code in a check's `blockers` (`wtB_<code>` in the text table). */
export function wtBlockerText(code, t, st = {}) {
  const list = Array.isArray(st.conflicts) ? st.conflicts : []
  const more = list.length - 3 + (Number(st.conflicts_more) || 0)
  const files = list.slice(0, 3).join(', ') + (more > 0 ? ` +${more}` : '')
  return wtText(t('wtB_' + code), { main: st.main_branch || '?', base: st.base_branch || '', files })
}

/** The questions before removing: the first is always asked (`force`: with changes that would be lost), the second only when commits would be left on no other branch. */
export function wtRemovePrompts(st, t) {
  const vars = { path: st.path || '', base: st.base_branch || '', branch: st.branch || '', n: st.uncommitted || 0 }
  const force = (st.uncommitted || 0) > 0
  const first = wtText(t('wtRemove1'), vars) + (force ? wtText(t('wtRemoveLoss'), vars) : '')
  // Asked unless every commit is in the base's history; content that only looks merged (a squash) is said so, and still asked.
  const second = (st.ahead || 0) > 0 && !st.in_base ? wtText(t(st.same_content ? 'wtRemove2b' : 'wtRemove2'), { ...vars, n: st.ahead }) : null
  return { force, first, second }
}

// ---- accounts: which account a panel is showing, and which messages are about it (pure, so they are tested) ----

const ACCOUNT_OP = /^(memory_(?!setting$)\w+|skill_(?!setting$)\w+|search)$/

/** True for the messages that act on one account's notes, skills or history (the host refuses them without an `account` key). */
export const needsAccount = name => ACCOUNT_OP.test(String(name))

/** The message with the account a panel is showing (an id, or null for the default account), unless it already names one. */
export function withAccount(op, account) {
  if (!op || !needsAccount(op.op) || 'account' in op) return op
  return { ...op, account: account ?? null }
}

/** True when an event is about what a panel shows: the account it names (none or null is the default one), or every account. */
export const forAccount = (data, view) => Boolean(data) && (data.all === true || (data.account ?? null) === (view ?? null))

/** The account a chat runs as: its id, or null for the default account. */
export const accountOf = session => (session && session.account && typeof session.account.id === 'string' ? session.account.id : null)

/**
 * True when a chat is listed under the account the panels show. A chat of the default account, a provider chat (it has no account)
 * and a chat of an account that was deleted all belong to the default one, so none of them becomes unreachable. `known` is every
 * account id the host listed (null before the list came: a chat of an extra account is then only listed under its own account).
 */
export function inContext(session, view, known) {
  const a = accountOf(session)
  const shown = view ?? null
  if (a === null) return shown === null
  if (known && !known.includes(a)) return shown === null
  return a === shown
}

/** The account a chat is shown under: its own, or null (the default one) when it has none or its account is gone. */
export function viewOf(session, known) {
  const a = accountOf(session)
  return a && known && !known.includes(a) ? null : a
}

/** What waits or runs in the chats that are listed under another account than the one shown: `{ wait, run }` per account id ('' = default). */
export function attn(sessions, view, known) {
  const out = new Map()
  for (const s of sessions || []) {
    const owner = viewOf(s, known) ?? ''
    if (owner === (view ?? '')) continue
    const e = out.get(owner) || { wait: 0, run: 0 }
    if ((s.pending || 0) > 0) e.wait += 1
    else if (s.state === 'running') e.run += 1
    out.set(owner, e)
  }
  return out
}

/** The permission modes a new chat may start with, by the name Claude Code takes after `--permission-mode` (the list is exactly those). */
export const PERM_MODES = ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions']

/**
 * The modes offered for a chat of this kind: all six, whatever the chat (a provider chat too). Skipping every check is asked about first
 * (see `permNeedsConfirm`).
 */
export function permChoices() {
  return PERM_MODES
}

/** True for the mode that skips every check: it is only sent after the person confirmed it. */
export const permNeedsConfirm = mode => mode === 'bypassPermissions'

/**
 * What the new-chat form takes from what the core remembers for its folder (the `project_defaults` answer): the fields to set, by the
 * form's own values; a field that is not in the result stays as it is. `touched` holds the fields the person set by hand in this form
 * ('model', 'account', 'permission', 'worktree'). `have` lists what the form offers: `models`, `accounts` ('' is the default account) and
 * `perms`. Nothing for an answer that is for another folder (a late one) or that found nothing; never the mode that skips every check; an
 * account, model or mode the form does not offer is left alone, and so is the account when the answer has no `account` at all (not known any more).
 */
export function formFill(answer, asked, touched, have) {
  const out = {}
  if (!answer || answer.found !== true || answer.cwd !== asked) return out
  if (!touched.has('model')) {
    const model = answer.model == null ? '' : answer.model
    if (have.models.includes(model)) out.model = model
  }
  if (!touched.has('account') && 'account' in answer) {
    const account = answer.account == null ? '' : answer.account
    if (have.accounts.includes(account)) out.account = account
  }
  if (!touched.has('permission') && typeof answer.permission === 'string' && answer.permission !== 'bypassPermissions' && have.perms.includes(answer.permission)) {
    out.permission = answer.permission
  }
  if (!touched.has('worktree') && typeof answer.worktree === 'boolean') out.worktree = answer.worktree
  return out
}

/** The mode a chat entry carries, or null for the default (nothing stored). Only a name from the list counts. */
export const permOf = session => (session && PERM_MODES.includes(session.permission_mode) && session.permission_mode !== 'default' ? session.permission_mode : null)

/**
 * The buttons that ask Claude Code to remember a rule, from what the host said about a permission request (`ask.grants`: the rule as
 * text for each kind, null when it cannot be offered). The page never builds a rule: it names the kind, and the host builds it from
 * the request's own suggestion.
 */
export function grantButtons(ask) {
  const g = ask && ask.grants
  if (!g || typeof g !== 'object') return []
  const out = []
  for (const grant of ['session', 'always']) {
    const rule = g[grant]
    if (typeof rule === 'string' && rule.length > 0 && rule.length <= 4100) out.push({ grant, rule, exact: g.exact === true })
  }
  return out
}

/** The name to show for an account id: as it is named now (it can be renamed), else the name the chat kept, else a short id. */
export function accountName(id, accounts, kept) {
  if (!id) return ''
  const now = (accounts || []).find(a => a.id === id)
  return (now && now.name) || kept || id.slice(0, 6)
}

// ---- chat output: thinking, code, edits, tool headers, sub-agents (pure, so they are tested; the page only draws what these return) ----

/** What a finished thinking block says about itself: its length, and its seconds when they were measured (a replayed chat has none). */
export function thinkSummary(chars, ms) {
  const n = Math.max(0, Number(chars) || 0)
  const secs = Number.isFinite(ms) && ms >= 0 ? Math.max(1, Math.round(ms / 1000)) : null
  return { chars: n, secs }
}

/** The language a fenced block names (marked puts it in the class of its `code`); '' when there is none or it looks wrong. */
export function codeLang(className) {
  const m = /(?:^|\s)language-([\w+#.-]{1,20})(?:\s|$)/.exec(String(className || ''))
  return m ? m[1] : ''
}

/** How long a text is and whether it is long enough to be folded. */
export function foldInfo(text, max = 30) {
  const lines = String(text).replace(/\n$/, '').split('\n').length
  return { lines, folded: lines > max }
}

/** The first `maxLines` lines of a tool's output, all of it (cut at `maxChars`), and what was left out. */
export function foldText(text, maxLines = 12, maxChars = 20000) {
  const s = String(text)
  const cut = s.length > maxChars
  const full = cut ? s.slice(0, maxChars) : s
  const all = full.split('\n')
  return { head: all.slice(0, maxLines).join('\n'), full, lines: all.length, cut, folded: all.length > maxLines }
}

/** The changed lines of an Edit, MultiEdit or Write call: { rows: [{ t, text }] | null, add, del }, or null for any other call. `rows` is null when the texts are too long to compare. */
export function editDiff(name, input, limit = 400) {
  const o = input && typeof input === 'object' ? input : null
  if (!o) return null
  const count = (rows, add, del) => ({ rows, add, del })
  if (name === 'Write' && typeof o.content === 'string') {
    const all = o.content.split('\n')
    if (all.length > limit) return count(null, all.length, 0)
    return count(all.map(text => ({ t: '+', text })), all.length, 0)
  }
  const edits = name === 'Edit' ? [o] : name === 'MultiEdit' && Array.isArray(o.edits) ? o.edits : null
  if (!edits || !edits.length) return null
  const rows = []
  let add = 0
  let del = 0
  let tooBig = false
  for (const e of edits) {
    if (!e || typeof e.old_string !== 'string' || typeof e.new_string !== 'string') return null
    const part = diffWithin(e.old_string, e.new_string, limit)
    if (!part) { tooBig = true; add += lineCount(e.new_string); del += lineCount(e.old_string); continue }
    if (rows.length) rows.push({ t: '…', text: '' })
    for (const r of part) { rows.push(r); if (r.t === '+') add++; else if (r.t === '-') del++ }
  }
  return count(tooBig ? null : rows, add, del)
}

const HEAD_LABEL = { Bash: 'cmd', Read: 'read', Grep: 'grep', Glob: 'glob', WebFetch: 'url', WebSearch: 'query', TodoWrite: 'todo' }

/** The checklist a TodoWrite call carries: [{ text, status }] (an unknown status is dropped, at most 50 items). */
export function todoItems(input) {
  const list = input && Array.isArray(input.todos) ? input.todos : []
  const out = []
  for (const it of list) {
    if (!it || typeof it !== 'object') continue
    if (!['pending', 'in_progress', 'completed'].includes(it.status)) continue
    const text = typeof it.content === 'string' ? it.content : typeof it.activeForm === 'string' ? it.activeForm : ''
    out.push({ text: text.slice(0, 200), status: it.status })
    if (out.length >= 50) break
  }
  return out
}

/** A one-line head for the tools that have a clear main argument: { label, text, extra }. Other tools have none (toolSummary is used). */
export function toolHead(name, input) {
  const o = input && typeof input === 'object' ? input : {}
  const first = v => (typeof v === 'string' ? v.split('\n')[0].slice(0, 160) : '')
  const label = HEAD_LABEL[name]
  if (!label) return null
  if (name === 'Bash') return { label, text: first(o.command), extra: first(o.description) }
  if (name === 'Read') {
    const from = Number(o.offset)
    const len = Number(o.limit)
    const range = Number.isFinite(from) && Number.isFinite(len) && len > 0 ? `:${from}–${from + len}` : ''
    return { label, text: first(o.file_path) + range, extra: '' }
  }
  if (name === 'Grep') return { label, text: first(o.pattern), extra: first(o.path || o.glob) }
  if (name === 'Glob') return { label, text: first(o.pattern), extra: first(o.path) }
  if (name === 'WebFetch') return { label, text: first(o.url), extra: '' }
  if (name === 'WebSearch') return { label, text: first(o.query), extra: '' }
  const items = todoItems(o)
  return { label, text: '', extra: `${items.filter(i => i.status === 'completed').length}/${items.length}` }
}

// One table for the badges, so a renamed plug-in is one edit.
const BADGE_AGENTS = { 'smartworkbench:advisor': 'advisor', 'smartworkbench:explorer': 'explorer' }
const BADGE_MCP = { plugin_vhdb_vhdb: 'vhdb', smartworkbench: 'swb', tenrec: 'tenrec' }

/** A small tag for a tool call: the sub-agent it starts (and its role), or the MCP server it asks. */
export function toolBadge(name, input) {
  const n = String(name || '')
  if (n === 'Task' || n === 'Agent') {
    const type = input && typeof input.subagent_type === 'string' ? input.subagent_type.slice(0, 40) : ''
    return type ? { text: type.replace(/^smartworkbench:/, ''), kind: BADGE_AGENTS[type] || 'agent' } : null
  }
  const m = /^mcp__(.+?)__/.exec(n)
  return m ? { text: BADGE_MCP[m[1]] || m[1].slice(0, 20), kind: 'mcp' } : null
}

/** True for the tools that start a sub-agent, whose own events carry this call's id as their `parent_tool_use_id`. */
export const isAgentCall = name => name === 'Task' || name === 'Agent'

// ---- system events worth a line, plug-in guards, and the plug-in chips (measured with claude 2.1.322; see the notes on what is not) ----

/**
 * One thin line for the few system events that matter, or null (a turn brings about eight hook_started/hook_response pairs, all of them fine):
 * a hook that failed, usage near its limit, and (UNVERIFIED: the VHDB switch for it is off by default) VHDB history put into the prompt.
 * Nothing the hook printed is ever put in a note, only its name and code. `key` is stable, so the page shows a replayed event once.
 */
export function systemNote(d, now = Date.now()) {
  if (!d || typeof d !== 'object') return null
  if (d.type === 'rate_limit_event') {
    const info = d.rate_limit_info && typeof d.rate_limit_info === 'object' ? d.rate_limit_info : {}
    const wins = info.unifiedWindows && typeof info.unifiedWindows === 'object' ? Object.entries(info.unifiedWindows) : [[info.rateLimitType || 'limit', { utilization: info.utilization, status: info.status, resetsAt: info.resetsAt }]]
    let top = null
    for (const [name, w] of wins) {
      if (!w || typeof w !== 'object') continue
      const util = Number(w.utilization)
      const status = typeof w.status === 'string' ? w.status : ''
      const reset = Number(w.resetsAt)
      if (Number.isFinite(reset) && reset * 1000 <= now) continue // a window that is over says nothing now (a replayed event)
      if (!(util >= 0.8) && (!status || status === 'allowed')) continue
      const score = Number.isFinite(util) ? util : 1
      if (!top || score > top.score) top = { name, util, status, reset, score }
    }
    if (!top) return null
    const pct = Number.isFinite(top.util) ? Math.round(top.util * 100) : null
    return { code: 'rate', level: 'warn', scope: 'window', key: `${top.name}:${top.status}:${pct == null ? '' : Math.floor(top.util * 20)}`, vars: { window: top.name, pct, at: Number.isFinite(top.reset) ? top.reset : null, status: top.status } }
  }
  if (d.type === 'system' && d.subtype === 'hook_response') {
    if (String(d.output || '').includes('<vhdb_history')) return { code: 'vhdb', level: 'info', scope: 'turn', key: 'vhdb', vars: {} }
    const failed = (typeof d.outcome === 'string' && d.outcome !== 'success') || (typeof d.exit_code === 'number' && d.exit_code !== 0)
    if (!failed) return null
    const name = String(d.hook_name || d.hook_event || 'hook').slice(0, 80)
    const code = typeof d.exit_code === 'number' ? d.exit_code : null
    const outcome = typeof d.outcome === 'string' ? d.outcome : ''
    return { code: code === null ? 'hook-outcome' : 'hook', level: code === 2 ? 'info' : 'warn', scope: 'turn', key: `hook:${name}:${code}:${outcome}`, vars: { name, code, outcome } }
  }
  return null
}

/** 'team' for a tool result the team-mode edit guard held, 'vhdb' for one the VHDB guard held (UNVERIFIED wording), else null. */
export function guardKind(content) {
  const text = resultBlocks(content).filter(p => p.type === 'text').map(p => p.text).join('\n').slice(0, 400)
  if (text.includes('SmartWorkbench team mode:')) return 'team'
  if (/VHDB guard/i.test(text)) return 'vhdb'
  // The guard asked and, with no terminal to ask in, its question ended as a decline: nobody here refused it.
  if (text.includes('The user declined this') && text.includes('in SmartWorkbench')) return 'swb_ask'
  return null
}

/** Which plug-ins a chat is wired to, from its system/init event. This says they are connected, not that they are switched on. */
export function initChips(d) {
  const o = d && typeof d === 'object' ? d : {}
  const team = Array.isArray(o.agents) && o.agents.some(a => typeof a === 'string' && a.startsWith('smartworkbench:'))
  const vhdb = Array.isArray(o.mcp_servers) && o.mcp_servers.some(s => s && /vhdb/i.test(String(s.name)) && s.status === 'connected')
  return { team, vhdb }
}

// ---- the Stitch layout: times, model names, the turn line, numbered code ----

/** The local day of a time (ms), as `YYYY-MM-DD`; '' for no time (logs written before times were kept carry 0). */
export function dayKey(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  const d = new Date(ms)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

/** `HH:MM` of a time (ms) in local time; '' for no time. */
export function clock(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  const d = new Date(ms)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** How long ago, short: `방금`/`now`, `5m`, `3h`, `2d`, else the date. `now` is passed so it can be tested. */
export function ago(ms, now, justNow = 'now') {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 60) return justNow
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h`
  if (s < 86400 * 7) return `${Math.floor(s / 86400)}d`
  return dayKey(ms).slice(5)
}

/** A model id as a short name: `claude-sonnet-4-5-20250929` → `sonnet-4-5`; an alias stays as it is. */
/** The model names a chat can be switched to (Claude Code's aliases, documented): "" is its default. A full model id can also be a chat's model. */
export const MODEL_ALIASES = ['', 'sonnet', 'opus', 'haiku', 'fable', 'sonnet[1m]', 'opus[1m]']

/** The entries of the model picker for a chat whose model is `current` (a name that is not an alias is added, so it shows as it is). */
export function modelChoices(current) {
  return current && !MODEL_ALIASES.includes(current) ? [...MODEL_ALIASES, current] : MODEL_ALIASES
}

export function modelLabel(id) {
  if (typeof id !== 'string' || !id) return ''
  return id.replace(/^claude-/, '').replace(/-\d{8}$/, '').replace(/\[1m\]$/, ' 1M').slice(0, 40)
}

/** The header line of the last turn: its cost and its tokens (input with cache plus output), or null when the result has neither. */
export function turnLine(result) {
  if (!result || typeof result !== 'object') return null
  const cost = typeof result.total_cost_usd === 'number' ? '$' + result.total_cost_usd.toFixed(3) : null
  const u = result.usage && typeof result.usage === 'object' ? result.usage : null
  const n = k => (u && Number.isFinite(u[k]) ? u[k] : 0)
  const total = u ? n('input_tokens') + n('cache_read_input_tokens') + n('cache_creation_input_tokens') + n('output_tokens') : 0
  const tokens = total > 0 ? (total >= 1000 ? (total / 1000).toFixed(1) + 'k' : String(total)) : null
  if (!cost && !tokens) return null
  return { cost, tokens }
}

/** The lines of a code block for numbering, with `+`/`-` marked when it is a diff; null past `max` lines (left as one block). */
export function codeLines(text, lang = '', max = 2000) {
  const body = String(text).replace(/\n$/, '')
  const lines = body.split('\n')
  if (lines.length > max) return null
  const diff = /^(diff|patch)$/i.test(lang)
  return lines.map(line => ({ text: line, kind: diff && line.startsWith('+') && !line.startsWith('+++') ? 'add' : diff && line.startsWith('-') && !line.startsWith('---') ? 'del' : '' }))
}

/** The end of a long path for a narrow pill: `…/a/b` keeping as many last folders as fit in `max` characters. */
export function tailPath(path, max = 36) {
  const text = String(path || '')
  if (text.length <= max) return text
  const sep = text.includes('\\') && !text.includes('/') ? '\\' : '/'
  const parts = text.split(/[\\/]/).filter(Boolean)
  let out = parts.pop() || ''
  while (parts.length && out.length + parts[parts.length - 1].length + 1 <= max - 2) out = parts.pop() + sep + out
  return '…' + sep + out
}
