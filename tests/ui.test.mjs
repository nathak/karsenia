import assert from 'node:assert/strict'
import { test } from 'node:test'

import { readFileSync } from 'node:fs'

import { systemNote, guardKind, initChips, thinkSummary, codeLang, foldInfo, foldText, editDiff, todoItems, toolHead, toolBadge, isAgentCall, accountName, accountOf, forAccount, inContext, viewOf, attn, PERM_MODES, permChoices, permNeedsConfirm, permOf, grantButtons, MODEL_ALIASES, modelChoices, needsAccount, withAccount, wtBlockerText, wtRemovePrompts, wtText, termFeed, termState, termText, asTextarea, diffWithin, gutterText, lineCount, relJoin, relParent, isUnder, toCrlf, baseName, checkImage, fmtTurn, imagePaths, parseSites, pathsInText, resultBlocks, toolSummary } from '../ui/util.js'

test('a tool call is summed up by what it works on', () => {
  assert.equal(toolSummary('Bash', { command: 'ls -la\nmore' }), 'ls -la')
  assert.equal(toolSummary('Read', { file_path: '/a/b.ts' }), '/a/b.ts')
  assert.equal(toolSummary('X', { n: 1, other: 'text' }), 'text')
  assert.equal(toolSummary('X', null), '')
  assert.equal(toolSummary('Bash', { command: 'x'.repeat(500) }).length, 160)
})

test('image files are found in inputs and in result text', () => {
  assert.deepEqual(imagePaths({ file_path: '/p/a.PNG', other: 'x.txt', again: '/p/a.PNG' }), ['/p/a.PNG'])
  assert.deepEqual(imagePaths({ text: 'see a.png and more' }), [])
  assert.deepEqual(imagePaths(null), [])
  assert.deepEqual(pathsInText('saved to /tmp/out/shot.png and (/x/y.jpeg).'), ['/tmp/out/shot.png', '/x/y.jpeg'])
  assert.deepEqual(pathsInText('nothing here'), [])
  assert.equal(pathsInText('/a.png /b.png /c.png /d.png').length, 3)
})

