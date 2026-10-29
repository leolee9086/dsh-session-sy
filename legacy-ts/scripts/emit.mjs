#!/usr/bin/env node
/** 把一个 DSH 会话转成 .sy 文件，供思源侧检查。分片会写成 .0.sy .1.sy … */

import { writeFile } from 'node:fs/promises'
import { readDshSessionFile } from '../lib/adapters/dsh.js'
import { spin, DEFAULT_MAX_PART_BYTES } from '../lib/spin.js'

const [file, out] = process.argv.slice(2)
if (!file || !out) {
  console.error('用法: node scripts/emit.mjs <session.zstd> <out.sy> [maxPartBytes]')
  process.exit(2)
}

const maxPartBytes = process.argv[5] === undefined ? DEFAULT_MAX_PART_BYTES : Number(process.argv[5])
const doc = await readDshSessionFile(file)
const parts = spin(doc, { maxPartBytes })

const written = []
for (const [index, part] of parts.entries()) {
  const path = parts.length === 1 ? out : out + '.' + index + '.sy'
  await writeFile(path, JSON.stringify(part) + '\n', 'utf8')
  written.push(path)
}
console.log(JSON.stringify({ files: written.length, blocks: parts.reduce((n, p) => n + p.Children.length, 0), written }))