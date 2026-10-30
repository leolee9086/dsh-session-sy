//! 把解析结果序列化成 Lute 的 JSON 结构（对应 Lute 的 render/json_renderer.go）。

use std::fmt::Write as _;

use serde_json::{json, Value};

use super::inline;
use super::util::{
    base64, decode_marker, trim_trailing_blank, trim_trailing_blank_ref, write_i64,
    write_json_string,
};
use super::{ListData, Parser};

impl Parser {
    /// 序列化成 Lute 的 JSON 结构。
    pub(super) fn render(&self, index: usize) -> Value {
        let node = &self.nodes[index];
        match node.node_type {
            "NodeDocument" => {
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!("NodeDocument"));
                // 空文档不输出 Children（Go 的 omitempty）
                if !node.children.is_empty() {
                    let children: Vec<Value> =
                        node.children.iter().map(|child| self.render(*child)).collect();
                    object.insert("Children".into(), Value::Array(children));
                }
                Value::Object(object)
            }
            "NodeFootnotesDef" => {
                // 脚注定义：Data 是标签（^bar），FootnotesRefs 是引用列表
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!("NodeFootnotesDef"));
                object.insert(
                    "Data".into(),
                    json!(String::from_utf8_lossy(&node.tokens)),
                );
                if let Some(position) = self
                    .footnotes_defs
                    .iter()
                    .position(|(_, at)| *at == index)
                {
                    let refs = self.footnotes_refs.borrow();
                    if let Some(labels) = refs.get(position) {
                        if !labels.is_empty() {
                            let mut list = Vec::new();
                            for (order, label) in labels.iter().enumerate() {
                                // 第一次是序号，之后是 "序号:第几次"
                                let ref_id = if order == 0 {
                                    (position + 1).to_string()
                                } else {
                                    format!("{}:{}", position + 1, order + 1)
                                };
                                // Lute 没给这些元素设类型，所以 Type 是空字符串
                                list.push(json!({
                                    "Type": "",
                                    "FootnotesRefLabel": base64(label),
                                    "FootnotesRefId": ref_id,
                                }));
                            }
                            object.insert("FootnotesRefs".into(), Value::Array(list));
                        }
                    }
                }
                if !node.children.is_empty() {
                    let children: Vec<Value> =
                        node.children.iter().map(|child| self.render(*child)).collect();
                    object.insert("Children".into(), Value::Array(children));
                }
                Value::Object(object)
            }
            "NodeParagraph" => {
                let text = trim_trailing_blank(&node.tokens);
                // 任务列表标记在块级收尾时就已经摘成独立节点了，这里先渲染它们
                let mut children: Vec<Value> =
                    node.children.iter().map(|child| self.render(*child)).collect();
                children.extend(inline::parse(
                    &text,
                    &self.link_ref_defs,
                    &self.footnotes_defs,
                    &self.footnotes_used,
                    &self.options,
                    inline::BlockKind::Paragraph,
                ));
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!("NodeParagraph"));
                // 块级 IAL 合并后的 Properties 等附加字段
                for (key, value) in &node.attrs {
                    object.insert((*key).to_string(), value.clone());
                }
                object.insert("Children".into(), Value::Array(children));
                Value::Object(object)
            }
            "NodeTableCell" => {
                // 单元格和段落一样走行内解析；空单元格不写 Children。
                // 单元格自己也可能有孩子（摘出来的 span IAL），先渲染它们。
                let mut children: Vec<Value> =
                    node.children.iter().map(|child| self.render(*child)).collect();
                children.extend(inline::parse(
                    &node.tokens,
                    &self.link_ref_defs,
                    &self.footnotes_defs,
                    &self.footnotes_used,
                    &self.options,
                    inline::BlockKind::TableCell,
                ));
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!("NodeTableCell"));
                for (key, value) in &node.attrs {
                    object.insert((*key).to_string(), value.clone());
                }
                if !children.is_empty() {
                    object.insert("Children".into(), Value::Array(children));
                }
                Value::Object(object)
            }
            "NodeHeading" => {
                let level = self.attr_u64(index, "HeadingLevel").unwrap_or(1);
                let text = trim_trailing_blank(&node.tokens);
                let mut children = Vec::new();
                for child in &node.children {
                    children.push(self.render(*child));
                }
                children.extend(inline::parse(
                    &text,
                    &self.link_ref_defs,
                    &self.footnotes_defs,
                    &self.footnotes_used,
                    &self.options,
                    inline::BlockKind::Heading,
                ));
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
                let mut object = serde_json::Map::new();
                object.insert("Type".into(), json!("NodeBlockquote"));
                // 块级 IAL 合并后的 Properties 等附加字段
                for (key, value) in &node.attrs {
                    object.insert((*key).to_string(), value.clone());
                }
                object.insert("Children".into(), Value::Array(children));
                Value::Object(object)
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
                // 块级 IAL 合并进来的属性
                for (key, value) in &node.attrs {
                    object.insert((*key).to_string(), value.clone());
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
        // 块级 IAL 合并进来的属性（例如代码块后面跟一行 {: id="..."}）
        if let Some((_, properties)) = node.attrs.iter().find(|(key, _)| *key == "Properties") {
            object.insert("Properties".into(), properties.clone());
        }
        object.insert("Children".into(), Value::Array(children));
        Value::Object(object)
    }
}

