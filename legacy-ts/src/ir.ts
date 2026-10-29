/**
 * 中间表示：任何 agent 的会话都能落到这里。
 *
 * 刻意保持极薄 —— 头 + 有序事件。不解释事件语义，因为解释权属于各适配器；
 * 这里只保证「顺序」和「原样」两件事，spin 的无损性全靠它们。
 */

/**
 * 一条会话事件。
 *
 * 只保留原始 JSON 对象，不拆字段：DSH 的事件信封除 type/seq/time/data 外，
 * 还有 surfaceOp、sourceEventSeqs、ignorable，以及格式迁移遗留的 seq0/time0。
 * 拆字段就会漏，而损失是不可见的。
 */
export interface SessionEvent {
  /** 事件的原始 JSON 对象，原样往返。 */
  readonly raw: Readonly<Record<string, unknown>>
}

/** 一份完整会话：头信息 + 按序事件。 */
export interface SessionDocument {
  /** 来源标识，例如 'dsh'。 */
  readonly agent: string
  /** 源格式版本，例如 3。 */
  readonly formatVersion: number
  /** 会话 id。 */
  readonly sessionId: string
  /** 头的其余字段，原样保留以保证往返无损。 */
  readonly headerExtra: Readonly<Record<string, unknown>>
  /** 按发生顺序排列的事件。 */
  readonly events: readonly SessionEvent[]
}
