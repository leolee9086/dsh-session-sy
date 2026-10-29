/**
 * .sy 文档构造与读取。
 *
 * 只负责「把 JSON 变成合法的 SyNode 树」和反向提取，不掺业务语义。
 */

import type { SyDocument, SyNode } from './types.js'

/** 由会话 id 派生一个稳定的、符合思源格式的块 id（14 位时间戳 + 7 位小写字母数字）。 */
export function blockId(createdAt: number, serial: number): string {
  const d = new Date(createdAt)
  const pad = (n: number, w: number) => String(n).padStart(w, '0')
  const stamp = pad(d.getUTCFullYear(), 4) + pad(d.getUTCMonth() + 1, 2) + pad(d.getUTCDate(), 2)
    + pad(d.getUTCHours(), 2) + pad(d.getUTCMinutes(), 2) + pad(d.getUTCSeconds(), 2)
  // 序列号映射到 7 位 base36，保证同输入同输出
  const tail = serial.toString(36).padStart(7, '0').slice(-7)
  return stamp + '-' + tail
}

/** 段落块：内容是一个 NodeText。 */
export function paragraph(id: string, text: string): SyNode {
  return {
    ID: id,
    Type: 'NodeParagraph',
    Properties: { id },
    Children: [{ Type: 'NodeText', Data: text }],
  }
}

/** 代码块：内容在 NodeCodeBlockCode.Data 里，字面量存储，Lute 不解析也不转义。 */
export function codeBlock(id: string, data: string, attrs: Record<string, string> = {}): SyNode {
  return {
    ID: id,
    Type: 'NodeCodeBlock',
    IsFencedCodeBlock: true,
    Properties: { id, ...attrs },
    Children: [{ Type: 'NodeCodeBlockCode', Data: data }],
  }
}

/** 取代码块的正文；不是代码块则返回 undefined。 */
export function codeBlockData(node: SyNode): string | undefined {
  if (node.Type !== 'NodeCodeBlock') return undefined
  const child = node.Children?.find(item => item.Type === 'NodeCodeBlockCode')
  return typeof child?.Data === 'string' ? child.Data : undefined
}

/** 新建一份空文档。 */
export function emptyDocument(id: string, title: string): SyDocument {
  return {
    ID: id,
    Spec: '2',
    Type: 'NodeDocument',
    Properties: { id, title },
    Children: [],
  }
}

/** 读 IAL 里我们自己的标记。 */
export function attrOf(node: SyNode, key: string): string | undefined {
  return node.Properties?.[key]
}
