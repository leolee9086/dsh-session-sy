//! 行内 HTML 与字符实体（对应 Lute 的 inline_html.go / entity.go）。

use serde_json::json;

use super::link::base64;
use super::{find_sequence, is_whitespace, Parser};

impl<'a> Parser<'a> {
    /// 对应 parseInlineHtml：注释、CDATA、处理指令、声明与完整的开/闭标签。
    pub(super) fn parse_inline_html(&mut self) -> bool {
        let tokens = self.tokens;
        let start = self.pos;
        if start + 1 >= tokens.len() {
            return false;
        }
        let mut end: Option<usize> = None;
        if tokens[start + 1] == b'/' {
            if let Some((next, name)) = parse_tag_name(tokens, start + 2) {
                if !name.is_empty() {
                    // 闭标签后面只允许空白和 >
                    let mut j = next;
                    while j < tokens.len() && is_whitespace(tokens[j]) {
                        j += 1;
                    }
                    if tokens.get(j) == Some(&b'>') {
                        end = Some(j + 1);
                    }
                }
            }
        } else if tokens[start + 1..].starts_with(b"!--") {
            // Lute 的注释以第一个 > 结束（<!--> 也算完整的行内 HTML）
            end = find_sequence(tokens, start + 4, b">").map(|at| at + 1);
        } else if tokens[start + 1..].starts_with(b"![CDATA[") {
            end = find_sequence(tokens, start + 9, b"]]>").map(|at| at + 3);
        } else if tokens[start + 1] == b'?' {
            end = find_sequence(tokens, start + 2, b"?>").map(|at| at + 2);
        } else if tokens[start + 1] == b'!' {
            end = find_sequence(tokens, start + 2, b">").map(|at| at + 1);
        } else if let Some((mut next, name)) = parse_tag_name(tokens, start + 1) {
            if !name.is_empty() {
                let mut valid = true;
                loop {
                    let (ok, after, attr) = tag_attr(tokens, next);
                    if !ok {
                        valid = false;
                        break;
                    }
                    next = after;
                    if attr.is_empty() {
                        break;
                    }
                }
                if valid {
                    // 属性解析停下的位置可能还有空白，先跳过再判断收尾
                    let mut tail = next;
                    while tail < tokens.len() && is_whitespace(tokens[tail]) {
                        tail += 1;
                    }
                    if tokens.get(tail) == Some(&b'>') {
                        end = Some(tail + 1);
                    } else if tokens.get(tail) == Some(&b'/') && tokens.get(tail + 1) == Some(&b'>') {
                        end = Some(tail + 2);
                    }
                }
            }
        }
        let Some(end) = end else {
            return false;
        };
        let text = tokens[start..end].to_vec();
        self.pos = end;
        let node = self.new_node("NodeInlineHTML");
        self.nodes[node].data = text;
        self.append(node);
        true
    }

    /// 对应 parseEntity：命名实体与数字字符引用。
    pub(super) fn parse_entity(&mut self) {
        let start = self.pos;
        let tokens = self.tokens;
        if tokens.len() < 2 || start + 1 >= tokens.len() {
            self.pos += 1;
            self.push_text(vec![b'&']);
            return;
        }
        let numeric = tokens[start + 1] == b'#';
        let mut i = start + 1;
        let mut closed = false;
        while i < tokens.len() {
            let token = tokens[i];
            if token.is_ascii_whitespace() {
                break;
            }
            if token == b';' {
                i += 1;
                closed = true;
                break;
            }
            i += 1;
        }
        if !closed {
            self.pos += 1;
            self.push_text(vec![b'&']);
            return;
        }
        let entity = String::from_utf8_lossy(&tokens[start..i]).to_string();
        if numeric {
            let body_len = entity.len();
            if !(4..=10).contains(&body_len) {
                self.pos += 1;
                self.push_text(vec![b'&']);
                return;
            }
            if body_len >= 3 && (entity.as_bytes()[2] == b'x' || entity.as_bytes()[2] == b'X') && body_len < 5 {
                self.pos += 1;
                self.push_text(vec![b'&']);
                return;
            }
        }
        let mut decoded = html_escape::decode_html_entities(&entity).to_string();
        if decoded == entity {
            // 无效的数字实体（如 &#0;）要落成替换字符
            let code = if entity.starts_with("&#x") || entity.starts_with("&#X") {
                u32::from_str_radix(&entity[3..entity.len().saturating_sub(1)], 16).ok()
            } else if entity.starts_with("&#") {
                entity[2..entity.len().saturating_sub(1)].parse::<u32>().ok()
            } else {
                None
            };
            match code {
                Some(value) if value == 0 || value > 0x10FFFF || (0xD800..=0xDFFF).contains(&value) => {
                    decoded = "\u{FFFD}".to_string();
                }
                _ => {
                    self.pos += 1;
                    self.push_text(vec![b'&']);
                    return;
                }
            }
        }
        // 空字符实体解出来的 NUL 要换成替换字符（Lute 的 html 包行为）
        if decoded.contains('\u{0}') {
            decoded = "\u{FFFD}".to_string();
        }
        self.pos = i;
        let node = self.new_node("NodeHTMLEntity");
        self.nodes[node].data = decoded.into_bytes();
        self.nodes[node]
            .attrs
            .push(("HtmlEntityTokens", json!(base64(entity.as_bytes()))));
        self.append(node);
    }
}

