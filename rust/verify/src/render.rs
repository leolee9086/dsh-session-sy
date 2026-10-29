//! 把会话渲染成思源文档。
//!
//! 两种配置共用同一段组装逻辑：
//!   - clean：只产出渲染需要的东西，导入思源后看到的就是 DSH 前端看到的内容。
//!   - spin：结构完全一致，额外在每个消息 callout 的 IAL 上挂原始事件 JSON，
//!     还原时不需要正文，也不会把任何事件挪到别处。
//!
//! 正文顺序就是事件顺序、content 块顺序，不做任何重排或抽离。

use serde_json::{json, Map, Value};

use crate::md;
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

/// 事件类型 -> (callout 类型, 标题, 图标)。
fn role_style(event_type: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match event_type {
        "user/message" => Some(("user", "用户", "👤")),
        "assistant/message" => Some(("assistant", "助手", "🤖")),
        "system/message" => Some(("system", "系统", "⚙️")),
        "developer/message" => Some(("developer", "开发者", "🛠")),
        // 工具结果是独立事件，正文里按顺序给它一个 callout
        "tool/result" => Some(("tool", "工具结果", "🔧")),
        _ => None,
    }
}

/// 一条消息 -> 一个 callout。
fn message_callout(
    event: &Value,
    index: usize,
    ids: &mut IdGen,
    profile: Profile,
    assets: Option<&std::path::Path>,
) -> Option<Value> {
    let event_type = event.get("type").and_then(Value::as_str)?;
    let data = event.get("data")?;

    // 读取图片的子调用：正文里只留图片本身（其余是内部调度信息，前端也不显示）
    if event_type == "tool/ptc-dispatch" {
        let mut nodes = Vec::new();
        for block in &content_blocks(data) {
            if block.get("type").and_then(Value::as_str) == Some("image") {
                if let Some(node) = image_block(block, index, ids, assets) {
                    nodes.push(node);
                }
            }
        }
        if nodes.is_empty() {
            return None;
        }
        let id = ids.next();
        let mut properties = Map::new();
        properties.insert("id".into(), json!(id));
        properties.insert(INDEX_ATTR.into(), json!(index.to_string()));
        return Some(json!({
            "ID": id,
            "Type": "NodeCallout",
            "CalloutType": "image",
            "CalloutTitle": "图片",
            "CalloutTitleExplicit": true,
            "CalloutIcon": "🖼",
            "Properties": properties,
            "Children": nodes,
        }));
    }

    let (callout_type, title, icon) = role_style(event_type)?;
    let blocks = content_blocks(data);

    let mut children: Vec<Value> = Vec::new();
    if event_type == "tool/result" {
        // 工具结果整段包成一个代码块：原文可能是几百行 JSON，
        // 交给 markdown 解析会被拆散成段落和行内代码。
        let mut body = String::new();
        for block in &blocks {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
        }
        if !body.is_empty() {
            children.push(code_block(&body, "", ids));
        }
    } else {
        for block in &blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    let mut tree = md::parse(text);
                    if let Some(items) = tree.get_mut("Children").and_then(Value::as_array_mut) {
                        for item in items.iter_mut() {
                            assign_ids(item, ids);
                            children.push(item.clone());
                        }
                    }
                }
                "reasoning" => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if let Some(node) = reasoning_block(text, ids) {
                        children.push(node);
                    }
                }
                "tool-call" => {
                    children.push(tool_block(block, ids));
                }
                "image" => {
                    if let Some(node) = image_block(block, index, ids, assets) {
                        children.push(node);
                    }
                }
                _ => {}
            }
        }
    }

    // 思源里每个 callout 至少要有一个子块，空消息给一个空段落
    if children.is_empty() {
        children.push(empty_paragraph(ids));
    }
    for child in children.iter_mut() {
        sanitize(child);
    }

    // 这条消息是哪个模型产出的：callout 和它的每个子块都记上
    let model = event
        .get("data")
        .and_then(|data| data.get("message"))
        .and_then(|message| message.get("source"))
        .and_then(|source| source.get("model"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(model) = &model {
        for child in children.iter_mut() {
            stamp_model(child, model);
        }
    }

    let id = ids.next();
    let mut properties = Map::new();
    properties.insert("id".into(), json!(id));
    properties.insert(INDEX_ATTR.into(), json!(index.to_string()));
    if let Some(model) = &model {
        properties.insert(MODEL_ATTR.into(), json!(model));
    }
    if profile == Profile::Spin {
        // 原始事件挂在属性上：结构不变、顺序不变，还原时直接读它
        properties.insert(
            EVENT_ATTR.into(),
            json!(base64(event.to_string().as_bytes())),
        );
    }

    Some(json!({
        "ID": id,
        "Type": "NodeCallout",
        "CalloutType": callout_type,
        "CalloutTitle": title,
        "CalloutTitleExplicit": true,
        "CalloutIcon": icon,
        "Properties": properties,
        "Children": children,
    }))
}

/// 取事件里的 content 数组。
fn content_blocks(data: &Value) -> Vec<Value> {
    if let Some(Value::Array(items)) = data.get("content") {
        return items.clone();
    }
    if let Some(Value::Array(items)) = data.get("message").and_then(|message| message.get("content"))
    {
        return items.clone();
    }
    Vec::new()
}

/// 空段落：思源要有内容，前端也不显示空块。
fn empty_paragraph(ids: &mut IdGen) -> Value {
    let id = ids.next();
    let mut properties = Map::new();
    properties.insert("id".into(), json!(id));
    json!({
        "ID": id,
        "Type": "NodeParagraph",
        "Properties": properties,
        "Children": [{"Type": "NodeText"}],
    })
}

/// 思考过程渲染成引用块（内部仍是 markdown 结构）；空内容不产块。
fn reasoning_block(text: &str, ids: &mut IdGen) -> Option<Value> {
    let mut tree = md::parse(text);
    let mut children: Vec<Value> = Vec::new();
    children.push(json!({
        "Type": "NodeBlockquoteMarker",
        "Data": "> ",
        "Close": true,
    }));
    if let Some(items) = tree.get_mut("Children").and_then(Value::as_array_mut) {
        for item in items.iter_mut() {
            assign_ids(item, ids);
            children.push(item.clone());
        }
    }
    if children.len() < 2 {
        return None;
    }
    let id = ids.next();
    let mut properties = Map::new();
    properties.insert("id".into(), json!(id));
    Some(json!({
        "ID": id,
        "Type": "NodeBlockquote",
        "Properties": properties,
        "Children": children,
    }))
}

/// 去掉思源会丢弃或改写的块。
///
/// 除了空列表/空引用块，行内 HTML 也要降级成文本：dataparser 会把
/// NodeInlineHTML 强制转成 HTML 块（历史迁移逻辑），那会改变结构。
fn sanitize(node: &mut Value) {
    if let Value::Object(record) = node {
        if record.get("Type").and_then(Value::as_str) == Some("NodeInlineHTML") {
            let data = record.get("Data").cloned().unwrap_or_else(|| json!(""));
            record.insert("Type".into(), json!("NodeText"));
            record.insert("Data".into(), data);
        }
    }
    if let Some(children) = node.get_mut("Children").and_then(Value::as_array_mut) {
        for child in children.iter_mut() {
            sanitize(child);
        }
        children.retain(|child| {
            let node_type = child.get("Type").and_then(Value::as_str).unwrap_or("");
            let count = child
                .get("Children")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            match node_type {
                "NodeList" | "NodeListItem" => count > 0,
                "NodeBlockquote" => count >= 2,
                _ => true,
            }
        });
    }
}

/// 围栏代码块（思源要求四段结构：开标记、信息标记、内容、关标记）。
fn code_block(body: &str, language: &str, ids: &mut IdGen) -> Value {
    let fence: String = std::iter::repeat('\x60').take(3).collect();
    let id = ids.next();
    let mut properties = Map::new();
    properties.insert("id".into(), json!(id));
    json!({
        "ID": id,
        "Type": "NodeCodeBlock",
        "IsFencedCodeBlock": true,
        "CodeBlockFenceChar": 96,
        "CodeBlockFenceLen": 3,
        "CodeBlockOpenFence": base64(fence.as_bytes()),
        "CodeBlockInfo": base64(language.as_bytes()),
        "CodeBlockCloseFence": base64(fence.as_bytes()),
        "Properties": properties,
        "Children": [
            {"Type": "NodeCodeBlockFenceOpenMarker", "Data": fence, "CodeBlockFenceLen": 3},
            {"Type": "NodeCodeBlockFenceInfoMarker"},
            {"Type": "NodeCodeBlockCode", "Data": body},
            {"Type": "NodeCodeBlockFenceCloseMarker", "Data": fence, "CodeBlockFenceLen": 3},
        ],
    })
}

/// 工具调用渲染成代码块：工具名 + 完整参数（能格式化的 JSON 就展开）。
fn tool_block(block: &Value, ids: &mut IdGen) -> Value {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    let raw = block
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = match serde_json::from_str::<Value>(raw) {
        Ok(parsed) => serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| raw.to_owned()),
        Err(_) => raw.to_owned(),
    };
    let body = if arguments.is_empty() {
        name.to_owned()
    } else {
        format!("{name}\n{arguments}")
    };
    code_block(&body, "", ids)
}