/// 直写 ListData，不构造中间的 serde_json::Value。
///
/// 列表项是文档里最常见的块之一，每项都要输出一组固定的数字字段；
/// 走 Value 的话，一次渲染就是一次 Map 分配加十几次 String 分配。
fn write_list_data(out: &mut String, data: &ListData) {
    out.push('{');
    let mut first = true;
    macro_rules! sep {
        () => {
            if !first {
                out.push(',');
            }
            first = false;
        };
    }
    if data.tight {
        sep!();
        out.push_str("\"Tight\":true");
    }
    if data.typ != 0 {
        sep!();
        out.push_str("\"Typ\":");
        write_i64(out, data.typ);
    }
    // 这三个字段各自独立、各自 omitempty：无序列表有 BulletChar，
    // 有序列表有 Start 与 Delimiter，无序任务列表两者都有。
    if data.bullet_char != 0 {
        sep!();
        out.push_str("\"BulletChar\":");
        write_i64(out, data.bullet_char as i64);
    }
    if data.start != 0 {
        sep!();
        out.push_str("\"Start\":");
        write_i64(out, data.start);
    }
    if data.delimiter != 0 {
        sep!();
        out.push_str("\"Delimiter\":");
        write_i64(out, data.delimiter as i64);
    }
    sep!();
    out.push_str("\"Padding\":");
    write_i64(out, data.padding as i64);
    if data.marker_offset > 0 {
        sep!();
        out.push_str("\"MarkerOffset\":");
        write_i64(out, data.marker_offset as i64);
    }
    if data.checked == Some(true) {
        sep!();
        out.push_str("\"Checked\":true");
    }
    sep!();
    out.push_str("\"Marker\":");
    write_json_string(out, &data.marker);
    if data.num != 0 {
        // 最后一个字段，不必再更新 first
        if !first {
            out.push(',');
        }
        out.push_str("\"Num\":");
        write_i64(out, data.num);
    }
    out.push('}');
}

fn render_list_data(data: &ListData) -> Value {
    let mut object = serde_json::Map::new();
    if data.tight {
        object.insert("Tight".into(), json!(true));
    }
    if data.typ != 0 {
        object.insert("Typ".into(), json!(data.typ));
    }
    // 这三个字段各自独立、各自 omitempty：无序列表有 BulletChar，
    // 有序列表有 Start 与 Delimiter，无序任务列表两者都有。
    if data.bullet_char != 0 {
        object.insert("BulletChar".into(), json!(data.bullet_char));
    }
    if data.start != 0 {
        object.insert("Start".into(), json!(data.start));
    }
    if data.delimiter != 0 {
        object.insert("Delimiter".into(), json!(data.delimiter));
    }
    object.insert("Padding".into(), json!(data.padding));
    if data.marker_offset > 0 {
        object.insert("MarkerOffset".into(), json!(data.marker_offset));
    }
    // Checked 只在勾选时出现（Go 的零值 omitempty）
    if data.checked == Some(true) {
        object.insert("Checked".into(), json!(true));
    }
    object.insert("Marker".into(), json!(data.marker));
    // Num 为 0 时省略（Go 的零值 omitempty）
    if data.num != 0 {
        object.insert("Num".into(), json!(data.num));
    }
    Value::Object(object)
}