test('a tool result becomes text and image blocks, and only safe image types pass', () => {
  assert.deepEqual(resultBlocks('hello'), [{ type: 'text', text: 'hello' }])
  assert.deepEqual(resultBlocks(''), [])
  const blocks = resultBlocks([
    { type: 'text', text: 'a' },
    { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAAA' } },
    { type: 'image', source: { type: 'base64', media_type: 'image/svg+xml', data: 'PHN2Zy8+' } },
    { type: 'image', source: { type: 'url', url: 'http://x' } },
    null,
  ])
  assert.deepEqual(blocks.map(b => b.type), ['text', 'image'])
  assert.deepEqual(resultBlocks(undefined), [])
})

test('attachments are limited by type, size and count', () => {
  assert.equal(checkImage({ type: 'image/png', size: 10 }, 0), '')
  assert.equal(checkImage({ type: 'image/svg+xml', size: 10 }, 0), 'type')
  assert.equal(checkImage({ type: 'image/png', size: 6 * 1024 * 1024 }, 0), 'size')
  assert.equal(checkImage({ type: 'image/png', size: 10 }, 5), 'count')
})

test('small formatters', () => {
  assert.equal(baseName('/a/b/c'), 'c')
  assert.equal(baseName('/'), '/')
  assert.equal(fmtTurn({ duration_ms: 1500, total_cost_usd: 0.0123 }), '1.5s · $0.0123')
  assert.equal(fmtTurn({}), '')
})

import { markParts, nearestSeq } from '../ui/util.js'

test('an excerpt is split into plain and marked pieces, and markup in it stays text', () => {
  assert.deepEqual(markParts('a \u0002hit\u0003 b'), [{ text: 'a ', mark: false }, { text: 'hit', mark: true }, { text: ' b', mark: false }])
  assert.deepEqual(markParts('<img src=x onerror=alert(1)>'), [{ text: '<img src=x onerror=alert(1)>', mark: false }])
  assert.deepEqual(markParts(''), [])
  assert.deepEqual(markParts('\u0002x\u0003'), [{ text: 'x', mark: true }])
})

test('a jump goes to the nearest drawn message, at or before the wanted one first', () => {
  assert.equal(nearestSeq([2, 5, 9], 5), 5)
  assert.equal(nearestSeq([2, 5, 9], 7), 5)
  assert.equal(nearestSeq([5, 9], 3), 5)
  assert.equal(nearestSeq([], 3), -1)
})

import { fmtBytes } from '../ui/util.js'

test('sizes are shown in plain units', () => {
  assert.equal(fmtBytes(512), '512 B')
  assert.equal(fmtBytes(1536), '1.5 KB')
  assert.equal(fmtBytes(-1), '')
  assert.equal(fmtBytes(NaN), '')
})

import { lineDiff, numberedLines, skillMd } from '../ui/util.js'

test('the file a person reads is the file the host writes', () => {
  assert.equal(skillMd('release-check', 'Use when "x"', 'Body\n\n'), '---\nname: release-check\ndescription: "Use when \\"x\\""\n---\n\nBody\n')
})

test('lines are numbered and carry their worst risk level', () => {
  const lines = numberedLines('a\nb\nc', [{ line: 2, level: 'warn' }, { line: 2, level: 'block' }, { line: 3, level: 'warn' }, { line: 0, level: 'block' }])
  assert.deepEqual(lines.map(l => [l.n, l.level]), [[1, ''], [2, 'block'], [3, 'warn']])
  assert.equal(numberedLines('x', [{ line: 1, level: 'warn' }], 5)[0].level, '', 'an offset moves the lines the risks point at')
  assert.equal(numberedLines('a\nb\nc\nd\ne\nf', [{ line: 1, level: 'warn' }], 5)[5].level, 'warn')
})

test('a diff keeps what stays and marks what goes and what comes', () => {
  assert.deepEqual(lineDiff('1\n2\n3', '1\nX\n3'), [{ t: ' ', text: '1' }, { t: '-', text: '2' }, { t: '+', text: 'X' }, { t: ' ', text: '3' }])
  assert.deepEqual(lineDiff('a', 'a'), [{ t: ' ', text: 'a' }])
  assert.deepEqual(lineDiff('', 'new').map(d => d.t), ['-', '+'])
  assert.equal(lineDiff('a\nb', 'a\nb\nc').filter(d => d.t === '+').length, 1)
})

import { daysToMask, fmtMinute, maskToDays, parseHHMM } from '../ui/util.js'

test('a time of day and the weekdays go to the form and back unchanged', () => {
  assert.equal(fmtMinute(545), '09:05')
  assert.equal(fmtMinute(1440), '')
  assert.equal(parseHHMM('9:05'), 545)
  assert.equal(parseHHMM('23:59'), 1439)
  assert.equal(parseHHMM('24:00'), -1)
  assert.equal(parseHHMM(''), -1)
  assert.equal(parseHHMM('09:5'), -1)
  assert.equal(daysToMask(maskToDays(0b1010101)), 0b1010101)
  assert.deepEqual(maskToDays(0b0000011), [true, true, false, false, false, false, false])
  assert.equal(daysToMask([true, true, true, true, true, false, false]), 31)
})

import { chatHost } from '../ui/util.js'

test('a chat on another provider names the host its content goes to, and a chat on the account names none', () => {
  assert.equal(chatHost({ sid: 'a', provider: { id: 'p', name: 'GW', host: 'gw.example.com:8443' } }), 'gw.example.com:8443')
  assert.equal(chatHost({ sid: 'a' }), '')
  assert.equal(chatHost({ sid: 'a', provider: { host: 5 } }), '')
  assert.equal(chatHost(undefined), '')
})

import { isLocalHost } from '../ui/util.js'

test('this computer is told from other hosts', () => {
  for (const h of ['localhost', 'localhost:11434', '127.0.0.1:8080', '[::1]:9000']) assert.equal(isLocalHost(h), true, h)
  for (const h of ['gw.example.com', 'localhost.evil.com', '127.0.0.2', '', undefined]) assert.equal(isLocalHost(h), false, String(h))
})

test('the sites of a web task are split, lower-cased, reduced to hosts and not repeated', () => {
  assert.deepEqual(parseSites('Example.com\nnews.example.org, example.com;  a.example.net'), ['example.com', 'news.example.org', 'a.example.net'])
  assert.deepEqual(parseSites('https://Example.com/path?q=1 http://news.example.org:8080/x'), ['example.com', 'news.example.org'])
  assert.deepEqual(parseSites('example.com/a/b'), ['example.com'])
  assert.deepEqual(parseSites('https://user:pw@Example.com/a'), ['example.com'], 'a user part is dropped with the rest of the address')
  assert.deepEqual(parseSites(''), [])
  assert.deepEqual(parseSites(undefined), [])
  // What is not a host name is passed on as typed, for the host to refuse.
  assert.deepEqual(parseSites('127.0.0.1 *.example.com localhost'), ['127.0.0.1', '*.example.com', 'localhost'])
})

test('the strings of the site list exist in both languages and the host reason the page translates is the one the host sends', () => {
  const app = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8')
  for (const key of ['scSites', 'scSitesNote', 'scSitesLabel', 'scNeedsSites']) assert.equal(app.split(`${key}:`).length - 1, 2, `${key} is in both languages`)
  const rust = readFileSync(new URL('../src/sched_store.rs', import.meta.url), 'utf8')
  const reason = /const NEEDS_SITES = '([^']+)'/.exec(app)[1]
  assert.ok(rust.includes(`pub const NEEDS_DOMAINS: &str = "${reason}";`), 'the page recognises the reason the host stores')
})

test('project-relative paths are joined and cut by slashes', () => {
  assert.equal(relJoin('', 'a.txt'), 'a.txt')
  assert.equal(relJoin('src', 'a.txt'), 'src/a.txt')
  assert.equal(relParent('src/ui/a.txt'), 'src/ui')
  assert.equal(relParent('a.txt'), '')
  assert.equal(relParent(''), '')
  assert.ok(isUnder('src/a.txt', 'src') && isUnder('src', 'src') && isUnder('a', ''))
  assert.ok(!isUnder('src2/a', 'src') && !isUnder('sr', 'src'))
})

