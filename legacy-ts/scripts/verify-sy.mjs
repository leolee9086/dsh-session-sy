#!/usr/bin/env node
/**
 * 用思源自己的解析器（sycheck -> dataparser.ParseJSON）验证生成的 .sy。
 *
 * 这是格式正确性的唯一权威判据：别的实现只能证明彼此一致，不能证明思源认得。
 *
 * 用法: node scripts/verify-sy.mjs --sample 30 | --all
 */

import { readdir, mkdir, rm, writeFile } from 'node:fs/promises'
import { join, basename } from 'node:path'
import { randomInt } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { readDshSessionFile } from '../lib/adapters/dsh.js'
import { spin } from '../lib/spin.js'

const SESSIONS = 'C:/Users/al765/.dsh/sessions'
const SYCHECK = 'tools/sycheck/sycheck.exe'
const OUT = '.tmp-sy'

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
const sampleSize = sampleIndex >= 0 ? Number(args[sampleIndex + 1]) : 30

const files = []
await collect(SESSIONS, 0, files)
const targets = all ? files : takeRandom(files, sampleSize)

await rm(OUT, { recursive: true, force: true })
await mkdir(OUT, { recursive: true })

let spun = 0
let parsed = 0
const rejected = []
const failures = []
const CHUNK = 40

// 边写边验边删：全量生成的 .sy 加起来有几百 MB，不能一次都堆在磁盘上。
for (let offset = 0; offset < targets.length; offset += CHUNK) {
  const slice = targets.slice(offset, offset + CHUNK)
  const batch = []
  for (const [index, file] of slice.entries()) {
    try {
      const doc = await readDshSessionFile(file)
      const parts = spin(doc)
      for (const [partIndex, part] of parts.entries()) {
        const out = join(OUT, String(offset + index).padStart(5, '0') + '-' + partIndex + '.sy')
        await writeFile(out, JSON.stringify(part), 'utf8')
        batch.push(out)
      }
      spun++
    } catch (error) {
      failures.push({ file, stage: 'spin', detail: String(error && error.message ? error.message : error) })
    }
  }

  if (batch.length > 0) {
    try {
      const output = execFileSync(SYCHECK, batch, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
      for (const line of output.split('\n')) {
        if (line.trim().length === 0) continue
        const record = JSON.parse(line)
        if (record.blocks > 0) parsed++
        else rejected.push({ file: record.file, detail: '解析出 0 个块' })
      }
    } catch (error) {
      const stderr = String(error.stderr ?? '')
      const stdout = String(error.stdout ?? '')
      rejected.push({ detail: (stderr || stdout || String(error)).slice(0, 400) })
    }
  }

  for (const out of batch) await rm(out, { force: true })
  if ((offset + CHUNK) % 400 === 0) console.error('progress ' + Math.min(offset + CHUNK, targets.length) + '/' + targets.length)
}

await rm(OUT, { recursive: true, force: true })

console.log(JSON.stringify({
  targeted: targets.length,
  spun,
  acceptedBySiYuan: parsed,
  rejected: rejected.length,
  spinFailures: failures.length,
  rejectedSample: rejected.slice(0, 5),
  failureSample: failures.slice(0, 5)
}, null, 2))

if (rejected.length > 0 || failures.length > 0) process.exitCode = 1
