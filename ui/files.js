// The file tree and the text editor: the project folder of the open chat, nothing outside it. Every answer arrives on the socket as
// `fs_*`; a file's text only ever goes into a textarea's value and into textContent (never innerHTML), and is kept nowhere else.
import { asTextarea, diffWithin, fmtBytes, gutterText, isUnder, lineCount, relJoin, relParent, toCrlf } from './util.js'

const TRASH = '.tenrec-trash'

export function initFiles({ $, el, t, send, toast, sid }) {
  const st = { req: 0, readSeq: 0, waits: new Map(), dirs: new Map(), open: new Set(['']), file: null, saving: false, fetching: false, forSid: null, checkSeq: 0, checking: false, cur: '' }
  const ta = $('ed-text')
  const gutter = $('ed-gutter')
  const dlg = $('fs-dlg')

  // ---- requests ----
  function ask(op, fn) {
    const current = sid()
    if (!current) return
    const req = ++st.req
    // A closed socket drops the message: the caller hears an error at once, so nothing waits for an answer that cannot come.
    if (!send({ ...op, sid: current, req })) { fn({ kind: 'fs_error', data: { message: t('fsOffline') } }); return }
    st.waits.set(req, { sid: current, fn })
  }

  /** The socket closed: every request still waiting gets the error (an answer cannot come back on a new socket). */
  function disconnected() {
    const waiting = [...st.waits.values()]
    st.waits.clear()
    for (const w of waiting) if (w.sid === sid()) w.fn({ kind: 'fs_error', data: { message: t('fsOffline') } })
  }

  /** The page's single entry for `fs_*` messages. */
  function onMessage(m) {
    const wait = st.waits.get(m.data && m.data.req)
    if (!wait) return
    st.waits.delete(m.data.req)
    if (wait.sid !== sid()) return
    wait.fn(m)
  }

  // ---- the tree ----
  function loadDir(path) {
    ask({ op: 'fs_list', path }, m => {
      if (m.kind === 'fs_error') { toast(m.data.message); st.dirs.delete(path); st.open.delete(path); renderTree(); return }
      st.dirs.set(path, m.data)
      renderTree()
    })
  }

  function refreshTree() {
    st.dirs.clear()
    if (!sid()) { renderTree(); return }
    for (const path of st.open) loadDir(path)
    renderTree()
  }

  function badge(flag) {
    const tag = el('span', `fs-tag ${flag}`, t(`fsTag_${flag}`))
    tag.title = t(`fsTip_${flag}`)
    return tag
  }

  function rows(parent, dir, depth) {
    const listing = st.dirs.get(dir)
    if (!listing) return
    for (const e of listing.entries) {
      const path = relJoin(dir, e.name)
      const item = el('div', 'fs-item')
      const row = el('button', 'fs-row' + (st.file && st.file.path === path ? ' on' : ''))
      row.type = 'button'
      row.style.paddingLeft = `${8 + depth * 14}px`
      const isDir = e.kind === 'dir'
      row.append(el('span', 'fs-caret', isDir ? (st.open.has(path) ? '▾' : '▸') : ''), el('span', 'fs-name', e.name + (e.link ? ' ↪' : '')))
      if (e.from) { const from = el('span', 'muted fs-from', '← ' + e.from); from.title = `${t('fsFrom')}: ${e.from}`; row.append(from) }
      for (const flag of e.flags) row.append(badge(flag))
      if (e.kind === 'file') row.append(el('span', 'fs-size', fmtBytes(e.size)))
      if (e.kind === 'other') {
        row.disabled = true
        row.title = t('fsOther')
      } else if (isDir) {
        row.addEventListener('click', () => {
          st.cur = path
          if (st.open.has(path)) st.open.delete(path)
          else { st.open.add(path); if (!st.dirs.has(path)) loadDir(path) }
          renderTree()
        })
      } else {
        row.addEventListener('click', () => { st.cur = relParent(path); openFile(path) })
      }
      item.appendChild(row)
      if (e.kind !== 'other') {
        const more = el('button', 'fs-more', '⋯')
        more.type = 'button'
        more.title = t('fsMore')
        more.setAttribute('aria-label', t('fsMore') + ': ' + e.name)
        more.addEventListener('click', () => entryMenu(path, e))
        item.appendChild(more)
      }
      parent.appendChild(item)
      if (isDir && st.open.has(path)) rows(parent, path, depth + 1)
    }
    if (listing.truncated) parent.appendChild(el('div', 'muted fs-note', t('fsTruncated')))
    if (listing.unreadable) parent.appendChild(el('div', 'muted fs-note', t('fsUnreadable').replace('{n}', String(listing.unreadable))))
    if (!listing.entries.length && !listing.truncated) parent.appendChild(el('div', 'muted fs-note', t('fsEmpty')))
  }

  function renderTree() {
    const box = $('fs-tree')
    box.replaceChildren()
    if (!sid()) { box.appendChild(el('div', 'muted fs-note', t('fsNoChat'))); return }
    if (!st.dirs.has('')) { box.appendChild(el('div', 'muted fs-note', t('fsLoading'))); return }
    rows(box, '', 0)
  }

  // ---- the editor ----
  const dirty = () => !!st.file && ta.value !== st.file.orig

  function updateGutter() {
    gutter.textContent = gutterText(lineCount(ta.value))
    gutter.scrollTop = ta.scrollTop
  }

  function updateHead() {
    const f = st.file
    $('ed-dirty').hidden = !dirty()
    $('ed-save').disabled = !f || f.readonly || !dirty() || st.saving
  }

  function close() {
    st.file = null
    $('main').classList.remove('editing')
    renderTree()
    $('input').focus()
  }

  /** True when the editor may be left: nothing unsaved, or the person agreed to lose it. */
  function leave() {
    if (st.saving) { toast(t('fsSaving')); return false }
    if (!dirty()) return true
    return confirm(t('fsDiscard'))
  }

  /** The note under the file's name that belongs to the file itself (read-only reasons, the guard warning). */
  function baseNote(d) {
    return d.eol === 'mixed' ? t('fsReadonlyMixed') : d.guard_kind === 'block' ? t('fsReadonlyBlocked') : d.guard_kind === 'confirm' ? t('fsGuardedNote') : ''
  }

  function setNote(text) {
    const note = $('ed-note')
    note.textContent = text
    note.hidden = !text
  }

  /** `quiet`: the same file read again by the app (not opened by the person), so the view stays where it was and takes no focus. */
  function show(path, d, quiet) {
    const keep = quiet ? { top: ta.scrollTop, at: ta.selectionStart } : null
    const readonly = d.eol === 'mixed' || d.guard_kind === 'block'
    st.file = { path, real: d.real || path, base: d.version, orig: '', eol: d.eol, guardKind: d.guard_kind, readonly }
    ta.value = d.text
    st.file.orig = ta.value // what the textarea made of it (CRLF turned into LF), so "changed" compares like with like
    ta.readOnly = readonly
    $('ed-path').textContent = path
    st.file.note = baseNote(d)
    setNote(st.file.note)
    $('main').classList.add('editing')
    updateGutter()
    updateHead()
    renderTree()
    if (keep) {
      const at = Math.min(keep.at, ta.value.length)
      ta.setSelectionRange(at, at)
      ta.scrollTop = keep.top
      gutter.scrollTop = keep.top
    } else {
      ta.scrollTop = 0
      gutter.scrollTop = 0
      ta.focus()
    }
  }

  // While a file is being fetched the editor takes no typing, so nothing typed in that time is overwritten by the answer.
  function fetchFile(path, quiet) {
    st.fetching = true
    ta.readOnly = true
    // Only the last file asked for is shown: an earlier answer arriving late is dropped, so it cannot replace what was typed.
    const mine = ++st.readSeq
    ask({ op: 'fs_read', path }, m => {
      if (mine !== st.readSeq) return
      st.fetching = false
      if (m.kind === 'fs_error') { ta.readOnly = !!(st.file && st.file.readonly); toast(m.data.message); return }
      show(path, m.data, quiet)
      if (quiet) toast(t('fsDiskReloaded'))
    })
  }

  /**
   * Looks at the open file on disk (after a turn ends, when the page comes back, when the connection returns). Unchanged: nothing.
   * Changed and nothing typed: read again in place. Changed and text typed: a note only; saving then asks (`fs_conflict`).
   * An answer is judged against the file and `base` as they are when it arrives, because a save can finish in between.
   */
  function checkDisk() {
    const f = st.file
    if (!f || !sid() || st.fetching || st.saving || st.checking || dlg.open) return
    const mine = ++st.checkSeq
    const base = f.base
    st.checking = true // before the request: a closed socket answers at once
    ask({ op: 'fs_check', path: f.path }, m => {
      st.checking = false
      if (mine !== st.checkSeq || st.file !== f || f.base !== base || st.fetching || st.saving) return
      if (m.kind === 'fs_error') { setNote(m.data.message); return } // no toast: this runs again at every focus
      if (m.data.version === f.base) { f.seen = null; setNote(f.note); return }
      if (dirty()) {
        if (f.seen !== m.data.version) { f.seen = m.data.version; setNote(t('fsDiskChanged')) }
        return
      }
      f.seen = null
      fetchFile(f.path, true)
    })
  }

  function openFile(path) {
    if (!leave()) return
    fetchFile(path)
  }

  function reload(force) {
    const f = st.file
    if (!f || (force !== true && !leave())) return
    fetchFile(f.path)
  }

  function diffBox(before, after) {
    const diff = diffWithin(before, after)
    if (!diff) return el('div', 'muted', t('fsDiffTooBig'))
    const box = el('div', 'skdiff')
    for (const d of diff) {
      const row = el('div', 'line ' + (d.t === '+' ? 'add' : d.t === '-' ? 'del' : ''))
      row.append(el('span', 'n', d.t === ' ' ? '' : d.t), el('span', '', d.text))
      box.appendChild(row)
    }
    return box
  }

  function dialog(title, intro, body, buttons) {
    dlg.replaceChildren(el('h3', '', title), el('p', '', intro), body)
    const acts = el('div', 'actions')
    for (const [label, cls, run] of buttons) {
      const b = el('button', cls, label)
      b.type = 'button'
      b.addEventListener('click', () => { dlg.close(); if (run) run() })
      acts.appendChild(b)
    }
    dlg.appendChild(acts)
    if (!dlg.open) dlg.showModal()
  }

  function save(confirmed) {
    const f = st.file
    if (!f || f.readonly || st.saving || st.fetching || !dirty()) return
    const value = ta.value
    st.saving = true
    updateHead()
    ask({ op: 'fs_write', path: f.path, text: f.eol === 'crlf' ? toCrlf(value) : value, base: f.base, confirm: confirmed || undefined }, m => {
      st.saving = false
      if (st.file !== f) return
      if (m.kind === 'fs_saved') {
        f.base = m.data.version
        f.orig = value
        f.seen = null
        setNote(f.note)
        toast(t('fsSaved'))
      } else if (m.kind === 'fs_needs_confirm') {
        dialog(t('fsConfirmTitle'), t('fsConfirmBody') + (f.real !== f.path ? ` (${t('fsRealPath')}: ${f.real})` : ''), diffBox(f.orig, value), [[t('cancel'), '', null], [t('fsConfirmSave'), 'primary', () => save(true)]])
      } else if (m.kind === 'fs_conflict') {
        const disk = asTextarea(m.data.text)
        dialog(t('fsConflictTitle'), t('fsConflictBody'), diffBox(disk, value), [
          [t('cancel'), '', null],
          [t('fsConflictReload'), '', () => reload(true)],
          [t('fsConflictOverwrite'), 'primary', () => { f.base = m.data.version; save(confirmed) }],
        ])
      } else {
        toast(m.data.message)
      }
      updateHead()
    })
  }

  // ---- create, rename, delete ----
  /** Sends a change; a place that a later chat reads asks first and the same request is sent again with the confirmation. */
  function change(op, done) {
    ask(op, m => {
      if (m.kind === 'fs_needs_confirm') {
        dialog(t('fsConfirmTitle'), t('fsChangeConfirmBody'), el('div', 'muted', (m.data && m.data.to) || op.to ? `${op.path} → ${(m.data && m.data.to) || op.to}` : op.path), [[t('cancel'), '', null], [t('fsChangeConfirm'), 'primary', () => change({ ...op, confirm: true }, done)]])
      } else if (m.kind === 'fs_error') toast(m.data.message)
      else done(m)
    })
  }

  /** The tree after a change: folders that no longer exist (or moved) are not asked for again. */
  function treeChanged(gone) {
    if (gone) {
      for (const d of [...st.open]) if (d !== '' && isUnder(d, gone)) st.open.delete(d)
      for (const d of [...st.dirs.keys()]) if (d !== '' && isUnder(d, gone)) st.dirs.delete(d)
      if (isUnder(st.cur, gone)) st.cur = relParent(gone)
    }
    refreshTree()
  }

  /** `asPath`: the field holds a whole path (a move is a new folder in it); only the name part is selected. */
  function nameDialog(title, intro, value, buttons, go, asPath) {
    const input = el('input')
    input.type = 'text'
    input.maxLength = asPath ? 4096 : 255
    input.value = value
    input.placeholder = t(asPath ? 'fsPathPh' : 'fsNamePh')
    input.setAttribute('aria-label', t(asPath ? 'fsPathPh' : 'fsNamePh'))
    input.addEventListener('keydown', event => { if (event.key === 'Enter') { event.preventDefault(); dlg.close(); go(input.value) } })
    dialog(title, intro, input, buttons(input))
    input.focus()
    if (asPath) input.setSelectionRange(value.lastIndexOf('/') + 1, value.length)
    else input.select()
  }

  function createEntry(kind) {
    if (!sid()) return
    const folder = st.cur
    const go = name => {
      if (!name) return
      const path = relJoin(folder, name)
      change({ op: 'fs_create', path, kind }, () => {
        toast(t('fsCreated'))
        if (folder) st.open.add(folder)
        treeChanged(null)
        if (kind === 'file') openFile(path)
      })
    }
    nameDialog(t(kind === 'dir' ? 'fsNewFolderTitle' : 'fsNewFileTitle'), folder || '/', '', input => [[t('cancel'), '', null], [t('fsCreate'), 'primary', () => go(input.value)]], go)
  }

  function renameEntry(path, target) {
    const f = st.file
    const holds = !!f && isUnder(f.path, path)
    if (holds && !leave()) return
    change({ op: 'fs_rename', path, to: target }, () => {
      toast(t('fsRenamed'))
      // The folder it went into is shown open, so the moved entry is seen there.
      if (relParent(target)) st.open.add(relParent(target))
      treeChanged(path)
      // The open file (or the file inside the renamed folder) is read again under its new name: its guard may differ.
      if (holds && st.file === f) fetchFile(target + f.path.slice(path.length))
    })
  }

  function deleteEntry(path, e) {
    const f = st.file
    const holds = !!f && isUnder(f.path, path)
    if (holds && !leave()) return
    const inTrash = isUnder(path, TRASH) && path !== TRASH
    if (!confirm(t(inTrash ? (e.kind === 'dir' ? 'fsDeleteDirAsk' : 'fsPurgeAsk') : 'fsTrashAsk').replace('{path}', path))) return
    change({ op: 'fs_delete', path, base: holds && f.path === path ? f.base : undefined }, m => {
      const trashed = m.data && m.data.trashed
      // Into the trash: the toast offers to take it back.
      const chat = sid()
      toast(t(trashed ? 'fsTrashed' : 'fsDeleted'), trashed ? { label: t('fsUndo'), run: () => { if (sid() === chat) restoreEntry(trashed) } } : undefined)
      treeChanged(path)
      if (holds && st.file === f) close()
    })
  }

  /**
   * Deletes for good: the host first says what it would reach (a count and a size, nothing is changed), the person agrees, and
   * the same request goes again with the confirmation.
   */
  function purgeFlow(op, done) {
    ask(op, m => {
      if (m.kind === 'fs_error') { toast(m.data.message); treeChanged(null); return }
      if (m.kind !== 'fs_purge_summary') { done(m); return }
      const d = m.data
      const emptying = op.op === 'fs_empty_trash'
      const text = t(emptying ? 'fsEmptyBody' : 'fsPurgeBody').replace('{path}', op.path).replace('{entries}', String(d.entries)).replace('{items}', String(d.items)).replace('{size}', fmtBytes(d.bytes)) + (d.left ? ' ' + t('fsPurgeLeft').replace('{left}', String(d.left)) : '')
      dialog(t(emptying ? 'fsEmptyTrash' : 'fsPurgeTitle'), t('fsPurgeIntro'), el('div', 'muted', text), [[t('cancel'), '', null], [t('fsPurgeGo'), 'danger', () => purgeFlow({ ...op, confirm: true }, done)]])
    })
  }

  function purged(path, f, holds) {
    return m => {
      const left = m.data.left || 0
      toast(left ? t('fsPurgedLeft').replace('{removed}', String(m.data.removed)).replace('{left}', String(left)) : t('fsPurged'))
      treeChanged(path)
      if (holds && st.file === f) close()
    }
  }

  function purgeEntry(path, op) {
    const f = st.file
    const holds = !!f && isUnder(f.path, path)
    if (holds && !leave()) return
    purgeFlow({ op, path }, purged(path, f, holds))
  }

  /** An entry of the trash goes back to where it came from. */
  function restoreEntry(path) {
    change({ op: 'fs_restore', path }, m => {
      toast(t('fsRestored'))
      if (relParent(m.data.to)) st.open.add(relParent(m.data.to))
      treeChanged(path)
    })
  }

  function entryMenu(path, e) {
    const go = target => { if (target && target !== path) renameEntry(path, target) }
    // The trash folder itself is emptied (not deleted); an entry directly in it can be deleted for good, with what is in it.
    if (path === TRASH) {
      dialog(path, t('fsTipTrashMenu'), el('div', 'muted', ''), [[t('cancel'), '', null], [t('fsEmptyTrash'), 'danger', () => purgeEntry(TRASH, 'fs_empty_trash')]])
      return
    }
    const entryOfTrash = relParent(path) === TRASH && e.name !== '.from' && e.name !== '.gitignore'
    const restore = e.from ? [[t('fsRestore'), '', () => restoreEntry(path)]] : []
    const remove = entryOfTrash ? [t('fsPurge'), 'danger', () => purgeEntry(path, 'fs_purge')] : [t('fsDelete'), 'danger', () => deleteEntry(path, e)]
    nameDialog(path, e.from ? `${t('fsFrom')}: ${e.from}` : t('fsEntryIntro'), path, input => [[t('cancel'), '', null], ...restore, remove, [t('fsRename'), 'primary', () => go(input.value)]], go, true)
  }

  // ---- wiring ----
  $('fs-newfile').addEventListener('click', () => createEntry('file'))
  $('fs-newdir').addEventListener('click', () => createEntry('dir'))
  ta.addEventListener('input', () => { updateGutter(); updateHead() })
  ta.addEventListener('scroll', () => { gutter.scrollTop = ta.scrollTop })
  let escaped = false
  ta.addEventListener('keydown', event => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); save(false); return }
    if (event.key === 'Escape') { escaped = true; return }
    if (event.key === 'Tab' && !event.shiftKey && !ta.readOnly) {
      if (escaped) { escaped = false; return } // Esc, then Tab: leave the field, as a keyboard user needs
      event.preventDefault()
      ta.setRangeText('\t', ta.selectionStart, ta.selectionEnd, 'end')
      updateGutter()
      updateHead()
      return
    }
    escaped = false
  })
  $('ed-save').addEventListener('click', () => save(false))
  $('ed-reload').addEventListener('click', () => reload())
  $('ed-back').addEventListener('click', () => { if (leave()) close() })
  $('fs-refresh').addEventListener('click', refreshTree)
  window.addEventListener('beforeunload', event => { if (dirty()) event.preventDefault() })

  return {
    onMessage,
    disconnected,
    checkDisk,
    /** The Files tab was shown: list the project's top folder if that has not been done for this chat. */
    shown() { if (sid() && (st.forSid !== sid() || !st.dirs.has(''))) { st.forSid = sid(); loadDir('') } else renderTree() },
    /** True when the editor may be left (called before another chat is opened). */
    leave,
    /** A chat was opened: the editor and the tree belong to the chat that was open before, so they are dropped. */
    reset() {
      st.waits.clear()
      st.dirs.clear()
      st.open = new Set([''])
      st.forSid = null
      st.file = null
      st.saving = false
      st.fetching = false
      st.checking = false
      st.checkSeq++
      st.cur = ''
      if (dlg.open) dlg.close()
      $('main').classList.remove('editing')
      renderTree()
    },
    /** The sidebar tab is on screen right now. */
    reloadIfShown(visible) { if (visible) this.shown() },
  }
}
