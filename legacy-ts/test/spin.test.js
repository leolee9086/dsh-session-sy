import test from 'node:test'
import assert from 'node:assert/strict'
import { readDshSessionFile } from '../lib/adapters/dsh.js'
import { spin, spinBack } from '../lib/spin.js'
import { firstDifference } from '../lib/verify.js'

const SAMPLE = 'C:/Users/al765/.dsh/sessions/--C-Users-al765-AppData-Roaming-SketchUp-SketchUp~00202024-SketchUp-Plugins--/548b0abf-9603-43b8-9f04-c569ced6d61f/session.v3.jsonl.zstd'

test('单文件 spin 往返无损（不分片）', async () => {
  const original = await readDshSessionFile(SAMPLE)
  const parts = spin(original, { maxPartBytes: 0 })
  assert.equal(parts.length, 1)
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('强制分片后拼接仍与原序列相等', async () => {
  const original = await readDshSessionFile(SAMPLE)
  const parts = spin(original, { maxPartBytes: 120_000 })
  assert.ok(parts.length > 1, '期望被切成多片，实际 ' + parts.length)
  const difference = firstDifference(original, spinBack(parts))
  assert.equal(difference, null, difference ?? '')
})

test('分片乱序输入也能拼回', async () => {
  const original = await readDshSessionFile(SAMPLE)
  const parts = spin(original, { maxPartBytes: 120_000 })
  const shuffled = [...parts].reverse()
  const difference = firstDifference(original, spinBack(shuffled))
  assert.equal(difference, null, difference ?? '')
})

test('spin 是确定性的', async () => {
  const original = await readDshSessionFile(SAMPLE)
  assert.deepEqual(spin(original, { maxPartBytes: 0 }), spin(original, { maxPartBytes: 0 }))
})