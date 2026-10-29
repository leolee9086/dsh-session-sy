/**
 * 思源 .sy 文档的最小 JSON 模型。
 *
 * 依据 SiYuan docs/SY-FORMAT.md（Spec 2）：
 *   块 = { ID, Type, Properties(IAL), Children? }，叶子块的内容在自己或子节点的 Data 里。
 *   Properties 的值一律是字符串。
 */

/** 一个 .sy 节点。未列出的固定字段按需附加，保持 JSON 原样即可。 */
export interface SyNode {
  ID?: string
  Type: string
  Properties?: Record<string, string>
  Children?: SyNode[]
  Data?: string
  IsFencedCodeBlock?: boolean
  HeadingLevel?: number
  [key: string]: unknown
}

/** 一份 .sy 文档（NodeDocument 为根）。 */
export interface SyDocument {
  ID: string
  Spec: string
  Type: 'NodeDocument'
  Properties: Record<string, string>
  Children: SyNode[]
}

/** 我们写入 IAL 的键名；custom- 前缀是思源给用户属性的命名空间。 */
export const ATTR = {
  /** 节点角色：header / events */
  kind: 'custom-sy-kind',
  /** 批次序号（events 块专用） */
  batch: 'custom-sy-batch',
  /** 该批第一个事件的原始序号，便于人读 */
  firstSeq: 'custom-sy-first-seq',
} as const
