#!/usr/bin/env node
/**
 * 往返验证：读取 DSH 会话 -> spin -> spinBack -> 深比较。
 *
 * 默认按 4MB 分片：最大的一份会话解压后 539MB，不分片会直接把 Node 撑爆。
 * 大文件需要更大的堆，用 node --max-old-space-size=4096 scripts/verify.mjs --all。
 *
 * 用法:
 *   node scripts/verify.mjs --sample 40
 *   node scripts/verify.mjs --all
 */

import { readdir } from 'node:fs/promises'
import { join } from 'node:path'
import { randomInt } from 'node:crypto'
import { readDshSessionFile } from '../lib/adapters/dsh.js'
import { spin, spinBack } from '../lib/spin.js'
import { firstDifference } from '../lib/verify.js'

const SESSIONS = 'C:/Users/al765/.dsh/sessions'

async function collect(dir, depth, out) {
  if (depth > 4) return
  let entries
  try { entries = await readdir(dir, { withFileTypes: true }) } catch { return }
  for (const entry of entries) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) await collect(full, depth + 1, out)
    else if (entry.name.endsWith('.zstd')) out.push(full)
  }
}

function takeRandom(pool, count) {
  const copy = [...pool]
  const picked = []
  const limit = Math.min(count, copy.length)
  for (let i = 0; i < limit; i++) picked.push(copy.splice(randomInt(copy.length), 1)[0])
  return picked
}

const args = process.argv.slice(2)
const all = args.includes('--all')
const sampleIndex = args.indexOf('--sample')
const sampleSize = sampleIndex >= 0 ? Number(args[sampleIndex + 1]) : 40
const seed = randomInt(1_000_000_000)

const files = []
await collect(SESSIONS, 0, files)
const targets = all ? files : takeRandom(files, sampleSize)

console.log(JSON.stringify({ mode: all ? 'all' : 'sample', seed, totalFiles: files.length, targets: targets.length }))

let passed = 0
let parts = 0
let eventCount = 0
const failures = []
const versionCount = new Map()

for (const [index, file] of targets.entries()) {
  try {
    const original = await readDshSessionFile(file)
    const documents = spin(original)
    const back = spinBack(documents)
    const difference = firstDifference(original, back)
    if (difference !== null) {
      failures.push({ file, kind: 'mismatch', detail: difference })
      continue
    }
    passed++
    parts += documents.length
    eventCount += original.events.length
    versionCount.set(original.formatVersion, (versionCount.get(original.formatVersion) ?? 0) + 1)
  } catch (error) {
    failures.push({ file, kind: 'error', detail: String(error && error.message ? error.message : error) })
  }
  if ((index + 1) % 200 === 0) console.error('progress ' + (index + 1) + '/' + targets.length)
}

console.log(JSON.stringify({
  passed,
  failed: failures.length,
  totalEvents: eventCount,
  totalParts: parts,
  versions: Object.fromEntries(versionCount),
  failures: failures.slice(0, 20),
}, null, 2))

if (failures.length > 0) process.exitCode = 1
