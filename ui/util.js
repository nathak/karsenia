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
