#!/usr/bin/env node
/**
 * 格式层验证：把会话导出成 `.sy`，交给思源自己的解析器裁决。
 *
 * 判据只有思源 —— `tools/sycheck` 调的就是内核 filesys/tree.go 用的那个函数：
 *   tree, needFix, err := dataparser.ParseJSON(data, engine.ParseOptions)
 * 自己写两套实现互相印证猜不出格式对错，只有这里能。
 *
 * 用法: node scripts/verify-siyuan.mjs <session.jsonl.zstd>
 *       node scripts/verify-siyuan.mjs --sample 40
 *       node scripts/verify-siyuan.mjs --all
 */

import { execFileSync } from 'node:child_process'
import { mkdirSync, readdirSync, rmSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

// 仓库根：本脚本在 scripts/ 下，往上一级
const here = join(import.meta.dirname, '..')
// 引擎二进制在兄弟仓库 rebec 里
const BINARY = join(here, '..', 'rebec', 'target', 'release', 'rebec.exe')
const SYCHECK = join(here, 'tools', 'sycheck', 'sycheck.exe')
const SESSIONS = join(homedir(), '.dsh', 'sessions')

/** 递归收集会话文件。 */
function collect(directory, out = []) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      collect(path, out)
    } else if (entry.name.endsWith('.jsonl.zstd')) {
      out.push(path)
    }
  }
  return out
}

/** 用思源解析器读一片文档。 */
function check(path) {
  const output = execFileSync(SYCHECK, [path], { encoding: 'utf8' })
  const result = JSON.parse(output.trim().split('\n').pop())
  return result
}

/** 一个会话：导出全部分片，逐片交思源裁决。 */
function verifySession(session, workRoot, index) {
  const directory = join(workRoot, String(index))
  mkdirSync(directory, { recursive: true })
  const emitted = execFileSync(BINARY, ['emit', session, directory], { encoding: 'utf8' })
  const summary = JSON.parse(emitted.trim().split('\n').pop())
  let blocks = 0
  for (const name of readdirSync(directory)) {
    if (!name.endsWith('.sy')) continue
    const result = check(join(directory, name))
    if (result.needFix) throw new Error('needFix: ' + name)
    blocks += result.blocks
  }
  rmSync(directory, { recursive: true, force: true })
  return { events: summary.events, parts: summary.parts, blocks }
}

const args = process.argv.slice(2)
let targets
if (args[0] === '--all') {
  targets = collect(SESSIONS)
} else if (args[0] === '--sample') {
  const size = Number(args[1] ?? 40)
  const all = collect(SESSIONS)
  const picked = []
  const taken = new Set()
  while (picked.length < size && taken.size < all.length) {
    const index = Math.floor(Math.random() * all.length)
    if (taken.has(index)) continue
    taken.add(index)
    picked.push(all[index])
  }
  targets = picked
} else if (args[0]) {
  targets = [args[0]]
} else {
  console.error('用法: node scripts/verify-siyuan.mjs <session.jsonl.zstd> | --sample N | --all')
  process.exit(2)
}

targets.sort()
// 工作区外的写入受策略限制，临时分片落在项目内。
const workRoot = join(here, '.tmp-siyuan')
rmSync(workRoot, { recursive: true, force: true })
mkdirSync(workRoot, { recursive: true })
let passed = 0
let failed = 0
let events = 0
let parts = 0
let blocks = 0
const failures = []

try {
  targets.forEach((session, index) => {
    try {
      const result = verifySession(session, workRoot, index)
      passed += 1
      events += result.events
      parts += result.parts
      blocks += result.blocks
    } catch (error) {
      failed += 1
      if (failures.length < 20) failures.push({ file: session, detail: String(error).slice(0, 400) })
    }
    if ((index + 1) % 100 === 0) process.stderr.write('progress ' + (index + 1) + '/' + targets.length + '\n')
  })
} finally {
  rmSync(workRoot, { recursive: true, force: true })
}

process.stdout.write(JSON.stringify({ sessions: targets.length, passed, failed, events, parts, blocks, failureSample: failures }, null, 2) + '\n')
if (failed > 0) process.exitCode = 1