/// 对应 parseTagName。
fn parse_tag_name(tokens: &[u8], from: usize) -> Option<(usize, Vec<u8>)> {
    if from >= tokens.len() || !tokens[from].is_ascii_alphabetic() {
        return None;
    }
    let mut name: Vec<u8> = vec![tokens[from]];
    let mut i = from + 1;
    while i < tokens.len() {
        let token = tokens[i];
        if !(token.is_ascii_alphanumeric() || token == b'-') {
            break;
        }
        name.push(token);
        i += 1;
    }
    Some((i, name))
}

/// 对应 TagAttr：返回 (是否合法, 新位置, 属性原文)。
fn tag_attr(tokens: &[u8], from: usize) -> (bool, usize, Vec<u8>) {
    let mut i = from;
    let mut whitespace: Vec<u8> = Vec::new();
    while i < tokens.len() && is_whitespace(tokens[i]) {
        whitespace.push(tokens[i]);
        i += 1;
    }
    if whitespace.is_empty() {
        return (true, from, Vec::new());
    }
    let Some((after_name, name)) = parse_attr_name(tokens, i) else {
        return (true, from, Vec::new());
    };
    i = after_name;
    let (valid, after_value, value_spec) = parse_attr_val_spec(tokens, i);
    if !valid {
        return (false, from, Vec::new());
    }
    i = after_value;
    let mut attr = whitespace;
    attr.extend_from_slice(&name);
    attr.extend_from_slice(&value_spec);
    (true, i, attr)
}

/// 对应 parseAttrName。
fn parse_attr_name(tokens: &[u8], from: usize) -> Option<(usize, Vec<u8>)> {
    if from >= tokens.len() {
        return None;
    }
    let first = tokens[from];
    if !first.is_ascii_alphabetic() && first != b'_' && first != b':' {
        return None;
    }
    let mut name: Vec<u8> = vec![first];
    let mut i = from + 1;
    while i < tokens.len() {
        let token = tokens[i];
        if !(token.is_ascii_alphanumeric() || token == b'-' || token == b'_' || token == b'.' || token == b':') {
            break;
        }
        name.push(token);
        i += 1;
    }
    Some((i, name))
}

/// 对应 parseAttrValSpec：等号加引号值或无引号值。
fn parse_attr_val_spec(tokens: &[u8], from: usize) -> (bool, usize, Vec<u8>) {
    let mut i = from;
    let mut spec: Vec<u8> = Vec::new();
    while i < tokens.len() && is_whitespace(tokens[i]) {
        spec.push(tokens[i]);
        i += 1;
    }
    if i >= tokens.len() || tokens[i] != b'=' {
        return (true, from, Vec::new());
    }
    spec.push(tokens[i]);
    i += 1;
    if i >= tokens.len() {
        return (false, from, Vec::new());
    }
    while i < tokens.len() && is_whitespace(tokens[i]) {
        spec.push(tokens[i]);
        i += 1;
    }
    if i >= tokens.len() {
        return (false, from, Vec::new());
    }
    let opener = tokens[i];
    spec.push(opener);
    i += 1;
    if opener == b'"' || opener == b'\'' {
        let mut closed = false;
        while i < tokens.len() {
            let token = tokens[i];
            spec.push(token);
            i += 1;
            if token == opener {
                closed = true;
                break;
            }
        }
        if !closed {
            return (false, from, Vec::new());
        }
        return (true, i, spec);
    }
    let mut closed = false;
    while i < tokens.len() {
        let token = tokens[i];
        if token == b'>' {
            break;
        }
        spec.push(token);
        i += 1;
        if is_whitespace(token) {
            break;
        }
        if matches!(token, b'"' | b'\'' | b'=' | b'<' | b'\x60') {
            closed = false;
            break;
        }
        closed = true;
    }
    if !closed {
        return (false, from, Vec::new());
    }
    (true, i, spec)
}
