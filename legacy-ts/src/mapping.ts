/**
 * 消息内容块 <-> .sy 节点的映射。
 *
 * 块类型的权威清单来自 DSH 的 persistence-schema：text / reasoning / image / file /
 * tool-call / tool-addition / tool-removal。它是 merge-extensible 的，插件还能加，
 * 运行时确实会出现 schema 里没有的 tool-result，所以每种类型都必须有兜底路径。
 *
 * 每个已知类型的字段按类型定义逐个落到节点或 IAL 上；未知类型整块 JSON 进代码块，
 * 一个字段都不丢。
 */

import type { SyNode } from './sy/types.js'
import { codeBlock, codeBlockData, paragraph } from './sy/document.js'

/** 内容块类型。 */
export const BLOCK_TYPE_ATTR = 'custom-block-type'
/** 工具调用 id（tool-call 的 id 与 tool-result 的 toolCallId）。 */
export const CALL_ID_ATTR = 'custom-call-id'
/** 工具名（tool-call）。 */
export const TOOL_NAME_ATTR = 'custom-tool-name'
/** 工具结果是否错误（tool-result）；false 与缺失是两回事。 */
export const ERROR_ATTR = 'custom-is-error'

/** 一个 content 块，形状由 agent 定义；这里只按 type 分派。 */
export interface ContentBlock {
  readonly type: string
  readonly [key: string]: unknown
}

/** 判断是不是内容块。 */
export function isContentBlock(value: unknown): value is ContentBlock {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    && typeof (value as Record<string, unknown>).type === 'string'
}

/**
 * 文本类块（text / reasoning）落成段落：原文进 NodeText.Data，交给 Lute 渲染。
 * 两者都是 { type, text } 两个字段，类型靠 IAL 记住。
 */
function textLikeToNode(id: string, type: string, text: string): SyNode {
  const node = paragraph(id, text)
  node.Properties = { id, [BLOCK_TYPE_ATTR]: type }
  return node
}

/** content 块 -> 节点。 */
export function contentToNode(block: ContentBlock, id: string): SyNode {
  if (block.type === 'text' && typeof block.text === 'string') return textLikeToNode(id, 'text', block.text)
  if (block.type === 'reasoning' && typeof block.text === 'string') return textLikeToNode(id, 'reasoning', block.text)

  if (block.type === 'tool-call') {
    const attrs: Record<string, string> = { [BLOCK_TYPE_ATTR]: 'tool-call' }
    if (typeof block.id === 'string') attrs[CALL_ID_ATTR] = block.id
    if (typeof block.name === 'string') attrs[TOOL_NAME_ATTR] = block.name
    // arguments 在协议里是 JSON 字符串；万一不是，标记成 -json 变体以便原样还原。
    if (typeof block.arguments === 'string') return codeBlock(id, block.arguments, attrs)
    attrs[BLOCK_TYPE_ATTR] = 'tool-call-json'
    return codeBlock(id, JSON.stringify(block.arguments ?? null), attrs)
  }

  if (block.type === 'tool-result') {
    const attrs: Record<string, string> = { [BLOCK_TYPE_ATTR]: 'tool-result' }
    if (typeof block.toolCallId === 'string') attrs[CALL_ID_ATTR] = block.toolCallId
    if (typeof block.isError === 'boolean') attrs[ERROR_ATTR] = String(block.isError)
    return codeBlock(id, JSON.stringify(block.content ?? []), attrs)
  }

  // image / file / tool-addition / tool-removal 以及任何插件扩展类型：整块保留。
  return codeBlock(id, JSON.stringify(block), { [BLOCK_TYPE_ATTR]: block.type })
}

/** 节点 -> content 块。 */
export function nodeToContent(node: SyNode): ContentBlock {
  const type = node.Properties?.[BLOCK_TYPE_ATTR]
  if (typeof type !== 'string' || type.length === 0) {
    throw new Error('节点 ' + String(node.ID) + ' 缺少 ' + BLOCK_TYPE_ATTR)
  }

  if (type === 'text' || type === 'reasoning') {
    const child = node.Children?.find(item => item.Type === 'NodeText')
    const text = typeof child?.Data === 'string' ? child.Data : ''
    return { type, text }
  }

  const data = codeBlockData(node)
  if (data === undefined) throw new Error('节点 ' + String(node.ID) + ' 不是代码块')

  if (type === 'tool-call' || type === 'tool-call-json') {
    const callId = node.Properties?.[CALL_ID_ATTR]
    const name = node.Properties?.[TOOL_NAME_ATTR]
    const block: Record<string, unknown> = { type: 'tool-call' }
    if (callId !== undefined) block.id = callId
    if (name !== undefined) block.name = name
    block.arguments = type === 'tool-call' ? data : JSON.parse(data)
    return block as unknown as ContentBlock
  }

  if (type === 'tool-result') {
    const callId = node.Properties?.[CALL_ID_ATTR]
    const error = node.Properties?.[ERROR_ATTR]
    const block: Record<string, unknown> = { type: 'tool-result' }
    if (callId !== undefined) block.toolCallId = callId
    block.content = JSON.parse(data)
    if (error !== undefined) block.isError = error === 'true'
    return block as unknown as ContentBlock
  }

  const parsed: unknown = JSON.parse(data)
  if (!isContentBlock(parsed)) throw new Error('节点 ' + String(node.ID) + ' 的内容不是对象')
  return parsed
}
