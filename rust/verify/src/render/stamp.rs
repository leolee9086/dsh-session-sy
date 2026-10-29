//! 块级来源签名与思源 ID 分配。
//!
//! 签名单独放在块上而不是文档上，是因为块以后会被搬走、被引用、被重排，
//! 离开本文档后仍要能说清自己从哪来。

use serde_json::{json, Value};

use crate::model::IdGen;
use crate::session::Session;

// 通用部分：将来从别的软件导入也用这套键，值说明来源
pub const SOURCE_ATTR: &str = "custom-import-source";
pub const ARCHIVED_ATTR: &str = "custom-import-archived";
pub const SESSION_ATTR: &str = "custom-dsh-session";
// dsh 专有部分：只有来源确实是 dsh 时才加
pub const PRESET_ATTR: &str = "custom-dsh-preset";
/// 产出这块内容的模型（只有 LLM 输出才标）。
pub const MODEL_ATTR: &str = "custom-dsh-model";

/// 一次归档的签名。
pub(super) struct Signature {
    source: String,
    archived: i64,
    session: String,
    preset: String,
}

impl Signature {
    pub(super) fn of(session: &Session) -> Self {
        Signature {
            source: "dsh".to_owned(),
            archived: now_millis(),
            session: session.id().to_owned(),
            preset: session
                .header
                .get("agentPreset")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 给树里每个块标上产出它的模型（递归）。
pub(super) fn stamp_model(node: &mut Value, model: &str) {
    let node_type = node
        .get("Type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if is_block(&node_type) {
        if let Value::Object(record) = node {
            let mut properties = record
                .get("Properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            properties.insert(MODEL_ATTR.into(), json!(model));
            record.insert("Properties".into(), Value::Object(properties));
        }
    }
    if let Some(children) = node.get_mut("Children").and_then(Value::as_array_mut) {
        for child in children.iter_mut() {
            stamp_model(child, model);
        }
    }
}

/// 给树里每个块盖上传来源签名（递归）。
pub(super) fn stamp(node: &mut Value, signature: &Signature) {
    let node_type = node
        .get("Type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if is_block(&node_type) {
        if let Value::Object(record) = node {
            let mut properties = record
                .get("Properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            properties.insert(SOURCE_ATTR.into(), json!(signature.source));
            properties.insert(ARCHIVED_ATTR.into(), json!(signature.archived.to_string()));
            properties.insert(SESSION_ATTR.into(), json!(signature.session));
            if signature.source == "dsh" {
                // 预设是 dsh 的概念，别的来源不需要这个键
                properties.insert(PRESET_ATTR.into(), json!(signature.preset));
            }
            record.insert("Properties".into(), Value::Object(properties));
        }
    }
    if let Some(children) = node.get_mut("Children").and_then(Value::as_array_mut) {
        for child in children.iter_mut() {
            stamp(child, signature);
        }
    }
}

/// 给块级节点补思源要求的 ID 与 Properties（递归）。
pub(super) fn assign_ids(node: &mut Value, ids: &mut IdGen) {
    let node_type = node
        .get("Type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if is_block(&node_type) {
        let id = ids.next();
        if let Value::Object(record) = node {
            record.insert("ID".into(), json!(id.clone()));
            let mut properties = record
                .get("Properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            properties.insert("id".into(), json!(id));
            record.insert("Properties".into(), Value::Object(properties));
        }
    }
    if let Some(children) = node.get_mut("Children").and_then(Value::as_array_mut) {
        for child in children.iter_mut() {
            assign_ids(child, ids);
        }
    }
}

/// 思源需要 ID 的块类型。
fn is_block(node_type: &str) -> bool {
    matches!(
        node_type,
        "NodeParagraph"
            | "NodeHeading"
            | "NodeCodeBlock"
            | "NodeBlockquote"
            | "NodeList"
            | "NodeListItem"
            | "NodeThematicBreak"
            | "NodeHTMLBlock"
            | "NodeCallout"
            | "NodeLinkRefDefBlock"
            | "NodeLinkRefDef"
    )
}
