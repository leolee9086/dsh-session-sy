/**
 * 等价性验证：判断两次 spin 之间是否真的没有信息损失。
 *
 * 判据是「深等」而不是「字节等」：.sy 是 JSON，键顺序在解析/序列化中会变，
 * 但字段、类型、数组顺序、数值必须逐个相等。
 */

import { isDeepStrictEqual } from 'node:util'
import type { SessionDocument } from './ir.js'

/** 比较两份会话是否等价；不等时返回第一处差异的路径。 */
export function firstDifference(left: SessionDocument, right: SessionDocument): string | null {
  return diff(left, right, '$')
}

function diff(left: unknown, right: unknown, path: string): string | null {
  if (isDeepStrictEqual(left, right)) return null

  const leftType = kindOf(left)
  const rightType = kindOf(right)
  if (leftType !== rightType) return path + ': 类型不同 ' + leftType + ' vs ' + rightType

  if (Array.isArray(left) && Array.isArray(right)) {
    if (left.length !== right.length) return path + ': 长度不同 ' + left.length + ' vs ' + right.length
    for (let i = 0; i < left.length; i++) {
      const found = diff(left[i], right[i], path + '[' + i + ']')
      if (found !== null) return found
    }
    return null
  }

  if (leftType === 'object') {
    const a = left as Record<string, unknown>
    const b = right as Record<string, unknown>
    const aKeys = Object.keys(a).sort()
    const bKeys = Object.keys(b).sort()
    if (aKeys.length !== bKeys.length || aKeys.some((key, i) => key !== bKeys[i])) {
      return path + ': 键集合不同 [' + aKeys.join(',') + '] vs [' + bKeys.join(',') + ']'
    }
    for (const key of aKeys) {
      const found = diff(a[key], b[key], path + '.' + key)
      if (found !== null) return found
    }
    return null
  }

  return path + ': 值不同 ' + JSON.stringify(left) + ' vs ' + JSON.stringify(right)
}

function kindOf(value: unknown): string {
  if (value === null) return 'null'
  if (Array.isArray(value)) return 'array'
  return typeof value
}
