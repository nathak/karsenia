// The worktree strip of a chat that works in its own git worktree: what it was made from, a check of what a merge would do, the merge
// and the removal. Everything shown is put in with textContent; what a merge or a removal acts on is decided by the host from the
// chat's id alone (the page sends back only the commits it was shown).
import { wtBlockerText, wtRemovePrompts, wtText } from './util.js'

export function initWt({ $, el, t, send, toast, sid, beforeRemove, removed }) {
  const banner = $('wt-banner')
  const line = $('wt-line')
  const panel = $('wt-panel')
  const st = { session: null, req: 0, purpose: null, status: null, busy: false }

  const vars = () => {
    const w = st.session?.worktree
    return { branch: w?.branch || '', base: w?.base_branch || '', repo: w?.repo || '' }
  }

  function busy(on) {
    st.busy = on
    for (const id of ['wt-check', 'wt-remove', 'wt-merge']) { const b = $(id); if (b) b.disabled = on }
  }

  /** Called when a chat is opened (or its record changed): the strip shows for a worktree chat only, and a removed one cannot be written to. */
  function show(session) {
    st.session = session || null
    st.status = null
    st.purpose = null
    panel.replaceChildren()
    panel.hidden = true
    const w = session?.worktree
    banner.hidden = !w
    $('input').disabled = Boolean(w?.removed)
    if (!w) return
    if (w.removed) {
      line.textContent = t('wtRemoved')
      for (const id of ['wt-check', 'wt-remove']) $(id).hidden = true
      return
    }
    line.textContent = wtText(t('wtBanner'), vars()) + ' — ' + wtText(t('wtHint'), vars())
    for (const id of ['wt-check', 'wt-remove']) $(id).hidden = false
    busy(false)
  }

  function ask(op, purpose, extra) {
    if (!sid() || st.busy) return
    st.purpose = purpose
    busy(true)
    send({ op, sid: sid(), req: ++st.req, ...extra })
  }

  function render(status) {
    st.status = status
    panel.replaceChildren()
    panel.hidden = false
    if (status.missing) {
      panel.append(el('div', 'wt-bad', wtBlockerText('missing', t, status)))
      return
    }
    panel.append(el('div', '', wtText(t('wtSummary'), { n: status.ahead, f: status.files, a: status.insertions, d: status.deletions, base: status.base_branch, b: status.behind })))
    for (const code of status.blockers) panel.append(el('div', 'wt-bad', wtBlockerText(code, t, status)))
    if (!status.blockers.length) {
      panel.append(el('div', 'wt-ok', t('wtOk')))
      const merge = el('button', 'primary', t('wtMerge'))
      merge.id = 'wt-merge'
      merge.type = 'button'
      merge.addEventListener('click', () => {
        const s = st.status
        if (!s || !confirm(wtText(t('wtMergeConfirm'), { ...vars(), n: s.ahead }))) return
        ask('wt_merge', 'merge', { tip: s.tip, main_head: s.main_head })
      })
      panel.append(merge)
    }
  }

  // The project's git settings that can run a program changed: shown as they are, and taken on only by a click on exactly what was shown.
  function renderConfig(d) {
    panel.replaceChildren()
    panel.hidden = false
    panel.append(el('div', 'wt-bad', t('wtCfgIntro')))
    for (const entry of d.entries || []) panel.append(el('code', 'wt-entry', entry))
    if (!d.hash) return
    const accept = el('button', '', t('wtCfgAccept'))
    accept.type = 'button'
    accept.addEventListener('click', () => { if (confirm(t('wtCfgConfirm'))) ask('wt_trust', 'trust', { hash: d.hash }) })
    panel.append(accept)
  }

  function remove(status) {
    const q = wtRemovePrompts(status, t)
    if (!confirm(q.first)) return
    const discard = q.second ? confirm(q.second) : false
    if (!beforeRemove()) return
    busy(true)
    send({ op: 'wt_remove', sid: sid(), req: ++st.req, force: q.force, discard, tip: status.tip })
  }

  $('wt-check').addEventListener('click', () => ask('wt_status', 'check'))
  $('wt-remove').addEventListener('click', () => ask('wt_status', 'remove'))

  function onMessage(m) {
    const d = m.data || {}
    // A removal is remembered whichever chat is open: the list and the chat's own page follow.
    if (m.kind === 'wt_removed') {
      if (m.sid === sid()) busy(false)
      toast(t('wtDone') + (d.branch_kept ? wtText(t('wtKept'), { branch: d.branch }) : '') + (d.warning ? ' ' + t('wtW_' + d.warning) : ''))
      removed(m.sid)
      return
    }
    if (m.sid !== sid()) return
    if (m.kind === 'wt_status') {
      busy(false)
      const purpose = st.purpose
      st.purpose = null
      if (purpose === 'remove') return remove(d)
      render(d)
      return
    }
    if (m.kind === 'wt_trusted') {
      busy(false)
      toast(t('wtCfgDone'))
      ask('wt_status', 'check')
      return
    }
    if (m.kind === 'wt_merged') {
      busy(false)
      toast(t('wtMerged'))
      ask('wt_status', 'check')
      return
    }
    if (m.kind === 'wt_error') {
      busy(false)
      st.purpose = null
      if (d.code === 'config_changed') return renderConfig(d)
      const known = t('wtE_' + d.code)
      toast(known !== 'wtE_' + d.code ? known : d.message || d.code)
      // A stale or blocked merge is shown again as it stands now.
      if (d.code === 'stale' || d.code === 'blocked' || d.code === 'conflict') ask('wt_status', 'check')
    }
  }

  return { show, onMessage }
}
