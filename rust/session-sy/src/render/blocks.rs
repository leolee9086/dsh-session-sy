//! 事件 → 思源块：callout 组装、代码块、图片、以及导入时的结构清理。

use serde_json::{json, Map, Value};

use super::stamp::{assign_ids, stamp_model};
use super::{base64, Profile, EVENT_ATTR, INDEX_ATTR, MODEL_ATTR};
use crate::md;
use crate::model::IdGen;

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
pub(super) fn message_callout(
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
        if profile == Profile::Spin {
            // 图片子调用同样是事件：不带它，这份 spin 文档就还原不回去
            properties.insert(
                EVENT_ATTR.into(),
                json!(base64(event.to_string().as_bytes())),
            );
        }
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
pub(super) fn sanitize(node: &mut Value) {
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
pub(super) fn code_block(body: &str, language: &str, ids: &mut IdGen) -> Value {
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
