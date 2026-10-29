/**
 * SessionDocument <-> SyDocument 的双向转换（纯 spin，必须无损）。
 *
 * 一个会话可能大到思源打不开（最大的一份解压后 539MB / 219434 行），所以转换结果是
 * 「一片或多片文档」，而不是永远一个。分片边界只按块大小切，不按语义切；反向按
 * part 序号拼回，拼接后与原事件序列逐字段相等。
 *
 * 单片布局：
 *   根 Properties : id / title / custom-sy-agent / custom-sy-session / custom-sy-part / custom-sy-parts
 *   Children[0]   : 头块（代码块，会话头 JSON）——每片都带，任意一片单独也能读懂
 *   其余          : 按事件原顺序混排
 *     - 可见面事件 -> NodeBlockquote 容器
 *         子块 = 该消息 content[] 的每一项（可读）
 *         末子块 = meta 代码块（事件的其余一切，JSON）
 *     - 其余事件 -> 连续打成代码块，每行一个事件 JSON
 *
 * 字段归属用减法而不是加法：type 与 content 位置进 IAL、content 展开成子块，剩下的一切
 * 整包进 meta 块。凡是「挑字段」的写法都必然漏，所以这里不挑。
 */

import type { SessionDocument, SessionEvent } from './ir.js'
import type { SyDocument, SyNode } from './sy/types.js'
import { ATTR } from './sy/types.js'
import { attrOf, blockId, codeBlock, codeBlockData, emptyDocument } from './sy/document.js'
import { contentToNode, isContentBlock, nodeToContent, type ContentBlock } from './mapping.js'
import { SURFACE_EVENT_TYPES } from './generated/session-schema.js'

/** 每个事件批包含的事件数。 */
const BATCH_SIZE = 100

/** 默认单片字节上限；0 表示不分片。 */
export const DEFAULT_MAX_PART_BYTES = 4 * 1024 * 1024

export const AGENT_ATTR = 'custom-sy-agent'
export const SESSION_ATTR = 'custom-sy-session'
export const PART_ATTR = 'custom-sy-part'
export const PARTS_ATTR = 'custom-sy-parts'
export const EVENT_TYPE_ATTR = 'custom-event-type'
export const CONTENT_PATH_ATTR = 'custom-content-path'
export const META_KIND = 'meta'

/**
 * 会产生消息、值得展开成可读结构的事件类型。
 *
 * 来自 DSH 发布的 persistence-schema，不手写：漏一个就会把整条消息塞进代码块。
 * 其余事件（包括插件扩展的未知类型）一律走批量代码块，那条路是无损的。
 */
const SURFACE_TYPES: ReadonlySet<string> = new Set<string>(SURFACE_EVENT_TYPES)

/** 转换选项。 */
export interface SpinOptions {
  /** 单片最大字节数；省略时用 DEFAULT_MAX_PART_BYTES，传 0 表示强制不分片。 */
  readonly maxPartBytes?: number
}

function headerRecord(doc: SessionDocument): Record<string, unknown> {
  return { type: 'session', version: doc.formatVersion, id: doc.sessionId, ...doc.headerExtra }
}

