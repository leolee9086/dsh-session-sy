#!/usr/bin/env node
/**
 * 从 DSH 发布的 docs/persistence-schema.json 生成 Rust 侧读的事件清单。
 *
 * 权威在 DSH 那边：它自己的生成器遍历所有 SessionEventMap 声明、产出那份 schema。
 * 这里只做翻译，不从仓库代码里搜集、也不抽样统计会话日志 ——
 * 统计出来的只是「见过的」，不是「允许的」。
 *
 * 用法: node scripts/generate-schema.mjs <persistence-schema.json> <out.json>
 */

import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { dirname } from 'node:path'

const [source, target] = process.argv.slice(2)
if (!source || !target) {
  console.error('用法: node scripts/generate-schema.mjs <schema.json> <out.json>')
  process.exit(2)
}

const schema = JSON.parse(await readFile(source, 'utf8'))
if (schema.formatVersion !== 2) throw new Error('只支持 formatVersion 2，实际 ' + String(schema.formatVersion))
if (!Array.isArray(schema.roots)) throw new Error('schema.roots 不是数组')

const roots = schema.roots

/** root.key 形如 'event:user/message'，事件名是冒号之后的部分。 */
function eventName(root) {
  const key = String(root.key)
  const at = key.indexOf(':')
  return at >= 0 ? key.slice(at + 1) : key
}

/** 对象节点的字段名列表。 */
function fieldNames(node) {
  if (!node || node.kind !== 'object' || !Array.isArray(node.properties)) return []
  return node.properties.map(p => p.name)
}

const events = roots.filter(r => r.kind === 'event').map(eventName).sort()
const surfaces = roots.filter(r => r.kind === 'event' && r.surface === true).map(eventName).sort()

/** content 块的类型名：从各事件 data.content / data.message.content 的联合里取。 */
const blockTypes = new Set()
for (const root of roots) {
  if (root.kind !== 'event') continue
  const nodes = root.schema?.nodes
  const index = root.schema?.root
  if (!Array.isArray(nodes) || typeof index !== 'number') continue
  const envelope = nodes[index]
  const dataProp = Array.isArray(envelope?.properties)
    ? envelope.properties.find(p => p.name === 'data')
    : undefined
  if (!dataProp) continue
  const dataNode = nodes[dataProp.type]
  if (!dataNode || !Array.isArray(dataNode.properties)) continue
  for (const property of dataNode.properties) {
    if (property.name !== 'content' && property.name !== 'message') continue
    const target = nodes[property.type]
    const arrayNode = target && target.kind === 'array' ? nodes[target.element] : undefined
    const candidates = arrayNode && arrayNode.kind === 'union'
      ? arrayNode.types.map(i => nodes[i])
      : [arrayNode]
    for (const candidate of candidates) {
      if (!candidate || candidate.kind !== 'object' || !Array.isArray(candidate.properties)) continue
      const literal = candidate.properties.find(p => p.name === 'type')
      const value = literal ? nodes[literal.type] : undefined
      if (value && value.kind === 'literal' && typeof value.value === 'string') blockTypes.add(value.value)
    }
  }
}

/** 事件 data 的字段名，用于将来做字段级映射时参考。 */
const dataFields = {}
for (const root of roots) {
  if (root.kind !== 'event') continue
  const nodes = root.schema?.nodes
  const index = root.schema?.root
  if (!Array.isArray(nodes) || typeof index !== 'number') {
    dataFields[eventName(root)] = []
    continue
  }
  const envelope = nodes[index]
  const dataProp = Array.isArray(envelope?.properties)
    ? envelope.properties.find(p => p.name === 'data')
    : undefined
  if (!dataProp) {
    dataFields[eventName(root)] = []
    continue
  }
  dataFields[eventName(root)] = fieldNames(nodes[dataProp.type]).sort()
}

const output = {
  note: '由 DSH 的 docs/persistence-schema.json 生成，勿手改；权威在 DSH 的 SessionEventMap 声明。',
  events,
  surfaces,
  contentBlockTypes: [...blockTypes].sort(),
  dataFields,
}

await mkdir(dirname(target), { recursive: true })
await writeFile(target, JSON.stringify(output, null, 2) + '\n', 'utf8')
console.log(JSON.stringify({
  events: events.length,
  surfaces: surfaces.length,
  contentBlockTypes: output.contentBlockTypes.length,
}))
