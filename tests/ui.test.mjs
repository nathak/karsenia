import assert from 'node:assert/strict'
import { test } from 'node:test'

import { readFileSync } from 'node:fs'

import { baseName, checkImage, fmtTurn, imagePaths, parseSites, pathsInText, resultBlocks, toolSummary } from '../ui/util.js'

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
