//! session-sy：DSH 会话日志与思源 \`.sy\` 文档之间的双向转换。
//!
//! 分层（下层不知道上层）：
//!   - [\`session\`]：读会话日志（多帧 zstd + JSONL），头与事件都保留原始 JSON
//!   - [\`md\`]：把 markdown 解析成 Lute 风格的 AST（与 \`commonmark-fixtures.json\` 等价）
//!   - [\`model\`]：思源块 id 派生
//!   - [\`render\`]：把会话装配成 \`.sy\`（clean / spin 两种配置），以及反向还原
//!
//! 判据只有两个外部系统：思源自己的解析器能读 \`.sy\`，还原出的会话与 DSH 逐字段深等。

pub mod md;
pub mod model;
pub mod render;
pub mod session;
