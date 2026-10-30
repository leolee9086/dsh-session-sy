//! 把会话渲染成思源文档。
//!
//! 两种配置共用同一段组装逻辑：
//!   - clean：只产出渲染需要的东西，导入思源后看到的就是 DSH 前端看到的内容。
//!   - spin：结构完全一致，额外在每个消息 callout 的 IAL 上挂原始事件 JSON，
//!     还原时不需要正文，也不会把任何事件挪到别处。
//!
//! 正文顺序就是事件顺序、content 块顺序，不做任何重排或抽离。

mod blocks;
mod stamp;

use serde_json::{json, Map, Value};

pub use stamp::{ARCHIVED_ATTR, MODEL_ATTR, PRESET_ATTR, SESSION_ATTR, SOURCE_ATTR};
use blocks::message_callout;
use stamp::{stamp, Signature};

use crate::model::IdGen;
use crate::session::Session;

/// 输出配置。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// 只保留渲染所需。
    Clean,
    /// 额外携带原文，可逐字节还原。
    Spin,
}

/// 正文 callout 上的事件序号。
pub const INDEX_ATTR: &str = "custom-dsh-index";
/// spin 版挂在 callout 上的原始事件（base64）。
pub const EVENT_ATTR: &str = "custom-dsh-event";

/// 非消息事件挂在这个 callout **之前**（键在 callout 自己的 IAL 上）。
///
/// 事件不集中收拢：它们留在所属消息的位置上，顺序由位置本身表达，
/// 不需要一份全局索引表。
pub const BEFORE_ATTR: &str = "custom-dsh-before";
/// 会话尾部剩余的事件，挂在最后一个 callout 上。
pub const AFTER_ATTR: &str = "custom-dsh-after";
/// 会话头（日志第一行，type: "session"）——它是元数据，不是事件，挂文档上。
pub const HEADER_ATTR: &str = "custom-dsh-header";

/// 渲染整篇文档。assets 是思源的资源目录（图片会落到这里），为 None 时图片只留说明。
pub fn document(session: &Session, profile: Profile, assets: Option<&std::path::Path>) -> Value {
    let mut ids = IdGen::new(session.created_at());
    let mut children: Vec<Value> = Vec::new();
    let mut pending: Vec<Value> = Vec::new();
    let mut last_callout: Option<usize> = None;

    for (index, event) in session.events.iter().enumerate() {
        match message_callout(event, index, &mut ids, profile, assets) {
            Some(mut callout) => {
                if profile == Profile::Spin && !pending.is_empty() {
                    // 这条消息之前发生的事，就地挂在它的 callout 上
                    set_events(&mut callout, BEFORE_ATTR, &pending);
                    pending.clear();
                }
                last_callout = Some(children.len());
                children.push(callout);
            }
            None => {
                if profile == Profile::Spin {
                    pending.push(json!([index, event]));
                }
            }
        }
    }

    let mut document_properties = Map::new();
    let id = ids.next();
    document_properties.insert("id".into(), json!(id));
    document_properties.insert("type".into(), json!("doc"));
    document_properties.insert("title".into(), json!(title_of(session)));
    if profile == Profile::Spin {
        // 头也要能还原，否则读回来第一行就成了事件
        document_properties.insert(
            HEADER_ATTR.into(),
            json!(base64(
                serde_json::to_string(&session.header).unwrap_or_default().as_bytes()
            )),
        );
    }
    if profile == Profile::Spin && !pending.is_empty() {
        // 尾部剩余：挂在最后一个 callout 上；整段会话没有消息时才落到文档上
        match last_callout {
            Some(position) => set_events(&mut children[position], AFTER_ATTR, &pending),
            None => {
                document_properties.insert(
                    BEFORE_ATTR.into(),
                    json!(base64(
                        serde_json::to_string(&pending).unwrap_or_default().as_bytes()
                    )),
                );
            }
        }
    }

    let mut document = json!({
        "ID": id,
        "Spec": "1",
        "Type": "NodeDocument",
        "Properties": document_properties,
        "Children": children,
    });
    // 两种配置都盖签名：以后搬走任何一块，都能查回它的出处
    let signature = Signature::of(session);
    stamp(&mut document, &signature);
    document
}

