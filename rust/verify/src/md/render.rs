//! 把解析结果序列化成 Lute 的 JSON 结构（对应 Lute 的 render/json_renderer.go）。

use serde_json::{json, Value};

use super::inline;
use super::util::{base64, decode_marker, trim_trailing_blank};
use super::{ListData, Parser};

impl Parser {
    /// 序列化成 Lute 的 JSON 结构。
    pub(super) fn render(&self, index: usize) -> Value {
        let node = &self.nodes[index];
        match node.node_type {
            "NodeDocument" => {
                let children: Vec<Value> =
                    node.children.iter().map(|child| self.render(*child)).collect();
                json!({"Type": "NodeDocument", "Children": children})
            }
            "NodeParagraph" => {
                let text = trim_trailing_blank(&node.tokens);
                let children = inline::parse(&text, &self.link_ref_defs);
                json!({"Type": "NodeParagraph", "Children": children})
            }
            "NodeHeading" => {
                let level = self.attr_u64(index, "HeadingLevel").unwrap_or(1);
                let text = trim_trailing_blank(&node.tokens);
                let mut children = Vec::new();
                for child in &node.children {
                    children.push(self.render(*child));
                }
                children.extend(inline::parse(&text, &self.link_ref_defs));
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!("NodeHeading"));
                object.insert("HeadingLevel".into(), json!(level));
                // Setext 标题会额外带 HeadingSetext，必须原样输出
                for (key, value) in &node.attrs {
                    if *key == "HeadingLevel" {
                        continue;
                    }
                    object.insert((*key).to_string(), value.clone());
                }
                object.insert("Children".into(), Value::Array(children));
                Value::Object(object)
            }
            "NodeThematicBreak" => json!({"Type": "NodeThematicBreak"}),
            "NodeBlockquote" => {
                let children: Vec<Value> =
                    node.children.iter().map(|child| self.render(*child)).collect();
                json!({"Type": "NodeBlockquote", "Children": children})
            }
            "NodeCodeBlock" => self.render_code_block(index),
            "NodeList" | "NodeListItem" => {
                let children: Vec<Value> =
                    node.children.iter().map(|child| self.render(*child)).collect();
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!(node.node_type));
                if node.node_type == "NodeListItem" {
                    let marker = node
                        .list_data
                        .as_ref()
                        .map(|data| decode_marker(&data.marker))
                        .unwrap_or_default();
                    object.insert("Data".into(), json!(marker));
                }
                if let Some(data) = &node.list_data {
                    object.insert("ListData".into(), render_list_data(data));
                }
                // 空的列表项不写 Children 字段（期望是直接省略）
                if !children.is_empty() {
                    object.insert("Children".into(), Value::Array(children));
                }
                Value::Object(object)
            }
            "NodeHeadingC8hMarker" | "NodeBlockquoteMarker" => {
                json!({
                    "Type": node.node_type,
                    "Data": String::from_utf8_lossy(&node.tokens).to_string(),
                })
            }
            _ => {
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!(node.node_type));
                if !node.children.is_empty() {
                    let children: Vec<Value> =
                        node.children.iter().map(|child| self.render(*child)).collect();
                    object.insert("Children".into(), Value::Array(children));
                }
                if !node.tokens.is_empty() {
                    object.insert(
                        "Data".into(),
                        json!(String::from_utf8_lossy(&node.tokens).to_string()),
                    );
                }
                for (key, value) in &node.attrs {
                    object.insert((*key).to_string(), value.clone());
                }
                Value::Object(object)
            }
        }
    }

    fn render_code_block(&self, index: usize) -> Value {
        let node = &self.nodes[index];
        let fenced = node
            .attrs
            .iter()
            .any(|(key, _)| *key == "IsFencedCodeBlock");
        let text = String::from_utf8_lossy(&node.tokens).to_string();
        // 空代码块不输出 Data（期望是只有 Type 的对象）
        let code = if text.is_empty() {
            json!({"Type": "NodeCodeBlockCode"})
        } else {
            json!({"Type": "NodeCodeBlockCode", "Data": text})
        };
        let mut object = serde_json::Map::new();
        object.insert("Type".into(), json!("NodeCodeBlock"));
        let mut children = Vec::new();
        if fenced {
            let ch = self.attr_u64(index, "FenceChar").unwrap_or(96) as u8;
            let len = self.attr_u64(index, "FenceLen").unwrap_or(3);
            let fence: String = std::iter::repeat(ch as char).take(len as usize).collect();
            object.insert("IsFencedCodeBlock".into(), json!(true));
            object.insert("CodeBlockFenceChar".into(), json!(ch));
            object.insert("CodeBlockFenceLen".into(), json!(len));
            // 围栏相对缩进只在非零时出现
            if let Some(offset) = self.attr_u64(index, "FenceOffset") {
                if offset > 0 {
                    object.insert("CodeBlockFenceOffset".into(), json!(offset));
                }
            }
            if let Some((_, open)) = node.attrs.iter().find(|(key, _)| *key == "OpenFence") {
                object.insert("CodeBlockOpenFence".into(), open.clone());
            }
            if let Some((_, info)) = node.attrs.iter().find(|(key, _)| *key == "Info") {
                object.insert("CodeBlockInfo".into(), info.clone());
                children.push(json!({"Type": "NodeCodeBlockFenceOpenMarker", "Data": fence, "CodeBlockFenceLen": len}));
                children.push(json!({"Type": "NodeCodeBlockFenceInfoMarker", "CodeBlockInfo": info}));
            } else {
                children.push(json!({"Type": "NodeCodeBlockFenceOpenMarker", "Data": fence, "CodeBlockFenceLen": len}));
                children.push(json!({"Type": "NodeCodeBlockFenceInfoMarker"}));
            }
            children.push(code);
            let close = node
                .attrs
                .iter()
                .find(|(key, _)| *key == "CloseFence")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| json!(base64(fence.as_bytes())));
            object.insert("CodeBlockCloseFence".into(), close.clone());
            // 闭合标记用实际闭合的那串围栏，不是开始时的长度
            let close_text = close
                .as_str()
                .map(decode_marker)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| fence.clone());
            children.push(json!({"Type": "NodeCodeBlockFenceCloseMarker", "Data": close_text, "CodeBlockFenceLen": len}));
        } else {
            children.push(code);
        }
        object.insert("Children".into(), Value::Array(children));
        Value::Object(object)
    }
}

fn render_list_data(data: &ListData) -> Value {
    let mut object = serde_json::Map::new();
    if data.tight {
        object.insert("Tight".into(), json!(true));
    }
    if data.typ == 1 {
        object.insert("Typ".into(), json!(1));
        // Start 为 0 时同样省略
        if data.start != 0 {
            object.insert("Start".into(), json!(data.start));
        }
        object.insert("Delimiter".into(), json!(data.delimiter));
    } else {
        object.insert("BulletChar".into(), json!(data.bullet_char));
    }
    object.insert("Padding".into(), json!(data.padding));
    if data.marker_offset > 0 {
        object.insert("MarkerOffset".into(), json!(data.marker_offset));
    }
    object.insert("Marker".into(), json!(data.marker));
    // Num 为 0 时省略（Go 的零值 omitempty）
    if data.num != 0 {
        object.insert("Num".into(), json!(data.num));
    }
    Value::Object(object)
}
