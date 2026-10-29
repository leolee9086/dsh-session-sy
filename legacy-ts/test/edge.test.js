import test from 'node:test'
import assert from 'node:assert/strict'
import { spin, spinBack } from '../lib/spin.js'
import { firstDifference } from '../lib/verify.js'
import { validateDocument } from '../lib/sy/validate.js'

const header = { type: 'session', version: 3, id: 'edge-session', createdAt: 1700000000000, cwd: '/tmp' }

function makeDoc(events) {
  const { type, version, id, ...rest } = header
  return { agent: 'dsh', formatVersion: version, sessionId: id, headerExtra: rest, events }
}

const event = (type, seq, data) => ({ raw: { type, seq, time: 1700000000000 + seq, data } })

test('空会话：只有头块', () => {
  const original = makeDoc([])
  const parts = spin(original)
  assert.equal(parts.length, 1)
  assert.equal(parts[0].Children.length, 1)
  assert.deepEqual(validateDocument(parts[0]), [])
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('单个非 surface 事件', () => {
  const original = makeDoc([event('turn/start', 0, { turn: 1 })])
  const parts = spin(original)
  assert.deepEqual(validateDocument(parts[0]), [])
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('单个 surface 事件（content 挂在 data 上）', () => {
  const original = makeDoc([
    event('user/message', 0, { role: 'user', id: 'm1', content: [{ type: 'text', text: '你好' }], source: { kind: 'user' } }),
  ])
  const parts = spin(original)
  assert.deepEqual(validateDocument(parts[0]), [])
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('单个 surface 事件（content 挂在 data.message 上）', () => {
  const original = makeDoc([
    event('assistant/message', 1, {
      turn: 1, step: 1,
      message: { role: 'assistant', content: [{ type: 'text', text: '回应' }, { type: 'reasoning', text: '思考' }], source: { kind: 'assistant' } },
      usage: { inputTokens: 10, outputTokens: 20 },
      stream: [], interrupted: false,
    }),
  ])
  const parts = spin(original)
  assert.deepEqual(validateDocument(parts[0]), [])
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('工具调用与结果配对', () => {
  const original = makeDoc([
    event('assistant/message', 0, {
      turn: 1, step: 1,
      message: { role: 'assistant', content: [
        { type: 'tool-call', id: 'call-1', name: 'read', arguments: '{"file_path":"a.txt"}' },
      ] },
    }),
    event('tool/result', 1, {
      turn: 1, step: 1,
      message: { source: { kind: 'tool', callId: 'call-1' }, content: [
        { type: 'tool-result', toolCallId: 'call-1', content: [{ type: 'text', text: '内容' }], isError: false },
      ] },
    }),
  ])
  const parts = spin(original)
  assert.deepEqual(validateDocument(parts[0]), [])
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('未知事件类型走批量代码块', () => {
  const original = makeDoc([event('plugin:custom/thing', 0, { anything: [1, 2, { nested: true }] })])
  const parts = spin(original)
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('未知 content 块类型整块保留', () => {
  const original = makeDoc([
    event('user/message', 0, { content: [{ type: 'unknown-block', payload: { a: 1 } }] }),
  ])
  const parts = spin(original)
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('surface 事件缺少 content 时降级为批量代码块', () => {
  const original = makeDoc([event('user/message', 0, { role: 'user', id: 'm1' })])
  const parts = spin(original)
  assert.equal(parts[0].Children.length, 2, '应当是头块 + 事件批')
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('极端分片阈值仍无损', () => {
  const original = makeDoc([
    event('user/message', 0, { content: [{ type: 'text', text: 'a'.repeat(500) }] }),
    event('assistant/message', 1, { message: { role: 'assistant', content: [{ type: 'text', text: 'b'.repeat(500) }] } }),
    event('turn/end', 2, { turn: 1 }),
  ])
  const parts = spin(original, { maxPartBytes: 1 })
  assert.ok(parts.length > 1, '阈值 1 应当每块一片，实际 ' + parts.length)
  for (const part of parts) assert.deepEqual(validateDocument(part), [])
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})