#!/usr/bin/env node
/**
 * 用 DSH 自己的 Session 校验一段事件序列。
 *
 * 这是会话正确性的外部判据：重建出来的事件必须能被 DSH 装载并派生消息，
 * 而不是「我的另一套实现同意」。
 *
 * 用法: node scripts/dsh-validate.mjs <session.jsonl.zstd> [rebuilt.jsonl]
 * 不给 rebuilt 时只校验原始日志（用来确认判据本身是通的）。
 */

import { readFile } from 'node:fs/promises'

const SESSION_LIB = 'file:///D:/dev/deepseek-harness/packages/core/session/lib/index.js'

const { Session } = await import(SESSION_LIB)

const [, , originalPath, rebuiltPath] = process.argv
if (!originalPath) {
  console.error('用法: node scripts/dsh-validate.mjs <session.jsonl.zstd> [rebuilt.jsonl]')
  process.exit(2)
}

// 原始日志是 zstd 多帧，这里复用项目里的解码器，避免重复实现。
const { decodeZstdFrames, parseDshJsonl } = await import('../lib/adapters/dsh.js')

const original = parseDshJsonl(decodeZstdFrames(await readFile(originalPath), originalPath), originalPath)

const header = { type: 'session', version: original.formatVersion, id: original.sessionId, ...original.headerExtra }
const events = original.events.map(event => event.raw)

const session = Session.create(original.sessionId, events, header)
const messages = session.deriveMessages()

const result = {
  sessionId: original.sessionId,
  eventsAccepted: session.snapshotEvents().length,
  messagesDerived: messages.length,
  roles: [...new Set(messages.map(message => message.role))],
}

if (rebuiltPath !== undefined) {
  const text = await readFile(rebuiltPath, 'utf8')
  const lines = text.split('\n').filter(line => line.trim().length > 0)
  const rebuiltHeader = JSON.parse(lines[0])
  const rebuiltEvents = lines.slice(1).map(line => JSON.parse(line))
  const rebuiltSession = Session.create(rebuiltHeader.id, rebuiltEvents, rebuiltHeader)
  result.rebuilt = {
    eventsAccepted: rebuiltSession.snapshotEvents().length,
    messagesDerived: rebuiltSession.deriveMessages().length,
  }
}

console.log(JSON.stringify(result, null, 2))