function headerTimestamp(doc: SessionDocument): number {
  const value = doc.headerExtra.createdAt
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

/** 从 data 里取出 content 数组，并说明它挂在哪一层。取不到返回 undefined。 */
function contentOf(data: unknown): { blocks: ContentBlock[]; fromMessage: boolean } | undefined {
  const record = asRecord(data)
  if (record === undefined) return undefined
  if (Array.isArray(record.content) && record.content.every(isContentBlock)) {
    return { blocks: record.content, fromMessage: false }
  }
  const message = asRecord(record.message)
  if (message !== undefined && Array.isArray(message.content) && message.content.every(isContentBlock)) {
    return { blocks: message.content, fromMessage: true }
  }
  return undefined
}

/** data 去掉 content 之后的样子；重建时把 content 塞回原位。 */
function dataWithoutContent(data: unknown, fromMessage: boolean): Record<string, unknown> {
  const record = asRecord(data) ?? {}
  if (!fromMessage) {
    const { content: _content, ...rest } = record
    return rest
  }
  const message = asRecord(record.message) ?? {}
  const { content: _content, ...messageRest } = message
  return { ...record, message: messageRest }
}

/** 一片文档的装配状态。 */
interface PartBuilder {
  readonly doc: SyDocument
  bytes: number
}

/** 正向：会话 -> 一片或多片 .sy 文档。 */
export function spin(doc: SessionDocument, options: SpinOptions = {}): SyDocument[] {
  const maxBytes = options.maxPartBytes ?? DEFAULT_MAX_PART_BYTES
  const stamp = headerTimestamp(doc)
  const title = typeof doc.headerExtra.title === 'string' && doc.headerExtra.title.length > 0
    ? doc.headerExtra.title
    : doc.sessionId
  const header = headerRecord(doc)
  const headerBytes = JSON.stringify(header).length

  let serial = 0
  const nextSerial = (): number => serial++
  const nextId = (): string => blockId(stamp, nextSerial())

  const created: PartBuilder[] = []
  const openPart = (): PartBuilder => {
    const document = emptyDocument(blockId(stamp, nextSerial()), title)
    document.Properties[AGENT_ATTR] = doc.agent
    document.Properties[SESSION_ATTR] = doc.sessionId
    document.Properties[PART_ATTR] = String(created.length)
    document.Children.push(codeBlock(nextId(), JSON.stringify(header), { [ATTR.kind]: 'header' }))
    const builder: PartBuilder = { doc: document, bytes: headerBytes }
    created.push(builder)
    return builder
  }

  let current = openPart()
  const push = (node: SyNode, bytes: number): void => {
    const hasContent = current.doc.Children.length > 1
    if (maxBytes > 0 && hasContent && current.bytes + bytes > maxBytes) current = openPart()
    current.doc.Children.push(node)
    current.bytes += bytes
  }

  let pending: SessionEvent[] = []
  const flush = (): void => {
    if (pending.length === 0) return
    const data = pending.map(event => JSON.stringify(event.raw)).join('\n')
    const attrs: Record<string, string> = { [ATTR.kind]: 'events', [ATTR.batch]: String(serial) }
    const firstSeq = pending[0]?.raw.seq
    if (typeof firstSeq === 'number') attrs[ATTR.firstSeq] = String(firstSeq)
    push(codeBlock(nextId(), data, attrs), data.length)
    pending = []
  }

  for (const event of doc.events) {
    const expanded = expandable(event, nextId)
    if (expanded === undefined) {
      pending.push(event)
      if (pending.length >= BATCH_SIZE) flush()
      continue
    }
    flush()
    push(expanded, JSON.stringify(expanded).length)
  }
  flush()

  // 片数只有到最后才知道，回填到每一片的属性里。
  for (const builder of created) builder.doc.Properties[PARTS_ATTR] = String(created.length)
  return created.map(builder => builder.doc)
}

/** 把可见面事件展开成可读容器；不适合展开时返回 undefined。 */
function expandable(event: SessionEvent, nextId: () => string): SyNode | undefined {
  const raw = event.raw
  const type = raw.type
  if (typeof type !== 'string' || !SURFACE_TYPES.has(type)) return undefined
  const content = contentOf(raw.data)
  if (content === undefined) return undefined

  const { type: _type, data, ...envelope } = raw
  const rest = dataWithoutContent(data, content.fromMessage)

  const id = nextId()
  const container: SyNode = {
    ID: id,
    Type: 'NodeBlockquote',
    Properties: {
      id,
      [EVENT_TYPE_ATTR]: type,
      [CONTENT_PATH_ATTR]: content.fromMessage ? 'message' : 'data',
    },
    Children: content.blocks.map(block => contentToNode(block, nextId())),
  }
  container.Children?.push(codeBlock(nextId(), JSON.stringify({ envelope, data: rest }), { [ATTR.kind]: META_KIND }))
  return container
}

function partIndexOf(sy: SyDocument): number {
  const value = sy.Properties[PART_ATTR]
  if (typeof value !== 'string') throw new Error('.sy 文档缺少 ' + PART_ATTR)
  const index = Number(value)
  if (!Number.isInteger(index) || index < 0) throw new Error(PART_ATTR + ' 不是非负整数: ' + value)
  return index
}

/** 反向：一片或多片文档 -> 会话。分片按 part 序号拼接。 */
export function spinBack(parts: readonly SyDocument[]): SessionDocument {
  if (parts.length === 0) throw new Error('没有可转换的 .sy 文档')
  const ordered = [...parts].sort((a, b) => partIndexOf(a) - partIndexOf(b))

  let agent: string | undefined
  let header: Record<string, unknown> | undefined
  const events: SessionEvent[] = []

  for (const sy of ordered) {
    const partAgent = sy.Properties[AGENT_ATTR]
    if (typeof partAgent !== 'string' || partAgent.length === 0) {
      throw new Error('.sy 文档缺少 ' + AGENT_ATTR + '，无法判断来源 agent')
    }
    if (agent === undefined) agent = partAgent
    else if (agent !== partAgent) throw new Error('分片的来源 agent 不一致: ' + agent + ' vs ' + partAgent)

    for (const node of sy.Children) {
      const kind = attrOf(node, ATTR.kind)

      if (kind === 'header') {
        const parsed = parseObject(codeBlockDataOf(node), '头块')
        if (header === undefined) header = parsed
        continue
      }

      if (kind === 'events') {
        for (const line of codeBlockDataOf(node).split('\n')) {
          if (line.trim().length === 0) continue
          events.push({ raw: parseObject(line, '事件行') })
        }
        continue
      }

      const eventType = attrOf(node, EVENT_TYPE_ATTR)
      if (eventType !== undefined) {
        events.push({ raw: eventFromNode(node, eventType) })
        continue
      }

      throw new Error('未知节点 ' + String(node.ID) + '（没有事件类型也没有块角色）')
    }
  }

  if (agent === undefined) throw new Error('没有可转换的 .sy 文档')
  if (header === undefined) throw new Error('.sy 文档缺少头块')
  const version = header.version
  const id = header.id
  if (typeof version !== 'number') throw new Error('头块缺少 version')
  if (typeof id !== 'string' || id.length === 0) throw new Error('头块缺少 id')

  const headerExtra: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(header)) {
    if (key === 'type' || key === 'version' || key === 'id') continue
    headerExtra[key] = value
  }

  return { agent, formatVersion: version, sessionId: id, headerExtra, events }
}

