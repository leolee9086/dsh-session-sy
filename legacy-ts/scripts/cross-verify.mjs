#!/usr/bin/env node
/**
 * 交叉验证：TS 生成 .sy，Rust 用自己的代码路径读回事件序列，再与原始日志比。
 *
 * 分批是为了避免几千次进程启动：每批写一批分片 + 一份清单，Rust 一次读完。
 * 格式正确性另有 sycheck（思源自己的 dataparser）裁决，两者职责不同。
 *
 * 用法: node --max-old-space-size=6144 scripts/cross-verify.mjs --sample 20 | --all
 */

import { readdir, mkdir, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { randomInt } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { readDshSessionFile } from '../lib/adapters/dsh.js'
import { spin } from '../lib/spin.js'

const SESSIONS = 'C:/Users/al765/.dsh/sessions'
const RUST = 'rust/verify/target/release/session-sy-verify.exe'
const OUT = '.tmp-cross'
const BATCH = 50

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
const sampleSize = sampleIndex >= 0 ? Number(args[sampleIndex + 1]) : 20

const files = []
await collect(SESSIONS, 0, files)
const targets = all ? files : takeRandom(files, sampleSize)

await rm(OUT, { recursive: true, force: true })
await mkdir(OUT, { recursive: true })

let passed = 0
let events = 0
const failures = []

for (let offset = 0; offset < targets.length; offset += BATCH) {
  const slice = targets.slice(offset, offset + BATCH)
  const manifest = []
  const written = []
  const expected = new Map()

  for (const [index, file] of slice.entries()) {
    try {
      const doc = await readDshSessionFile(file)
      const parts = spin(doc)
      const paths = []
      for (const [partIndex, part] of parts.entries()) {
        const out = join(OUT, String(offset + index).padStart(5, '0') + '-' + partIndex + '.sy')
        await writeFile(out, JSON.stringify(part), 'utf8')
        paths.push(out)
        written.push(out)
      }
      manifest.push([file, ...paths].join('\t'))
      expected.set(file, doc.events.length)
    } catch (error) {
      failures.push({ file, stage: 'spin', detail: String(error && error.message ? error.message : error) })
    }
  }

  if (manifest.length > 0) {
    const manifestPath = join(OUT, 'manifest.txt')
    await writeFile(manifestPath, manifest.join('\n') + '\n', 'utf8')
    try {
      const output = execFileSync(RUST, [manifestPath], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
      for (const line of output.split('\n')) {
        if (line.trim().length === 0) continue
        const record = JSON.parse(line)
        if (record.ok === true) {
          passed++
          events += expected.get(record.file) ?? 0
        } else {
          failures.push({ file: record.file, stage: 'rebuild', detail: record.difference })
        }
      }
    } catch (error) {
      failures.push({ stage: 'rust', detail: (String(error.stderr ?? '') || String(error)).slice(0, 300) })
    }
    await rm(manifestPath, { force: true })
  }

  for (const out of written) await rm(out, { force: true })
  if ((offset + BATCH) % 500 === 0) console.error('progress ' + Math.min(offset + BATCH, targets.length) + '/' + targets.length)
}

await rm(OUT, { recursive: true, force: true })

console.log(JSON.stringify({
  targeted: targets.length,
  passed,
  failed: failures.length,
  totalEvents: events,
  failureSample: failures.slice(0, 8),
}, null, 2))

if (failures.length > 0) process.exitCode = 1