test('the editor gutter has one number per line and CRLF survives a round trip', () => {
  assert.equal(gutterText(3), '1\n2\n3')
  assert.equal(gutterText(0), '1')
  assert.equal(lineCount(''), 1)
  assert.equal(lineCount('a\nb\n'), 3)
  assert.equal(toCrlf('a\nb\n'), 'a\r\nb\r\n')
  assert.equal(toCrlf('a\r\nb'), 'a\r\nb', 'no doubled CR')
  assert.equal(asTextarea('a\r\nb\r\n'), 'a\nb\n')
  assert.equal(toCrlf(asTextarea('a\r\nb\r\n')), 'a\r\nb\r\n')
})

test('a diff of a big text is not attempted', () => {
  assert.deepEqual(diffWithin('a\nb', 'a\nc').map(d => d.t), [' ', '-', '+'])
  assert.equal(diffWithin('x\n'.repeat(2001), 'y'), null)
  assert.equal(diffWithin('y', 'x\n'.repeat(2001)), null)
  assert.ok(diffWithin('x\n'.repeat(1999), 'y'))
})

const screen = (...chunks) => { const st = termState(); for (const c of chunks) termFeed(st, c); return termText(st) }

test('terminal output keeps its text and loses every escape sequence', () => {
  assert.equal(screen('hello\r\nworld\r\n'), 'hello\nworld\n')
  assert.equal(screen('\x1b[31mred\x1b[0m \x1b[1;32mgreen\x1b[m'), 'red green')
  assert.equal(screen('a\x1b]0;a title\x07b'), 'ab')
  assert.equal(screen('a\x1b]52;c;SGVsbG8=\x07b'), 'ab', 'a clipboard write')
  assert.equal(screen('a\x1b]8;;http://evil.example\x1b\\link\x1b]8;;\x1b\\b'), 'alinkb', 'a link keeps its text only')
  assert.equal(screen('a\x1bPq data\x1b\\b\x1b_x\x1b\\c\x1b^y\x1b\\d'), 'abcd')
  assert.equal(screen('a\x1b(Bb\x1b7c\x1bMd'), 'abcd')
  assert.equal(screen('x\x07\x00\x0b\x7f\u0085y'), 'xy', 'other control characters')
  assert.equal(screen('<b>z</b><img src=x onerror=alert(1)>'), '<b>z</b><img src=x onerror=alert(1)>', 'markup stays text')
})

test('a sequence cut between two chunks is finished in the next one', () => {
  assert.equal(screen('a\x1b', '[3', '1mred\x1b', '[0m!'), 'ared!')
  assert.equal(screen('a\x1b]0;ti', 'tle\x1b', '\\b'), 'ab')
  assert.equal(screen('a\x1b]0;title\x07b'), screen('a\x1b', ']0;title', '\x07b'))
})

test('a carriage return overwrites and a backspace steps back, as a progress line needs', () => {
  assert.equal(screen('10%\r50%\r100%'), '100%')
  assert.equal(screen('abc\r12'), '12c')
  assert.equal(screen('abc\b\bX'), 'aXc')
  assert.equal(screen('ab\b \bc'), 'ac')
})

test('the screen keeps the last lines only, and an endless line wraps', () => {
  const st = termState(50)
  for (let i = 0; i < 1000; i++) termFeed(st, `line ${i}\r\n`)
  const lines = termText(st).split('\n')
  assert.equal(lines.length, 50)
  assert.equal(lines[48], 'line 999')
  assert.ok(st.lines.length < 50 + 201, 'old lines are dropped in batches')
  const wide = termState()
  termFeed(wide, 'q'.repeat(50000))
  assert.ok(wide.lines.length >= 3 && wide.lines.every(l => l.length <= 20000))
  assert.equal(wide.lines.join('').length, 50000)
})

test('a sequence that never ends does not swallow the rest of the output', () => {
  assert.ok(screen('\x1b]0;' + 'x'.repeat(5000) + 'visible').endsWith('visible'))
  const text = screen('\x1b]0;' + 'x'.repeat(5000) + '\r\nafter')
  assert.ok(text.endsWith('after'))
})

test('the terminal page code puts output in textContent only and never in the debug drawer', () => {
  const src = readFileSync(new URL('../ui/term.js', import.meta.url), 'utf8')
  assert.ok(!/innerHTML|outerHTML|insertAdjacentHTML|document\.write|debugLog/.test(src))
  assert.ok(/textContent/.test(src))
  const app = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8')
  assert.ok(/instanceof ArrayBuffer\) \{ term\.onBinary/.test(app), 'a binary frame goes to the terminal before anything is parsed or logged')
  assert.ok(!/localStorage[^\n]*term/.test(src), 'terminal output is not kept in browser storage')
})

test('a control character inside a sequence still acts, and the one-character sequence starters are sequences too', () => {
  assert.equal(screen('a\x1b\nb'), 'a\n', 'a newline right after ESC is not swallowed (the b ends the short sequence)')
  assert.equal(screen('a\x1b[3\n1mb'), 'a\nb')
  assert.equal(screen('a\u009b31mred\u009b0m'), 'ared')
  assert.equal(screen('a\u009d0;title\x07b'), 'ab')
  assert.equal(screen('a\x1b]0;title\u009cb'), 'ab', 'the one-character ST ends a string')
  assert.equal(screen('a\x1b]0;title\x18b'), 'ab', 'CAN ends a string')
})

