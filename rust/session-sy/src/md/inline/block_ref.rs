//! 块引用 `((id "锚文本"))`，逐函数对照 Lute 的 `parse/block_ref.go`。

use super::Parser;

impl<'a> Parser<'a> {
    /// 对应 parseBlockRef：不是合法的块引用就退化成普通左括号。
    pub(super) fn parse_block_ref(&mut self) {
        if !self.options.block_ref {
            self.pos += 1;
            self.push_text(vec![b'(']);
            return;
        }
        let tokens = self.tokens[self.pos..].to_vec();
        if tokens.len() < 5 || tokens[0] != b'(' || tokens[1] != b'(' {
            self.pos += 1;
            self.push_text(vec![b'(']);
            return;
        }

        let mut cursor = 2usize;
        let mut id: Vec<u8> = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        let mut subtype = String::new();
        let mut matched = false;

        if let Some(skip) = spnl(&tokens[cursor..]) {
            cursor += skip;
            if let Some((passed, value, closed)) = parse_block_ref_id(&tokens[cursor..]) {
                cursor += passed;
                id = value;
                if closed {
                    matched = true;
                } else if let Some(skip2) = spnl(&tokens[cursor..]) {
                    // ID 后面跟空格，说明可能还有锚文本
                    cursor += skip2;
                    if tokens.get(cursor) == Some(&b')') && tokens.get(cursor + 1) == Some(&b')') {
                        cursor += 2;
                        matched = true;
                    } else if let Some((passed2, value2, sub)) =
                        parse_block_ref_text(&tokens[cursor..])
                    {
                        cursor += passed2;
                        text = value2;
                        subtype = sub;
                        if let Some(skip3) = spnl(&tokens[cursor..]) {
                            cursor += skip3;
                            if tokens.get(cursor) == Some(&b')')
                                && tokens.get(cursor + 1) == Some(&b')')
                            {
                                cursor += 2;
                                matched = true;
                            }
                        }
                    }
                }
            }
        }

        if !matched {
            self.pos += 1;
            self.push_text(vec![b'(']);
            return;
        }

        self.pos += cursor;
        let node = self.new_node("NodeBlockRef");
        let open1 = self.new_node("NodeOpenParen");
        let open2 = self.new_node("NodeOpenParen");
        let id_node = self.new_node("NodeBlockRefID");
        self.nodes[id_node].data = id;
        let mut children = vec![open1, open2, id_node];
        if !text.is_empty() {
            let space = self.new_node("NodeBlockRefSpace");
            // 双引号锚文本是静态文本，单引号是动态文本
            let text_type = if subtype == "d" {
                "NodeBlockRefDynamicText"
            } else {
                "NodeBlockRefText"
            };
            let text_node = self.new_node(text_type);
            self.nodes[text_node].data = text;
            children.push(space);
            children.push(text_node);
        }
        let close1 = self.new_node("NodeCloseParen");
        let close2 = self.new_node("NodeCloseParen");
        children.push(close1);
        children.push(close2);
        self.nodes[node].children = children;
        self.append(node);
    }
}

/// 对应 lex.Spnl：跳过空白，但换行不能超过一个。
fn spnl(tokens: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    let mut newlines = 0usize;
    while i < tokens.len() {
        match tokens[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => {
                newlines += 1;
                i += 1;
            }
            _ => break,
        }
    }
    if newlines > 1 {
        None
    } else {
        Some(i)
    }
}

/// 对应 parseBlockRefID：解析块 ID（`时间戳-随机串`），返回 (消耗字节数, ID, 是否已闭合)。
fn parse_block_ref_id(tokens: &[u8]) -> Option<(usize, Vec<u8>, bool)> {
    if tokens.is_empty() {
        return None;
    }
    let mut i = 0usize;
    while i < tokens.len() && is_ascii_letter_num_hyphen(tokens[i]) {
        i += 1;
    }
    let id = tokens[..i].to_vec();
    let remains = &tokens[i..];
    if remains.len() < 2 || !is_node_id_pattern(&id) {
        return None;
    }
    if remains[0] == b')' && remains[1] == b')' {
        return Some((i + 2, id, true));
    }
    if !remains[0].is_ascii_whitespace() {
        return None;
    }
    Some((i, id, false))
}

/// 对应 parseBlockRefText：锚文本用双引号或单引号包住。
fn parse_block_ref_text(tokens: &[u8]) -> Option<(usize, Vec<u8>, String)> {
    if tokens.is_empty() || tokens[0] == b'[' {
        return None;
    }
    let (passed, value, subtype) = if tokens[0] == b'"' {
        let (passed, value) = parse_quoted(tokens, b'"')?;
        (passed, value, "s".to_string())
    } else if tokens[0] == b'\'' {
        let (passed, value) = parse_quoted(tokens, b'\'')?;
        (passed, value, "d".to_string())
    } else {
        return None;
    };
    Some((passed, unescape_html(&value), subtype))
}

/// 解析一段被引号包住的文本，返回 (消耗字节数, 内容)。
fn parse_quoted(tokens: &[u8], quote: u8) -> Option<(usize, Vec<u8>)> {
    let mut i = 1usize;
    let mut value = Vec::new();
    while i < tokens.len() {
        if tokens[i] == quote {
            return Some((i + 1, value));
        }
        value.push(tokens[i]);
        i += 1;
    }
    None
}

/// 对应 html.UnescapeBytes 的最简形式：只处理常见实体。
fn unescape_html(tokens: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(tokens).to_string();
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    text.into_bytes()
}

/// 对应 lex.IsASCIILetterNumHyphen。
fn is_ascii_letter_num_hyphen(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// 对应 ast.IsNodeIDPattern：14 位数字 + 连字符 + 7 位小写字母数字。
fn is_node_id_pattern(id: &[u8]) -> bool {
    if id.len() != 22 {
        return false;
    }
    if id.iter().filter(|byte| **byte == b'-').count() != 1 {
        return false;
    }
    let Some(dash) = id.iter().position(|byte| *byte == b'-') else {
        return false;
    };
    let (time_part, rest) = id.split_at(dash);
    let random_part = &rest[1..];
    if time_part.len() != 14 || random_part.len() != 7 {
        return false;
    }
    if !time_part.iter().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    random_part
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}