/// 把事件数组写进节点的 IAL（base64 的 JSON）。
fn set_events(node: &mut Value, key: &str, events: &[Value]) {
    if let Value::Object(record) = node {
        let mut properties = record
            .get("Properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        properties.insert(
            key.into(),
            json!(base64(
                serde_json::to_string(&events).unwrap_or_default().as_bytes()
            )),
        );
        record.insert("Properties".into(), Value::Object(properties));
    }
}

/// 文档标题：DSH 把标题记在 session/title 事件里。
fn title_of(session: &Session) -> String {
    for event in session.events.iter().rev() {
        if event.get("type").and_then(Value::as_str) != Some("session/title") {
            continue;
        }
        let title = event
            .get("data")
            .and_then(|data| data.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !title.is_empty() {
            return title.to_owned();
        }
    }
    "会话".to_owned()
}









/// 从 spin 文档还原事件序列（按原始顺序）。
///
/// 消息事件在各自 callout 的属性里，其余事件在文档属性里；两者按序号合并排序。
pub fn restore(document: &Value) -> Result<(Value, Vec<Value>), String> {
    let mut events: Vec<Value> = Vec::new();
    let header = document
        .get("Properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(HEADER_ATTR))
        .and_then(Value::as_str)
        .ok_or("文档属性里没有会话头（这份文档不是 spin 版）")?;
    let header_bytes = base64_decode(header)?;
    let header: Value = serde_json::from_slice(&header_bytes).map_err(|error| error.to_string())?;

    // 整段会话没有消息时，事件才落在文档上
    if let Some(properties) = document.get("Properties").and_then(Value::as_object) {
        if let Some(encoded) = properties.get(BEFORE_ATTR).and_then(Value::as_str) {
            events.extend(decode_events(encoded)?);
        }
    }

    if let Some(children) = document.get("Children").and_then(Value::as_array) {
        for child in children {
            if child.get("Type").and_then(Value::as_str) != Some("NodeCallout") {
                continue;
            }
            let Some(properties) = child.get("Properties").and_then(Value::as_object) else {
                continue;
            };
            // 位置即顺序：先补这条消息之前的事件，再是消息本身，最后是尾部残余
            if let Some(encoded) = properties.get(BEFORE_ATTR).and_then(Value::as_str) {
                events.extend(decode_events(encoded)?);
            }
            let encoded = properties
                .get(EVENT_ATTR)
                .and_then(Value::as_str)
                .ok_or("callout 上找不到事件属性（这份文档不是 spin 版）")?;
            let bytes = base64_decode(encoded)?;
            let event: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            events.push(event);
            if let Some(encoded) = properties.get(AFTER_ATTR).and_then(Value::as_str) {
                events.extend(decode_events(encoded)?);
            }
        }
    }
    Ok((header, events))
}

/// 解出 [序号, 事件] 数组里的事件本体。
fn decode_events(encoded: &str) -> Result<Vec<Value>, String> {
    let bytes = base64_decode(encoded)?;
    let list: Vec<Value> = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut events = Vec::new();
    for item in list {
        let event = item.get(1).cloned().ok_or("属性里的条目缺少事件")?;
        events.push(event);
    }
    Ok(events)
}

/// 标准 base64 解码。
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Result<u32, String> {
        match byte {
            b'A'..=b'Z' => Ok((byte - b'A') as u32),
            b'a'..=b'z' => Ok((byte - b'a') as u32 + 26),
            b'0'..=b'9' => Ok((byte - b'0') as u32 + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("非法的 base64 字符: {}", byte as char)),
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return Err("base64 长度不对".into());
        }
        let mut n = 0u32;
        let mut pad = 0;
        for (position, byte) in chunk.iter().enumerate() {
            if *byte == b'=' {
                pad += 1;
                n <<= 6;
            } else {
                n = (n << 6) | value(*byte)?;
                let _ = position;
            }
        }
        // 补齐到 4 字节
        for _ in chunk.len()..4 {
            n <<= 6;
            pad += 1;
        }
        out.push(((n >> 16) & 0xFF) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xFF) as u8);
        }
        if pad < 1 {
            out.push((n & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// 标准 base64（带填充）。
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}