/// 单行段落。
fn paragraph_text(text: &str, ids: &mut IdGen) -> Value {
    let id = ids.next();
    let mut properties = Map::new();
    properties.insert("id".into(), json!(id));
    json!({
        "ID": id,
        "Type": "NodeParagraph",
        "Properties": properties,
        "Children": [{"Type": "NodeText", "Data": text}],
    })
}

/// 图片块：附件从 DSH 的附件库复制进思源 assets，再按 NodeImage 结构引用。
fn image_block(
    block: &Value,
    index: usize,
    ids: &mut IdGen,
    assets: Option<&std::path::Path>,
) -> Option<Value> {
    let attachment = block.get("attachment")?;
    let attachment_id = attachment.get("attachmentId")?.as_str()?;
    let media_type = attachment
        .get("mediaType")
        .and_then(Value::as_str)
        .unwrap_or("image/png");
    let name = attachment
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("image");
    let extension = match media_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "png",
    };

    let digest = attachment_id.strip_prefix("sha256:").unwrap_or(attachment_id);
    let mut source = std::path::PathBuf::new();
    if let Ok(root) = std::env::var("USERPROFILE") {
        source = std::path::PathBuf::from(root)
            .join(".dsh")
            .join("attachments")
            .join("v1")
            .join("objects");
        if digest.len() >= 2 {
            source = source.join(&digest[..2]).join(digest);
        }
    }

    let id = ids.next();
    let file_name = format!("image-{}-{id}.{extension}", index);
    let mut reference = String::new();
    if let (Some(dir), true) = (assets, source.is_file()) {
        let target = dir.join(&file_name);
        // 附件是只读的，fs::copy 会连属性一起复制而报拒绝访问；读字节再写最稳
        match std::fs::read(&source) {
            Ok(bytes) => match std::fs::write(&target, &bytes) {
                Ok(_) => reference = format!("assets/{file_name}"),
                Err(error) => eprintln!("写入图片失败 {}: {error}", target.display()),
            },
            Err(error) => eprintln!("读取附件失败 {}: {error}", source.display()),
        }
    }
    if reference.is_empty() {
        // 落不到 assets 时至少留一句说明，不要静默吞掉
        return Some(paragraph_text(&format!("[图片未落地：{name}]"), ids));
    }

    let mut properties = Map::new();
    properties.insert("id".into(), json!(id));
    Some(json!({
        "ID": id,
        "Type": "NodeImage",
        "Properties": properties,
        "Children": [
            {"Type": "NodeOpenBracket"},
            {"Type": "NodeLinkText", "Data": name},
            {"Type": "NodeCloseBracket"},
            {"Type": "NodeOpenParen"},
            {"Type": "NodeLinkDest", "Data": reference},
            {"Type": "NodeCloseParen"},
        ],
    }))
}


/// 每个块自带的来源签名：谁导的、什么时候导的、从哪个会话、哪个预设。
///
/// 单独放在块上而不是文档上，是因为块以后会被搬走、被引用、被重排，
/// 离开本文档后仍要能说清自己从哪来。
// 通用部分：将来从别的软件导入也用这套键，值说明来源
pub const SOURCE_ATTR: &str = "custom-import-source";
pub const ARCHIVED_ATTR: &str = "custom-import-archived";
pub const SESSION_ATTR: &str = "custom-dsh-session";
// dsh 专有部分：只有来源确实是 dsh 时才加
pub const PRESET_ATTR: &str = "custom-dsh-preset";
/// 产出这块内容的模型（只有 LLM 输出才标）。
pub const MODEL_ATTR: &str = "custom-dsh-model";

/// 一次归档的签名。
struct Signature {
    source: String,
    archived: i64,
    session: String,
    preset: String,
}

impl Signature {
    fn of(session: &Session) -> Self {
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
fn stamp_model(node: &mut Value, model: &str) {
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
fn stamp(node: &mut Value, signature: &Signature) {
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
fn assign_ids(node: &mut Value, ids: &mut IdGen) {
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
