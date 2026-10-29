/**
 * DSH 会话日志适配器。
 *
 * 磁盘布局：<sessions>/<workspace>/<sessionId>/session[.vN].jsonl.zstd
 * 物理编码：多个 zstd 帧首尾相接，每帧独立解压，拼接后才是完整 JSONL。
 * 逻辑内容：第 1 行是会话头，其余每行一个事件。
 *
 * 不迁移格式版本：往返无损要求原样保留事件，迁移属于另一条路径。
 */

import { readFile } from 'node:fs/promises'
import { zstdDecompressSync } from 'node:zlib'
import { StringDecoder } from 'node:string_decoder'
import type { SessionDocument, SessionEvent } from '../ir.js'

/** zstd 帧起始魔数 0x28B52FFD。 */
const FRAME_MAGIC = [0x28, 0xb5, 0x2f, 0xfd] as const

function asRecord(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(label + ': 期望 JSON 对象')
  }
  return value as Record<string, unknown>
}

/** 按魔数切出所有帧的起始偏移。 */
export function frameStarts(buffer: Buffer, label: string): number[] {
  const starts: number[] = []
  for (let i = 0; i + 4 <= buffer.length; i++) {
    if (buffer[i] === FRAME_MAGIC[0] && buffer[i + 1] === FRAME_MAGIC[1]
      && buffer[i + 2] === FRAME_MAGIC[2] && buffer[i + 3] === FRAME_MAGIC[3]) starts.push(i)
  }
  if (starts.length === 0) throw new Error(label + ': 没有找到 zstd 帧')
  return starts
}

/** 多帧 zstd 解压成完整文本；只用于小文件与测试，大文件走流式读取。 */
export function decodeZstdFrames(buffer: Buffer, label: string): string {
  const starts = frameStarts(buffer, label)
  const parts: Buffer[] = []
  for (let k = 0; k < starts.length; k++) {
    const end = k + 1 < starts.length ? starts[k + 1] : buffer.length
    try {
      parts.push(zstdDecompressSync(buffer.subarray(starts[k], end)))
    } catch (error) {
      throw new Error(label + ': 第 ' + k + ' 帧解压失败（字节 ' + starts[k] + '）', { cause: error })
    }
  }
  return Buffer.concat(parts).toString('utf8')
}

/** 由头和事件拼出 SessionDocument。 */
function finishDocument(
  header: Record<string, unknown> | undefined,
  events: SessionEvent[],
  label: string,
): SessionDocument {
  if (header === undefined) throw new Error(label + ': 文件为空')
  if (header.type !== 'session') {
    throw new Error(label + ': 第 1 行不是会话头（type=' + String(header.type) + '）')
  }
  const version = header.version
  const id = header.id
  if (typeof version !== 'number') throw new Error(label + ': 会话头缺少 version')
  if (typeof id !== 'string' || id.length === 0) throw new Error(label + ': 会话头缺少 id')

  const headerExtra: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(header)) {
    if (key === 'type' || key === 'version' || key === 'id') continue
    headerExtra[key] = value
  }

  return { agent: 'dsh', formatVersion: version, sessionId: id, headerExtra, events }
}

/** 把 JSONL 文本拆成「会话头 + 事件」；用于测试与小文件。 */
export function parseDshJsonl(text: string, label: string): SessionDocument {
  const lines = text.split('\n').filter(line => line.trim().length > 0)
  if (lines.length === 0) throw new Error(label + ': 文件为空')

  const header = asRecord(JSON.parse(lines[0]), label + ' 第 1 行')
  const events: SessionEvent[] = []
  for (let i = 1; i < lines.length; i++) {
    const event = asRecord(JSON.parse(lines[i]), label + ' 第 ' + (i + 1) + ' 行')
    if (typeof event.type !== 'string') throw new Error(label + ' 第 ' + (i + 1) + ' 行: 事件缺少 type')
    events.push({ raw: event })
  }
  return finishDocument(header, events, label)
}

/**
 * 读取一个 .zstd 会话文件。
 *
 * 逐帧解压、边解边切行：整份日志可能有上亿字符（最大的一份 218989 个事件），
 * 一次性拼成字符串会撞上 Node 的 0x1fffffe8 上限。
 * StringDecoder 处理跨帧的多字节字符边界，carry 处理跨帧的行边界。
 */
export async function readDshSessionFile(path: string): Promise<SessionDocument> {
  const buffer = await readFile(path)
  const starts = frameStarts(buffer, path)
  const decoder = new StringDecoder('utf8')

  let header: Record<string, unknown> | undefined
  let carry = ''
  let lineNumber = 0
  const events: SessionEvent[] = []

  const consume = (line: string): void => {
    if (line.trim().length === 0) return
    lineNumber++
    const parsed = asRecord(JSON.parse(line), path + ' 第 ' + lineNumber + ' 行')
    if (header === undefined) {
      header = parsed
      return
    }
    if (typeof parsed.type !== 'string') {
      throw new Error(path + ' 第 ' + lineNumber + ' 行: 事件缺少 type')
    }
    events.push({ raw: parsed })
  }

  for (let k = 0; k < starts.length; k++) {
    const end = k + 1 < starts.length ? starts[k + 1] : buffer.length
    let chunk: Buffer
    try {
      chunk = zstdDecompressSync(buffer.subarray(starts[k], end))
    } catch (error) {
      throw new Error(path + ': 第 ' + k + ' 帧解压失败（字节 ' + starts[k] + '）', { cause: error })
    }
    const text = carry + decoder.write(chunk)
    const parts = text.split('\n')
    carry = parts.pop() ?? ''
    for (const line of parts) consume(line)
  }
  carry += decoder.end()
  if (carry.length > 0) consume(carry)

  return finishDocument(header, events, path)
}