// The words of the worktree strip: a table stand-in for the page's text table, so a missing key shows as itself.
const WT = {
  wtB_wrong_branch: 'on {main} not {base}', wtB_conflict: 'conflicts: {files}', wtB_nothing: 'nothing',
  wtRemove1: 'delete {path}?', wtRemoveLoss: ' lose {n}', wtRemove2: '{n} not in {base}; delete {branch}?', wtRemove2b: '{n} look merged into {base}; delete {branch}?',
}
const wtT = key => WT[key] ?? key

test('a worktree check is told in words: branch names, and at most three conflicted files with a count of the rest', () => {
  assert.equal(wtBlockerText('wrong_branch', wtT, { main_branch: 'dev', base_branch: 'main' }), 'on dev not main')
  assert.equal(wtBlockerText('wrong_branch', wtT, {}), 'on ? not ', 'nothing known is still a sentence')
  assert.equal(wtBlockerText('conflict', wtT, { conflicts: ['a', 'b'] }), 'conflicts: a, b')
  assert.equal(wtBlockerText('conflict', wtT, { conflicts: ['a', 'b', 'c', 'd', 'e'], conflicts_more: 10 }), 'conflicts: a, b, c +12')
  assert.equal(wtBlockerText('mystery', wtT), 'wtB_mystery', 'an unknown code shows its key, not nothing')
  assert.equal(wtText('{a} {b}', { a: 1 }), '1 {b}', 'a place without a value stays')
})

test('removing a worktree asks once, again about changes that would be lost, and about the branch only when commits would be left on no other branch', () => {
  const base = { path: '/w/x', base_branch: 'main', branch: 'tenrec/x', uncommitted: 0, ahead: 0, in_base: false }
  assert.deepEqual(wtRemovePrompts(base, wtT), { force: false, first: 'delete /w/x?', second: null })
  assert.deepEqual(wtRemovePrompts({ ...base, uncommitted: 3 }, wtT), { force: true, first: 'delete /w/x? lose 3', second: null })
  assert.equal(wtRemovePrompts({ ...base, ahead: 2 }, wtT).second, '2 not in main; delete tenrec/x?')
  assert.equal(wtRemovePrompts({ ...base, ahead: 2, in_base: true }, wtT).second, null, 'already in the base: nothing to lose, nothing to ask')
  assert.equal(wtRemovePrompts({ ...base, ahead: 2, same_content: true }, wtT).second, '2 look merged into main; delete tenrec/x?', 'content that only looks merged is asked about, in its own words')
})

