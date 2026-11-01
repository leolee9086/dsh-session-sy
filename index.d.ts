/** 随包发布的 rebec 原生扩展对象；字段由 napi 决定，调用方不需要直接使用。 */
export function addon(): Record<string, unknown>;
/** 多帧 zstd 会话日志（Buffer / Uint8Array）→ 会话 JSON 文本。 */
export function sessionFromLog(bytes: Uint8Array): string;
/** 规范 JSONL 文本 → 会话 JSON 文本。 */
export function sessionFromJsonl(text: string): string;
/** 会话头与事件（结构化的 JSON 值）→ 会话 JSON 文本。 */
export function sessionFromParts(header: Record<string, unknown>, events: readonly unknown[]): string;
/** 会话 JSON 文本 → .sy 文档 JSON 文本；spin 决定是否挂原始事件。 */
export function sessionToSy(sessionJson: string, spin?: boolean, assets?: string): string;
/** .sy 文档 JSON 文本 → 会话 JSON 文本。 */
export function syToSession(sy: string): string;
/** rebec 版本。 */
export function version(): string;