/** 从一个容器块重建事件：content 从子块取，其余从 meta 块取。 */
function eventFromNode(node: SyNode, eventType: string): Record<string, unknown> {
  const children = node.Children ?? []
  const contentNodes = children.filter(child => attrOf(child, ATTR.kind) !== META_KIND)
  const metaNode = children.find(child => attrOf(child, ATTR.kind) === META_KIND)
  if (metaNode === undefined) throw new Error('事件块 ' + String(node.ID) + ' 缺少 meta 子块')

  const meta = parseObject(codeBlockDataOf(metaNode), '元数据块')
  const envelope = asRecord(meta.envelope)
  const rest = asRecord(meta.data)
  if (envelope === undefined || rest === undefined) {
    throw new Error('事件块 ' + String(node.ID) + ' 的 meta 结构不完整')
  }

  const blocks = contentNodes.map(nodeToContent)
  const fromMessage = attrOf(node, CONTENT_PATH_ATTR) === 'message'
  const data = fromMessage
    ? { ...rest, message: { ...(asRecord(rest.message) ?? {}), content: blocks } }
    : { ...rest, content: blocks }

  return { type: eventType, ...envelope, data }
}

function codeBlockDataOf(node: SyNode): string {
  const data = codeBlockData(node)
  if (data === undefined) throw new Error('节点 ' + String(node.ID) + ' 不是代码块')
  return data
}

function parseObject(text: string, label: string): Record<string, unknown> {
  const parsed: unknown = JSON.parse(text)
  const record = asRecord(parsed)
  if (record === undefined) throw new Error(label + ' 不是 JSON 对象')
  return record
}
