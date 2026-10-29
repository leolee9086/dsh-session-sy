/**
 * .sy 结构自检：按 SiYuan docs/SY-FORMAT.md（Spec 2）的规范校验生成的文档。
 *
 * 只检查「写出来的东西是不是合法块树」，不检查内容语义。
 */

import type { SyDocument, SyNode } from './types.js'

/** 思源块 id 格式：14 位时间戳 + 7 位小写字母数字。 */
const BLOCK_ID = /^\d{14}-[a-z0-9]{7}$/

/** 叶子块：内容在自己或子节点的 Data 里，没有再分块。 */
const LEAF_BLOCKS = new Set([
  'NodeParagraph', 'NodeHeading', 'NodeThematicBreak', 'NodeHTMLBlock', 'NodeCodeBlock',
  'NodeMathBlock', 'NodeTable', 'NodeBlockQueryEmbed', 'NodeAttributeView', 'NodeIFrame',
  'NodeVideo', 'NodeAudio', 'NodeWidget', 'NodeCustomBlock',
])

/** 容器块：子节点仍是块。 */
const CONTAINER_BLOCKS = new Set([
  'NodeList', 'NodeListItem', 'NodeBlockquote', 'NodeCallout', 'NodeSuperBlock', 'NodeTabs', 'NodeTabItem',
])

/** 行内节点：不再包含块。 */
const INLINE_NODES = new Set([
  'NodeText', 'NodeTextMark', 'NodeImage', 'NodeKramdownSpanIAL', 'NodeSoftBreak', 'NodeBr',
  'NodeCodeBlockCode', 'NodeCodeBlockFenceOpenMarker', 'NodeCodeBlockFenceInfoMarker', 'NodeCodeBlockFenceCloseMarker',
])

/** 校验一份文档，返回发现的问题；空数组表示通过。 */
export function validateDocument(doc: SyDocument): string[] {
  const problems: string[] = []
  if (doc.Type !== 'NodeDocument') problems.push('根节点 Type 必须是 NodeDocument')
  if (typeof doc.Spec !== 'string') problems.push('根节点缺少 Spec')
  if (!isBlockId(doc.ID)) problems.push('根节点 ID 不合法: ' + String(doc.ID))
  if (doc.Properties?.id !== doc.ID) problems.push('根节点 Properties.id 与 ID 不一致')
  if (!Array.isArray(doc.Children)) problems.push('根节点缺少 Children')
  else doc.Children.forEach((node, index) => checkNode(node, 'Children[' + index + ']', problems))
  return problems
}

function checkNode(node: SyNode, path: string, problems: string[]): void {
  if (typeof node.Type !== 'string' || node.Type.length === 0) {
    problems.push(path + ': 缺少 Type')
    return
  }

  const isBlock = LEAF_BLOCKS.has(node.Type) || CONTAINER_BLOCKS.has(node.Type)
  if (isBlock) {
    if (!isBlockId(node.ID)) problems.push(path + ': 块 ID 不合法 ' + String(node.ID))
    if (node.Properties?.id !== node.ID) problems.push(path + ': Properties.id 与 ID 不一致')
  }

  if (node.Type === 'NodeCodeBlock') {
    const code = node.Children?.find(child => child.Type === 'NodeCodeBlockCode')
    if (code === undefined) problems.push(path + ': 代码块缺少 NodeCodeBlockCode')
    else if (typeof code.Data !== 'string') problems.push(path + ': NodeCodeBlockCode 缺少 Data')
    if (node.Children !== undefined) {
      for (const child of node.Children) {
        if (!INLINE_NODES.has(child.Type)) problems.push(path + ': 代码块下出现非行内节点 ' + child.Type)
      }
    }
    return
  }

  if (node.Type === 'NodeParagraph') {
    if (!Array.isArray(node.Children) || node.Children.length === 0) problems.push(path + ': 段落缺少子节点')
    else for (const child of node.Children) {
      if (!INLINE_NODES.has(child.Type)) problems.push(path + ': 段落下出现非行内节点 ' + child.Type)
    }
    return
  }

  if (!isBlock && !INLINE_NODES.has(node.Type)) problems.push(path + ': 未知节点类型 ' + node.Type)

  if (Array.isArray(node.Children)) {
    node.Children.forEach((child, index) => checkNode(child, path + '.Children[' + index + ']', problems))
  }
}

function isBlockId(value: unknown): boolean {
  return typeof value === 'string' && BLOCK_ID.test(value)
}
