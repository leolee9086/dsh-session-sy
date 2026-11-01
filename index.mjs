/**
 * dsh-session-sy：DSH 会话与思源 .sy 的转换入口。
 *
 * 加载随包发布的 rebec 原生扩展，调用方不需要知道二进制放在哪、也不需要自己解析路径。
 */
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
let loaded;

/** 加载原生扩展；失败时把文件路径带进错误，平台或 ABI 不匹配一眼能看出来。 */
export function addon() {
  if (loaded !== undefined) return loaded;
  const file = fileURLToPath(new URL("./addon/rebec.node", import.meta.url));
  try { loaded = require(file); }
  catch (error) { throw new Error(`rebec 原生扩展加载失败：${file}`, { cause: error }); }
  if (loaded === null || typeof loaded !== "object") throw new Error("rebec 原生扩展没有导出对象");
  return loaded;
}

/** 多帧 zstd 会话日志（Buffer）→ 会话 JSON 文本。 */
export function sessionFromLog(bytes) { return addon().sessionFromLog(bytes); }

/** 规范 JSONL 文本 → 会话 JSON 文本。 */
export function sessionFromJsonl(text) { return addon().sessionFromJsonl(text); }

/**
 * 会话头与事件（结构化的 JSON 值）→ 会话 JSON 文本。
 *
 * 宿主给出的头是内存形状；物理日志第一行还带 `type: "session"` 与 `delegationDepth`。
 * 这一步属于日志格式，留在转换封装里，调用方只负责搬结构化的数据。
 */
export function sessionFromParts(header, events) {
  if (header === null || typeof header !== "object" || !Array.isArray(events)) {
    throw new TypeError("sessionFromParts 需要会话头对象和事件数组");
  }
  const lines = [JSON.stringify({ type: "session", ...header, delegationDepth: header.delegationDepth ?? 0 })];
  for (const event of events) lines.push(JSON.stringify(event));
  return sessionFromJsonl(`${lines.join("\n")}\n`);
}

/** 会话 JSON 文本 → .sy 文档 JSON 文本；spin 决定是否挂原始事件。 */
export function sessionToSy(sessionJson, spin, assets) { return addon().sessionToSy(sessionJson, spin, assets); }

/** .sy 文档 JSON 文本 → 会话 JSON 文本。 */
export function syToSession(sy) { return addon().syToSession(sy); }

/** rebec 版本。 */
export function version() { return addon().version(); }