/// 直出路径：块级节点直接写 JSON 文本，不构造 Value 树。
///
/// 和上面 `render()` 一一对应，语义必须完全一致——测试用
/// `from_str(parse_to_json(md)) == parse(md)` 交叉验证。
impl Parser {
    pub(super) fn write_json(&self, index: usize, out: &mut String) {
        let node = &self.nodes[index];
        match node.node_type {
            "NodeDocument" => {
                out.push_str("{\"Type\":\"NodeDocument\"");
                // 空文档不输出 Children（Go 的 omitempty）
                if !node.children.is_empty() {
                    out.push_str(",\"Children\":[");
                    let mut first = true;
                    self.write_children_flat(index, out, &mut first);
                    out.push(']');
                }
                out.push('}');
            }
            "NodeFootnotesDef" => {
                // 脚注定义：Data 是标签，FootnotesRefs 是引用列表
                out.push_str("{\"Type\":\"NodeFootnotesDef\",\"Data\":");
                write_json_string(out, &String::from_utf8_lossy(&node.tokens));
                if let Some(position) = self
                    .footnotes_defs
                    .iter()
                    .position(|(_, at)| *at == index)
                {
                    let refs = self.footnotes_refs.borrow();
                    if let Some(labels) = refs.get(position) {
                        if !labels.is_empty() {
                            out.push_str(",\"FootnotesRefs\":[");
                            for (order, label) in labels.iter().enumerate() {
                                if order > 0 {
                                    out.push(',');
                                }
                                // 第一次是序号，之后是 "序号:第几次"
                                let ref_id = if order == 0 {
                                    (position + 1).to_string()
                                } else {
                                    format!("{}:{}", position + 1, order + 1)
                                };
                                // Lute 没给这些元素设类型，所以 Type 是空字符串
                                out.push_str("{\"Type\":\"\",\"FootnotesRefLabel\":");
                                write_json_string(out, &base64(label));
                                out.push_str(",\"FootnotesRefId\":");
                                write_json_string(out, &ref_id);
                                out.push('}');
                            }
                            out.push(']');
                        }
                    }
                }
                if !node.children.is_empty() {
                    out.push_str(",\"Children\":[");
                    let mut first = true;
                    self.write_children_flat(index, out, &mut first);
                    out.push(']');
                }
                out.push('}');
            }
            "NodeParagraph" => {
                let text = trim_trailing_blank_ref(&node.tokens);
                out.push_str("{\"Type\":\"NodeParagraph\"");
                // 块级 IAL 合并后的 Properties 等附加字段
                for (key, value) in &node.attrs {
                    out.push(',');
                    write_json_string(out, key);
                    out.push(':');
                    let _ = write!(out, "{}", value);
                }
                out.push_str(",\"Children\":[");
                let mut first = true;
                // 任务列表标记在块级收尾时就已经摘成独立节点了，这里先写它们
                self.write_children_flat(index, out, &mut first);
                inline::parse_to_json_flat(
                    &text,
                    &self.link_ref_defs,
                    &self.footnotes_defs,
                    &self.footnotes_used,
                    out,
                    &mut first,
                    &self.options,
                    inline::BlockKind::Paragraph,
                );
                out.push_str("]}");
            }
            "NodeTableCell" => {
                // 直出路径的单元格：先把行内内容写进临时串，再决定要不要 Children
                out.push_str("{\"Type\":\"NodeTableCell\"");
                for (key, value) in &node.attrs {
                    out.push(',');
                    write_json_string(out, key);
                    out.push(':');
                    let _ = write!(out, "{}", value);
                }
                // 单元格自己也可能有孩子（摘出来的 span IAL），先写它们再接行内内容
                let mut children = String::from("[");
                let mut first = true;
                self.write_children_flat(index, &mut children, &mut first);
                inline::parse_to_json_flat(
                    &node.tokens,
                    &self.link_ref_defs,
                    &self.footnotes_defs,
                    &self.footnotes_used,
                    &mut children,
                    &mut first,
                    &self.options,
                    inline::BlockKind::TableCell,
                );
                children.push(']');
                if children != "[]" {
                    out.push_str(",\"Children\":");
                    out.push_str(&children);
                }
                out.push('}');
            }
            "NodeHeading" => {
                let level = self.attr_u64(index, "HeadingLevel").unwrap_or(1);
                let text = trim_trailing_blank_ref(&node.tokens);
                out.push_str("{\"Type\":\"NodeHeading\",\"HeadingLevel\":");
                let _ = write!(out, "{}", level);
                for (key, value) in &node.attrs {
                    if *key == "HeadingLevel" {
                        continue;
                    }
                    out.push(',');
                    write_json_string(out, key);
                    out.push(':');
                    let _ = write!(out, "{}", value);
                }
                out.push_str(",\"Children\":[");
                let mut first = true;
                self.write_children_flat(index, out, &mut first);
                // 标题的行内节点是平铺进 Children 的，不是嵌套数组
                inline::parse_to_json_flat(
                    &text,
                    &self.link_ref_defs,
                    &self.footnotes_defs,
                    &self.footnotes_used,
                    out,
                    &mut first,
                    &self.options,
                    inline::BlockKind::Heading,
                );
                out.push_str("]}");
            }
            "NodeThematicBreak" => out.push_str("{\"Type\":\"NodeThematicBreak\"}"),
            "NodeBlockquote" => {
                out.push_str("{\"Type\":\"NodeBlockquote\"");
                for (key, value) in &node.attrs {
                    out.push(',');
                    write_json_string(out, key);
                    out.push(':');
                    let _ = write!(out, "{}", value);
                }
                out.push_str(",\"Children\":[");
                let mut first = true;
                self.write_children_flat(index, out, &mut first);
                out.push_str("]}");
            }
            "NodeCodeBlock" => self.write_code_block_json(index, out),
            "NodeList" | "NodeListItem" => {
                out.push_str("{\"Type\":");
                write_json_string(out, node.node_type);
                if node.node_type == "NodeListItem" {
                    let marker = node
                        .list_data
                        .as_ref()
                        .map(|data| decode_marker(&data.marker))
                        .unwrap_or_default();
                    out.push_str(",\"Data\":");
                    write_json_string(out, &marker);
                }
                if let Some(data) = &node.list_data {
                    out.push_str(",\"ListData\":");
                    write_list_data(out, data);
                }
                // 块级 IAL 合并进来的属性
                for (key, value) in &node.attrs {
                    out.push(',');
                    write_json_string(out, key);
                    out.push(':');
                    let _ = write!(out, "{}", value);
                }
                if !node.children.is_empty() {
                    out.push_str(",\"Children\":[");
                    let mut first = true;
                    self.write_children_flat(index, out, &mut first);
                    out.push(']');
                }
                out.push('}');
            }
            "NodeHeadingC8hMarker" | "NodeBlockquoteMarker" => {
                out.push_str("{\"Type\":");
                write_json_string(out, node.node_type);
                out.push_str(",\"Data\":");
                write_json_string(out, &String::from_utf8_lossy(&node.tokens));
                out.push('}');
            }
            _ => {
                out.push_str("{\"Type\":");
                write_json_string(out, node.node_type);
                if !node.children.is_empty() {
                    out.push_str(",\"Children\":[");
                    let mut first = true;
                    self.write_children_flat(index, out, &mut first);
                    out.push(']');
                }
                if !node.tokens.is_empty() {
                    out.push_str(",\"Data\":");
                    write_json_string(out, &String::from_utf8_lossy(&node.tokens));
                }
                for (key, value) in &node.attrs {
                    out.push(',');
                    write_json_string(out, key);
                    out.push(':');
                    let _ = write!(out, "{}", value);
                }
                out.push('}');
            }
        }
    }

