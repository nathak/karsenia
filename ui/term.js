// The terminal view: a shell in the open chat's project folder, shown as plain text lines with one input line below. Output comes as
// binary frames ([1][tid u32][bytes] and [2][tid u32][exit code i32]); it is decoded, stripped of escape sequences by `termFeed`
// and only ever put into textContent. It is kept nowhere: not in storage, not in the debug drawer, not in the page's history.
import { termFeed, termState, termText } from './util.js'

export function initTerm({ $, t, send, toast, sid }) {
  const st = { tid: null, sid: null, req: 0, opening: 0, openSid: null, queue: [], early: [], dec: new TextDecoder('utf-8'), screen: termState(), drawn: false, exited: false, hist: [], at: -1 }
  const out = $('term-out')
  const input = $('term-in')
  const note = $('term-note')

  const alive = () => st.tid !== null && !st.exited

  function draw() {
    st.drawn = false
    const follow = out.scrollHeight - out.scrollTop - out.clientHeight < 40
    out.textContent = termText(st.screen)
    if (follow) out.scrollTop = out.scrollHeight
  }
  function schedule() {
    if (st.drawn) return
    st.drawn = true
    requestAnimationFrame(draw)
  }

  function ended(text) {
    st.exited = true
    note.textContent = text
    note.hidden = false
    input.disabled = true
    $('term-int').disabled = true
    $('term-eof').disabled = true
  }

  function fresh() {
    st.tid = null
    st.sid = null
    st.exited = false
    st.opening = 0
    st.queue = []
    st.early = []
    st.dec = new TextDecoder('utf-8')
    st.screen = termState()
    out.textContent = ''
    note.hidden = true
    input.disabled = false
    input.value = ''
    $('term-int').disabled = false
    $('term-eof').disabled = false
  }

  function show() {
    $('main').classList.add('terminal')
    $('term-title').textContent = $('title').textContent
    input.focus()
  }
  function hide() {
    $('main').classList.remove('terminal')
    $('input').focus()
  }

  /** How many characters fit across and down in the output box (the size the shell is told). */
  function size() {
    const probe = document.createElement('span')
    probe.textContent = 'M'.repeat(20)
    probe.style.visibility = 'hidden'
    probe.style.position = 'absolute'
    out.appendChild(probe)
    const width = probe.getBoundingClientRect().width / 20 || 8
    probe.remove()
    const line = parseFloat(getComputedStyle(out).lineHeight) || 19
    return { cols: Math.max(20, Math.floor(out.clientWidth / width) - 1), rows: Math.max(5, Math.floor(out.clientHeight / line)) }
  }

  function open() {
    const current = sid()
    if (!current) { toast(t('termNoChat')); return }
    fresh()
    show() // the box must be on screen to be measured
    st.opening = ++st.req
    st.openSid = current
    if (!send({ op: 'term_open', sid: current, req: st.opening, ...size() })) { st.opening = 0; toast(t('fsOffline')) }
  }

  /** The header button: show the terminal of this chat, or start one. */
  function toggle() {
    if ($('main').classList.contains('terminal')) { hide(); return }
    if (alive() && st.sid === sid()) { show(); send({ op: 'term_resize', tid: st.tid, ...size() }) } // the window may have changed while it was hidden
    else if (st.opening) show()
    else open()
  }

  function typed(data) {
    if (st.exited) return
    if (new TextEncoder().encode(data).length > 3500) { toast(t('termTooLong')); return }
    // Typed while the shell is still being started: kept (a few lines at most) and sent when it is there.
    if (st.tid === null) { if (st.opening && st.queue.length < 20) st.queue.push(data); return }
    send({ op: 'term_input', tid: st.tid, data })
  }

  function close() {
    if (alive()) send({ op: 'term_close', tid: st.tid })
    fresh()
    hide()
  }

  function onMessage(m) {
    if (m.kind === 'term_opened') {
      if (m.data.req !== st.opening) { send({ op: 'term_close', tid: m.data.tid }); return } // a terminal nobody waits for any more
      st.opening = 0
      if (m.sid !== sid()) { send({ op: 'term_close', tid: m.data.tid }); return }
      st.tid = m.data.tid
      st.sid = m.sid
      const early = st.early
      st.early = []
      for (const buffer of early) {
        const view = new DataView(buffer)
        if (view.getUint32(1) === st.tid) feed(new Uint8Array(buffer), view)
      }
      const waiting = st.queue
      st.queue = []
      for (const data of waiting) typed(data)
      input.focus()
      return
    }
    if (m.kind === 'term_error') {
      if (m.data.req !== undefined && m.data.req === st.opening) { st.opening = 0; fresh(); hide() }
      toast(m.data.message)
    }
  }

  /** One binary frame for the shell `st.tid`. */
  function feed(bytes, view) {
    if (bytes[0] === 1) {
      termFeed(st.screen, st.dec.decode(bytes.subarray(5), { stream: true }))
      schedule()
    } else if (bytes[0] === 2 && bytes.length >= 9) {
      draw()
      ended(t('termEnded').replace('{code}', String(view.getInt32(5))))
    }
  }

  /** One binary frame from the socket. */
  function onBinary(buffer) {
    const bytes = new Uint8Array(buffer)
    if (bytes.length < 5) return
    const view = new DataView(buffer)
    const tid = view.getUint32(1)
    // The first output can overtake the answer that names the shell (the socket carries both): kept a little, played when it is named.
    if (st.tid === null) { if (st.opening && st.early.length < 64) st.early.push(buffer); return }
    if (tid !== st.tid) return
    feed(bytes, view)
  }

  /** The socket closed: the shells it had are ended by the app. */
  function disconnected() {
    const starting = st.opening !== 0 && st.tid === null
    st.opening = 0
    if (alive() || starting) ended(t('termOffline'))
  }

  /** True when the view may be left for another chat: no shell is running, or the person agreed to end it. */
  function leave(target) {
    if (!alive() || st.sid === null || target === st.sid) return true
    return confirm(t('termLeave'))
  }

  // ---- wiring ----
  $('term-form').addEventListener('submit', event => {
    event.preventDefault()
    const line = input.value
    if (line.trim()) { st.hist.push(line); st.at = st.hist.length }
    typed(line + '\r')
    input.value = '' // nothing typed stays in the box, a password included
  })
  input.addEventListener('keydown', event => {
    const key = event.key.toLowerCase()
    const none = input.selectionStart === input.selectionEnd
    if (event.ctrlKey && key === 'c' && none) { event.preventDefault(); typed('\x03'); return }
    if (event.ctrlKey && key === 'd' && !input.value) { event.preventDefault(); typed('\x04'); return }
    if (event.ctrlKey && key === 'l') { event.preventDefault(); st.screen = termState(); draw(); return }
    if (event.key === 'ArrowUp' && st.hist.length) { event.preventDefault(); st.at = Math.max(0, st.at - 1); input.value = st.hist[st.at] }
    else if (event.key === 'ArrowDown' && st.hist.length) { event.preventDefault(); st.at = Math.min(st.hist.length, st.at + 1); input.value = st.hist[st.at] || '' }
  })
  input.addEventListener('paste', event => {
    const text = event.clipboardData ? event.clipboardData.getData('text') : ''
    if (!/\r|\n/.test(text.replace(/[\r\n]+$/, ''))) return // one line: an ordinary paste
    event.preventDefault()
    if (confirm(t('termPaste'))) typed(text.replace(/\r?\n/g, '\r').replace(/\r*$/, '\r'))
  })
  $('term-int').addEventListener('click', () => { typed('\x03'); input.focus() })
  $('term-eof').addEventListener('click', () => { typed('\x04'); input.focus() })
  $('term-hide').addEventListener('click', hide)
  $('term-close').addEventListener('click', close)
  $('term-toggle').addEventListener('click', toggle)
  let resizing = null
  window.addEventListener('resize', () => {
    clearTimeout(resizing)
    resizing = setTimeout(() => { if (alive() && $('main').classList.contains('terminal')) send({ op: 'term_resize', tid: st.tid, ...size() }) }, 200)
  })

  return {
    onMessage,
    onBinary,
    disconnected,
    leave,
    /** The server says whether the terminal is on: the button shows only then. */
    available(on) { $('term-toggle').hidden = !on },
    /** A chat was opened: a shell belongs to the chat it was started in, so it ends here. */
    reset() {
      const other = (st.tid !== null && st.sid !== sid()) || (st.opening && st.openSid !== sid())
      if (!other) return
      if (alive()) send({ op: 'term_close', tid: st.tid })
      fresh()
      hide()
    },
  }
}
