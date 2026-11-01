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
