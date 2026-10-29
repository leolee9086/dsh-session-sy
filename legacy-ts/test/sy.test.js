import test from 'node:test'
import assert from 'node:assert/strict'
import { readDshSessionFile } from '../lib/adapters/dsh.js'
import { spin } from '../lib/spin.js'
import { validateDocument } from '../lib/sy/validate.js'

const SAMPLE = 'C:/Users/al765/.dsh/sessions/--C-Users-al765-AppData-Roaming-SketchUp-SketchUp~00202024-SketchUp-Plugins--/548b0abf-9603-43b8-9f04-c569ced6d61f/session.v3.jsonl.zstd'

test('生成的 .sy 通过结构自检（单文档）', async () => {
  const doc = await readDshSessionFile(SAMPLE)
  const parts = spin(doc, { maxPartBytes: 0 })
  for (const part of parts) assert.deepEqual(validateDocument(part), [], part.Properties.title)
})

test('分片后的每一片都通过结构自检', async () => {
  const doc = await readDshSessionFile(SAMPLE)
  const parts = spin(doc, { maxPartBytes: 120_000 })
  assert.ok(parts.length > 1)
  for (const part of parts) assert.deepEqual(validateDocument(part), [], part.Properties.title)
})