test('the worktree strip puts what it shows in textContent only, and the removal question is asked before anything is sent', () => {
  const src = readFileSync(new URL('../ui/wt.js', import.meta.url), 'utf8')
  assert.ok(!/innerHTML|outerHTML|insertAdjacentHTML|document\.write/.test(src))
  assert.ok(/confirm\(q\.first\)[\s\S]*send\(\{ op: 'wt_remove'/.test(src), 'the first question comes before the request')
  assert.ok(/wt_merge', 'merge', \{ tip: s\.tip, main_head: s\.main_head \}/.test(src), 'a merge carries the commits that were shown')
  assert.ok(/op: 'wt_remove'[^\n]*tip: status\.tip/.test(src), 'a removal carries the branch tip that was shown')
  assert.ok(/m\.kind === 'wt_removed'[\s\S]*?if \(m\.sid !== sid\(\)\) return/.test(src), 'a removal is acted on whichever chat is open')
  assert.ok(/wt_trust', 'trust', \{ hash: d\.hash \}/.test(src), 'accepting settings sends the hash that was shown')
})

test('a message about notes, skills or history names the account the panel shows, and the settings and other messages name none', () => {
  for (const name of ['memory_list', 'memory_add', 'memory_edit', 'memory_delete', 'memory_reject', 'memory_undo', 'memory_approve', 'memory_pin', 'memory_export', 'skill_list', 'skill_propose', 'skill_approve', 'skill_reject', 'skill_enable', 'skill_delete', 'skill_undo', 'skill_versions', 'skill_rollback', 'search']) {
    assert.ok(needsAccount(name), `${name} acts on one account`)
  }
  for (const name of ['memory_setting', 'skill_setting', 'new', 'attach', 'sched_list', 'provider_save', 'account_list', 'wt_status', 'fs_read']) assert.ok(!needsAccount(name), `${name} names no account`)
  assert.deepEqual(withAccount({ op: 'memory_list', cwd: '/p' }, 'ab12'), { op: 'memory_list', cwd: '/p', account: 'ab12' })
  assert.deepEqual(withAccount({ op: 'search', q: 'x' }, null), { op: 'search', q: 'x', account: null }, 'the default account is a null, not a missing key')
  assert.deepEqual(withAccount({ op: 'search', q: 'x' }, undefined), { op: 'search', q: 'x', account: null })
  assert.deepEqual(withAccount({ op: 'skill_propose', account: 'chat-account' }, 'other'), { op: 'skill_propose', account: 'chat-account' }, 'an account the message already names is kept')
  const setting = { op: 'memory_setting', auto: 'off' }
  assert.equal(withAccount(setting, 'ab12'), setting)
})

test('a change is for a panel when it names the account the panel shows, or every account', () => {
  assert.ok(forAccount({ account: null }, null) && forAccount({}, null) && forAccount({ account: 'a' }, 'a'))
  assert.ok(!forAccount({ account: 'a' }, null) && !forAccount({ account: null }, 'a') && !forAccount({ account: 'a' }, 'b'))
  assert.ok(forAccount({ all: true }, 'a') && forAccount({ all: true, account: null }, 'b'), 'a setting changed for every account')
  assert.ok(!forAccount(null, null) && !forAccount(undefined, 'a'))
  assert.equal(accountOf({ account: { id: 'ab', name: 'Work' } }), 'ab')
  assert.equal(accountOf({}), null)
  assert.equal(accountOf(undefined), null)
  assert.equal(accountOf({ account: { id: 7 } }), null)
  assert.equal(accountName('ab', [{ id: 'ab', name: 'Company' }], 'Work'), 'Company', 'the name as it is now wins over the one the chat kept')
  assert.equal(accountName('ab', [], 'Work'), 'Work')
  assert.equal(accountName('abcdef123456', [], undefined), 'abcdef')
  assert.equal(accountName(null, [], 'x'), '')
})

test('the account strings exist in both languages and every message to the host goes through the one function that names the account', () => {
  const app = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8')
  const keys = new Set([...app.matchAll(/\b(acct[A-Za-z_]*): '/g)].map(m => m[1]))
  assert.ok(keys.size > 20)
  for (const key of keys) assert.equal(app.split(`${key}: '`).length - 1, 2, `${key} is in both languages`)
  // The warning codes the host sends are all translated: acctW_<code>.
  const rust = readFileSync(new URL('../src/registry.rs', import.meta.url), 'utf8')
  for (const code of new Set([...rust.matchAll(/warnings\.push\("(\w+)"\)/g)].map(m => m[1]))) assert.ok(keys.has('acctW_' + code), `${code} has a text`)
  // Only wsSend writes to the socket besides the sign-in message, so no message can skip withAccount.
  const sends = [...app.matchAll(/\bws\.send\(/g)].length
  assert.equal(sends, 2, 'wsSend and the sign-in')
  assert.ok(app.includes('S.ws.send(JSON.stringify(withAccount(op, S.acct)))'))
  // What an account's name or login shows is put in as text.
  const block = app.slice(app.indexOf('// ---- accounts: made, renamed'))
  assert.ok(!/innerHTML|insertAdjacentHTML/.test(block), 'account text is never parsed as HTML')
})

test('moving the panels to another account drops what belonged to the one that was shown', () => {
  const app = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8')
  const start = app.indexOf('function setAcct(id)')
  const body = app.slice(start, app.indexOf('// ---- sessions ----', start))
  for (const dropped of ['S.searchReq += 1', 'S.editing = 0', 'S.skEditing = 0', 'S.skDraft = null', 'S.skEditReport = null', 'S.skHistory = null']) {
    assert.ok(body.includes(dropped), `${dropped} when the account changes`)
  }
})

test('a thinking block says how long it is, and how long it took only when that was measured', () => {
  assert.deepEqual(thinkSummary(1234, 5300), { chars: 1234, secs: 5 })
  assert.deepEqual(thinkSummary(10), { chars: 10, secs: null })
  assert.equal(thinkSummary(10, 200).secs, 1, 'never zero seconds')
  assert.deepEqual(thinkSummary('x', -5), { chars: 0, secs: null })
})

test('a fenced block names its language, and a long one is folded', () => {
  assert.equal(codeLang('language-js'), 'js')
  assert.equal(codeLang('foo language-c++ bar'), 'c++')
  assert.equal(codeLang('language-<x>'), '')
  assert.equal(codeLang('language-' + 'a'.repeat(30)), '')
  assert.equal(codeLang(''), '')
  assert.equal(codeLang(undefined), '')
  assert.deepEqual(foldInfo('a\n'.repeat(31)), { lines: 31, folded: true })
  assert.deepEqual(foldInfo('a\nb'), { lines: 2, folded: false })
  const f = foldText(Array.from({ length: 30 }, (_, i) => 'l' + i).join('\n'))
  assert.equal(f.head.split('\n').length, 12)
  assert.equal(f.lines, 30)
  assert.ok(f.folded && !f.cut)
  assert.ok(foldText('x'.repeat(25000)).cut)
  assert.equal(foldText('x'.repeat(25000)).full.length, 20000)
})

test('Edit, MultiEdit and Write calls become changed lines; other calls and odd input become nothing', () => {
  const one = editDiff('Edit', { file_path: '/a', old_string: 'a\nb', new_string: 'a\nc' })
  assert.deepEqual([one.add, one.del], [1, 1])
  assert.deepEqual(one.rows.map(r => r.t), [' ', '-', '+'])
  const write = editDiff('Write', { file_path: '/a', content: 'x\ny\nz' })
  assert.deepEqual([write.add, write.del, write.rows.length], [3, 0, 3])
  const multi = editDiff('MultiEdit', { edits: [{ old_string: 'a', new_string: 'b' }, { old_string: 'c', new_string: 'd' }] })
  assert.deepEqual([multi.add, multi.del], [2, 2])
  assert.ok(multi.rows.some(r => r.t === '…'), 'a divider between edits')
  const big = editDiff('Write', { content: 'x\n'.repeat(5000) })
  assert.equal(big.rows, null)
  assert.ok(big.add > 4000)
  const bigEdit = editDiff('Edit', { old_string: 'a\n'.repeat(500), new_string: 'b\n'.repeat(500) })
  assert.equal(bigEdit.rows, null)
  assert.ok(bigEdit.add >= 500 && bigEdit.del >= 500)
  for (const bad of [editDiff('Bash', { command: 'ls' }), editDiff('Edit', null), editDiff('Edit', { old_string: 1, new_string: 2 }), editDiff('MultiEdit', { edits: [] }), editDiff('Write', {})]) assert.equal(bad, null)
})

test('tools with a clear main argument get a one-line head, and a TodoWrite becomes a checklist', () => {
  assert.deepEqual(toolHead('Bash', { command: 'ls -la\nmore', description: 'list' }), { label: 'cmd', text: 'ls -la', extra: 'list' })
  assert.equal(toolHead('Read', { file_path: '/a', offset: 10, limit: 5 }).text, '/a:10–15')
  assert.equal(toolHead('Read', { file_path: '/a' }).text, '/a')
  assert.equal(toolHead('Grep', { pattern: 'foo', path: 'src' }).extra, 'src')
  assert.equal(toolHead('Glob', { pattern: '**/*.rs' }).text, '**/*.rs')
  assert.equal(toolHead('WebFetch', { url: 'https://x.test/' }).text, 'https://x.test/')
  assert.equal(toolHead('WebSearch', { query: 'q' }).text, 'q')
  assert.equal(toolHead('Edit', { file_path: '/a' }), null)
  assert.equal(toolHead('Bash', null).text, '')
  const todos = { todos: [{ content: 'a', status: 'completed' }, { content: 'b', status: 'in_progress' }, { content: 'c', status: 'weird' }, null, { content: 'd', status: 'pending' }] }
  assert.deepEqual(todoItems(todos).map(i => i.status), ['completed', 'in_progress', 'pending'])
  assert.equal(toolHead('TodoWrite', todos).extra, '1/3')
  assert.equal(todoItems({ todos: Array.from({ length: 80 }, () => ({ content: 'x', status: 'pending' })) }).length, 50)
  assert.deepEqual(todoItems(null), [])
})

test('sub-agent and plug-in calls carry a small badge, and the agent tools are told apart', () => {
  assert.deepEqual(toolBadge('mcp__plugin_vhdb_vhdb__query_history'), { text: 'vhdb', kind: 'mcp' })
  assert.deepEqual(toolBadge('mcp__smartworkbench__set_done_conditions'), { text: 'swb', kind: 'mcp' })
  assert.deepEqual(toolBadge('mcp__tenrec__search'), { text: 'tenrec', kind: 'mcp' })
  assert.deepEqual(toolBadge('mcp__other__x'), { text: 'other', kind: 'mcp' })
  assert.deepEqual(toolBadge('Task', { subagent_type: 'smartworkbench:advisor' }), { text: 'advisor', kind: 'advisor' })
  assert.deepEqual(toolBadge('Agent', { subagent_type: 'smartworkbench:explorer' }), { text: 'explorer', kind: 'explorer' })
  assert.deepEqual(toolBadge('Agent', { subagent_type: 'Explore' }), { text: 'Explore', kind: 'agent' })
  assert.equal(toolBadge('Agent', {}), null)
  assert.equal(toolBadge('Bash', { command: 'ls' }), null)
  assert.ok(isAgentCall('Task') && isAgentCall('Agent') && !isAgentCall('Bash'))
})

test('the chat output strings exist in both languages, and what the model or a tool wrote is never parsed as markup by the new drawing code', () => {
  const app = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8')
  const keys = new Set([...app.matchAll(/\b(cv[A-Za-z_]*): '/g)].map(m => m[1]))
  assert.ok(keys.size >= 13)
  for (const key of keys) assert.equal(app.split(`${key}: '`).length - 1, 2, `${key} is in both languages`)
  for (const used of new Set([...app.matchAll(/\bt\('(cv[A-Za-z_]*)'\)/g)].map(m => m[1]))) assert.ok(keys.has(used), `${used} has a text`)
  const part = (from, to) => app.slice(app.indexOf(from), app.indexOf(to, app.indexOf(from)))
  for (const [from, to] of [['function startLiveThink', 'function addAssistant'], ['function decorateCode', 'function place'], ['function place', 'function addThinking'], ['function addThinking', '// ---- tools and images ----'], ['function addTool', 'function setToolResult'], ['function setToolResult', '// An image of the project']]) {
    assert.ok(!/innerHTML|insertAdjacentHTML/.test(part(from, to)), `${from} puts text in as text`)
  }
  assert.ok(!app.includes('redacted_thinking.data') && !/block\.data/.test(part('case \'assistant\'', 'case \'user\'')), 'the data of a hidden thinking block is never read')
})

test('only the system events that matter get a line, and nothing a hook printed is put in one', () => {
  const ok = { type: 'system', subtype: 'hook_response', hook_name: 'Stop', hook_event: 'Stop', outcome: 'success', exit_code: 0, output: '{"metrics":{"a":1}}', stdout: '', stderr: '' }
  assert.equal(systemNote(ok), null)
  for (const quiet of [{ type: 'system', subtype: 'hook_started', hook_name: 'Stop' }, { type: 'system', subtype: 'ui_log', plugin: 'smartworkbench', text: 'x' }, { type: 'system', subtype: 'status', status: 'requesting' }, { type: 'system', subtype: 'init' }, null, 'x']) assert.equal(systemNote(quiet), null)
  const failed = systemNote({ ...ok, hook_name: 'PreToolUse:Edit', exit_code: 1, outcome: 'error', stderr: 'SECRET prompt text' })
  assert.equal(failed.level, 'warn')
  assert.ok(failed.key.includes('PreToolUse:Edit'))
  assert.ok(!JSON.stringify(failed).includes('SECRET'), 'what a hook printed never reaches a note')
  assert.equal(systemNote({ ...ok, outcome: 'error', exit_code: null }).code, 'hook-outcome')
  assert.equal(systemNote({ ...ok, exit_code: 2, outcome: 'blocking' }).level, 'info', 'a deliberate hold is not shown as a failure')
  const win = (util, status = 'allowed', resetsAt = 4102444800) => ({ type: 'rate_limit_event', rate_limit_info: { status, unifiedWindows: { five_hour: { utilization: util, status, resetsAt }, seven_day: { utilization: 0.1, status: 'allowed', resetsAt } } } })
  assert.equal(systemNote(win(0.36)), null)
  const high = systemNote(win(0.85))
  assert.deepEqual([high.code, high.vars.window, high.vars.pct, high.scope], ['rate', 'five_hour', 85, 'window'])
  assert.equal(systemNote(win(0.85, 'allowed', 1000)), null, 'a window that is over says nothing')
  assert.equal(systemNote(win(0.2, 'rejected')).level, 'warn')
  assert.notEqual(systemNote(win(0.85)).key, systemNote(win(0.97)).key, 'a rise in use is shown again')
  assert.equal(systemNote(win(0.85)).key, systemNote(win(0.86)).key, 'the same level is shown once')
  // UNVERIFIED against a real capture: the VHDB switch for injecting history is off by default.
  assert.equal(systemNote({ ...ok, output: 'x <vhdb_history>\n..</vhdb_history>' }).code, 'vhdb')
})

test('a result held by a plug-in guard is told from a plain error', () => {
  assert.equal(guardKind('SmartWorkbench team mode: this edit is not small (more than 6 lines)'), 'team')
  assert.equal(guardKind([{ type: 'text', text: 'PreToolUse:Edit hook: SmartWorkbench team mode: this edit is not small' }]), 'team')
  assert.equal(guardKind('VHDB guard (한 번만): see the history first'), 'vhdb')
  assert.equal(guardKind('File not found'), null)
  assert.equal(guardKind([{ type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAAA' } }]), null)
  assert.equal(guardKind(undefined), null)
  assert.equal(guardKind('x'.repeat(1000) + 'SmartWorkbench team mode:'), null, 'only the start of a result is read')
})

test('the plug-in chips follow what the chat is wired to, not what is switched on', () => {
  const init = { agents: ['smartworkbench:advisor', 'smartworkbench:explorer', 'Explore'], mcp_servers: [{ name: 'plugin:vhdb:vhdb', status: 'connected' }, { name: 'smartworkbench', status: 'connected' }], plugins: [{ name: 'vhdb' }, { name: 'vhdb' }] }
  assert.deepEqual(initChips(init), { team: true, vhdb: true })
  assert.deepEqual(initChips({ agents: ['Explore'], mcp_servers: [{ name: 'plugin:vhdb:vhdb', status: 'needs-auth' }] }), { team: false, vhdb: false })
  assert.deepEqual(initChips({}), { team: false, vhdb: false })
  assert.deepEqual(initChips(null), { team: false, vhdb: false })
})

test('the Stitch layout helpers: day, clock, age, model name, turn line, numbered code, path tail', async () => {
  const u = await import('../ui/util.js')
  const t0 = new Date(2026, 10, 13, 9, 5).getTime()
  assert.equal(u.dayKey(t0), '2026-11-13')
  assert.equal(u.dayKey(0), '', 'a log written before times were kept has no day')
  assert.equal(u.clock(t0), '09:05')
  assert.equal(u.ago(t0, t0 + 30_000, '방금'), '방금')
  assert.equal(u.ago(t0, t0 + 5 * 60_000), '5m')
  assert.equal(u.ago(t0, t0 + 3 * 3_600_000), '3h')
  assert.equal(u.ago(t0, t0 + 2 * 86_400_000), '2d')
  assert.equal(u.modelLabel('claude-sonnet-4-5-20250929'), 'sonnet-4-5')
  assert.equal(u.modelLabel('haiku'), 'haiku')
  assert.equal(u.modelLabel(undefined), '')
  assert.deepEqual(u.turnLine({ total_cost_usd: 0.0123, usage: { input_tokens: 100, cache_read_input_tokens: 900, output_tokens: 500 } }), { cost: '$0.012', tokens: '1.5k' })
  assert.equal(u.turnLine({ duration_ms: 5 }), null, 'no cost and no tokens: no line')
  assert.deepEqual(u.codeLines('a\n+b\n-c\n', 'diff').map(l => l.kind), ['', 'add', 'del'])
  assert.deepEqual(u.codeLines('+x\n', 'js').map(l => l.kind), [''], 'only a diff marks lines')
  assert.equal(u.codeLines('x\n'.repeat(5), '', 3), null, 'a very long block is not split')
  assert.equal(u.tailPath('/home/me/p'), '/home/me/p')
  assert.equal(u.tailPath('/home/someone/projects/deep/nested/project-name', 24), '…/nested/project-name')
  assert.equal(u.tailPath('C:\\Users\\someone\\projects\\deep\\project', 20), '…\\deep\\project')
})

test('a chat is listed under its own account; provider chats and chats of a deleted account under the default one', () => {
  const none = { sid: 'p' }
  const work = { sid: 'w', account: { id: 'a1', name: 'Work' } }
  const gone = { sid: 'g', account: { id: 'zz', name: 'Old' } }
  const known = ['a1']
  assert.ok(inContext(none, null, known) && !inContext(none, 'a1', known))
  assert.ok(inContext(work, 'a1', known) && !inContext(work, null, known))
  assert.ok(inContext(gone, null, known) && !inContext(gone, 'a1', known), 'a deleted account\'s chat stays reachable')
  assert.ok(inContext(work, 'a1', null) && !inContext(work, null, null) && !inContext(gone, null, null), 'before the list came')
  assert.ok(inContext({ account: { id: 7 } }, null, known), 'an id that is not text counts as none')
  assert.equal(viewOf(work, known), 'a1')
  assert.equal(viewOf(gone, known), null)
  assert.equal(viewOf(gone, null), 'zz')
  assert.equal(viewOf(none, known), null)
})

test('what waits or runs under the accounts that are not shown is counted per account', () => {
  const list = [
    { sid: '1', pending: 2, state: 'running' },
    { sid: '2', account: { id: 'a1' }, state: 'running' },
    { sid: '3', account: { id: 'a1' }, pending: 1 },
    { sid: '4', account: { id: 'a1' }, state: 'idle' },
    { sid: '5', account: { id: 'zz' }, state: 'running' },
  ]
  const known = ['a1']
  const seen = attn(list, null, known)
  assert.deepEqual([...seen.keys()], ['a1'])
  assert.deepEqual(seen.get('a1'), { wait: 1, run: 1 })
  const other = attn(list, 'a1', known)
  assert.deepEqual(other.get(''), { wait: 1, run: 1 }, 'the deleted account\'s chat is under the default one')
  assert.equal(attn(null, null, null).size, 0)
})

test('the permission modes offered are the six Claude Code takes, narrowed for a provider chat, and skipping every check needs a confirmation', () => {
  assert.deepEqual(PERM_MODES, ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'])
  assert.deepEqual(permChoices(), PERM_MODES)
  // A provider chat: no auto, and no skipping of checks unless it runs in a container.
  assert.deepEqual(permChoices({ provider: true }), ['default', 'acceptEdits', 'plan', 'dontAsk'])
  assert.ok(permChoices({ provider: true, container: true }).includes('bypassPermissions'))
  assert.ok(!permChoices({ provider: true, container: true }).includes('auto'))
  assert.ok(permNeedsConfirm('bypassPermissions') && !permNeedsConfirm('plan') && !permNeedsConfirm('default') && !permNeedsConfirm(undefined))
  assert.equal(permOf({ permission_mode: 'acceptEdits' }), 'acceptEdits')
  assert.equal(permOf({ permission_mode: 'default' }), null, 'the default is not a badge')
  assert.equal(permOf({}), null)
  assert.equal(permOf({ permission_mode: 'yolo' }), null, 'a name that is not on the list is nothing')
  assert.equal(permOf(undefined), null)
})

test('the buttons that ask Claude Code to remember a rule come from what the host said and never from the page', () => {
  assert.deepEqual(grantButtons({ grants: { session: 'Bash(touch a)', always: 'Bash(touch a)', exact: true } }), [{ grant: 'session', rule: 'Bash(touch a)', exact: true }, { grant: 'always', rule: 'Bash(touch a)', exact: true }])
  assert.deepEqual(grantButtons({ grants: { session: 'Bash(ls)', always: null, exact: false } }), [{ grant: 'session', rule: 'Bash(ls)', exact: false }], 'a provider chat has no always')
  assert.deepEqual(grantButtons({ grants: { session: null, always: null, exact: false } }), [])
  for (const bad of [undefined, null, {}, { grants: null }, { grants: 'x' }, { grants: { session: '', always: 3 } }, { grants: { session: 'x'.repeat(5000) } }]) assert.deepEqual(grantButtons(bad), [], JSON.stringify(bad))
})

test('the model picker offers the documented aliases and shows a full model id as it is', () => {
  assert.deepEqual(modelChoices(''), MODEL_ALIASES)
  assert.deepEqual(modelChoices('opus'), MODEL_ALIASES)
  assert.deepEqual(modelChoices('claude-sonnet-5-5'), [...MODEL_ALIASES, 'claude-sonnet-5-5'])
  assert.equal(MODEL_ALIASES[0], '', 'the first is the default')
})