    fn write_children_flat(&self, index: usize, out: &mut String, first: &mut bool) {
        for child in &self.nodes[index].children {
            if !*first {
                out.push(',');
            }
            *first = false;
            self.write_json(*child, out);
        }
    }

    fn write_code_block_json(&self, index: usize, out: &mut String) {
        let node = &self.nodes[index];
        let fenced = node.attrs.iter().any(|(key, _)| *key == "IsFencedCodeBlock");
        let text = String::from_utf8_lossy(&node.tokens);
        let code_is_empty = text.is_empty();

        out.push_str("{\"Type\":\"NodeCodeBlock\"");
        let mut fence = String::new();
        let mut fence_len = 0u64;
        if fenced {
            let ch = self.attr_u64(index, "FenceChar").unwrap_or(96) as u8;
            fence_len = self.attr_u64(index, "FenceLen").unwrap_or(3);
            fence = std::iter::repeat(ch as char).take(fence_len as usize).collect();
            out.push_str(",\"IsFencedCodeBlock\":true,\"CodeBlockFenceChar\":");
            let _ = write!(out, "{}", ch);
            out.push_str(",\"CodeBlockFenceLen\":");
            let _ = write!(out, "{}", fence_len);
            if let Some(offset) = self.attr_u64(index, "FenceOffset") {
                if offset > 0 {
                    out.push_str(",\"CodeBlockFenceOffset\":");
                    let _ = write!(out, "{}", offset);
                }
            }
            if let Some((_, open)) = node.attrs.iter().find(|(key, _)| *key == "OpenFence") {
                out.push_str(",\"CodeBlockOpenFence\":");
                let _ = write!(out, "{}", open);
            }
            if let Some((_, info)) = node.attrs.iter().find(|(key, _)| *key == "Info") {
                out.push_str(",\"CodeBlockInfo\":");
                let _ = write!(out, "{}", info);
            }
        }

        // 闭合围栏：没记录就用开围栏本身（Lute 的行为）
        let close_value = node
            .attrs
            .iter()
            .find(|(key, _)| *key == "CloseFence")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| json!(base64(fence.as_bytes())));
        let close_text = close_value
            .as_str()
            .map(decode_marker)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| fence.clone());
        if fenced {
            out.push_str(",\"CodeBlockCloseFence\":");
            let _ = write!(out, "{}", close_value);
        }

        // 块级 IAL 合并进来的属性（例如代码块后面跟一行 {: id="..."}）
        if let Some((_, properties)) = node.attrs.iter().find(|(key, _)| *key == "Properties") {
            out.push_str(",\"Properties\":");
            let _ = write!(out, "{}", properties);
        }

        out.push_str(",\"Children\":[");
        let mut first = true;
        macro_rules! sep {
            () => {
                if !first {
                    out.push(',');
                }
                first = false;
            };
        }
        if fenced {
            sep!();
            out.push_str("{\"Type\":\"NodeCodeBlockFenceOpenMarker\",\"Data\":");
            write_json_string(out, &fence);
            out.push_str(",\"CodeBlockFenceLen\":");
            let _ = write!(out, "{}", fence_len);
            out.push('}');

            sep!();
            if let Some((_, info)) = node.attrs.iter().find(|(key, _)| *key == "Info") {
                out.push_str("{\"Type\":\"NodeCodeBlockFenceInfoMarker\",\"CodeBlockInfo\":");
                let _ = write!(out, "{}", info);
                out.push('}');
            } else {
                out.push_str("{\"Type\":\"NodeCodeBlockFenceInfoMarker\"}");
            }
        }

        sep!();
        if code_is_empty {
            out.push_str("{\"Type\":\"NodeCodeBlockCode\"}");
        } else {
            out.push_str("{\"Type\":\"NodeCodeBlockCode\",\"Data\":");
            write_json_string(out, &text);
            out.push('}');
        }

        if fenced {
            // 最后一个子节点，不必再更新 first
            if !first {
                out.push(',');
            }
            out.push_str("{\"Type\":\"NodeCodeBlockFenceCloseMarker\",\"Data\":");
            write_json_string(out, &close_text);
            out.push_str(",\"CodeBlockFenceLen\":");
            let _ = write!(out, "{}", fence_len);
            out.push('}');
        }
        out.push_str("]}");
    }
}

