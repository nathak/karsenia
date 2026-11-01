import assert from 'node:assert/strict'
import { test } from 'node:test'

import { baseName, checkImage, fmtTurn, imagePaths, pathsInText, resultBlocks, toolSummary } from '../ui/util.js'

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
