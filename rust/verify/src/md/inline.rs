//! 复刻 Lute 的行内解析：单遍扫描 + 分隔符栈。
//!
//! 对应 parse/inlines.go（parseInline 主循环）、parse/inline.go（分派）、
//! parse/text.go（文本、转义、换行）、parse/delimiter.go（强调算法）。
//! 只实现 CommonMark 规范用例覆盖到的部分；未涉及的分支（链接、图片、内联 HTML、
//! 公式等）留作后续，字符会被当作普通文本。

mod bracket;
mod emphasis;
mod html;
mod link;
mod text;

use serde_json::{json, Value};

pub(crate) use link::*;

/// 行内节点。用 arena + children 下标表示树。
struct Node {
    node_type: &'static str,
    data: Vec<u8>,
    children: Vec<usize>,
    attrs: Vec<(&'static str, Value)>,
}

struct Delim {
    typ: u8,
    num: i32,
    original_num: i32,
    unit_len: usize,
    node: usize,
    can_open: bool,
    can_close: bool,
    prev: Option<usize>,
    next: Option<usize>,
}

/// 对应 Lute 的 bracket（链接/图片的 [ 栈）。
#[derive(Clone)]
struct Bracket {
    node: usize,
    /// 左方括号在 tokens 里的位置，快捷引用要从原文取标签。
    index: usize,
    image: bool,
    active: bool,
    previous_delimiter: Option<usize>,
    bracket_after: bool,
}

struct Parser<'a> {
    tokens: &'a [u8],
    pos: usize,
    nodes: Vec<Node>,
    root: Vec<usize>,
    delims: Vec<Delim>,
    top: Option<usize>,
    brackets: Vec<Bracket>,
    defs: &'a std::collections::HashMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
}

/// 解析一段块级 tokens 的行内内容。
pub fn parse(
    tokens: &[u8],
    defs: &std::collections::HashMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
) -> Vec<Value> {
    let mut parser = Parser {
        tokens,
        pos: 0,
        nodes: Vec::new(),
        root: Vec::new(),
        delims: Vec::new(),
        top: None,
        brackets: Vec::new(),
        defs,
    };
    parser.run();
    parser.render()
}

impl<'a> Parser<'a> {
    fn new_node(&mut self, node_type: &'static str) -> usize {
        self.nodes.push(Node {
            node_type,
            data: Vec::new(),
            children: Vec::new(),
            attrs: Vec::new(),
        });
        self.nodes.len() - 1
    }

    fn append(&mut self, node: usize) {
        self.root.push(node);
    }

    fn run(&mut self) {
        while self.pos < self.tokens.len() {
            let token = self.tokens[self.pos];
            match token {
                b'\\' => self.parse_backslash(),
                b'\x60' => self.parse_code_span(),
                b'*' | b'_' => self.handle_delim(token),
                b'\n' => self.parse_newline(),
                b'[' => self.parse_open_bracket(),
                b']' => self.parse_close_bracket(),
                b'!' => self.parse_bang(),
                b'<' => {
                    // 依次尝试自动链接、邮件自动链接、行内 HTML，都不成立就是普通字符。
                    if !self.parse_autolink()
                        && !self.parse_email_autolink()
                        && !self.parse_inline_html()
                    {
                        self.pos += 1;
                        self.push_text(vec![b'<']);
                    }
                }
                b'&' => self.parse_entity(),
                _ => self.parse_text(),
            }
        }
        self.process_emphasis(None);
        self.merge_text();
    }

    /// 对应 parseText：一直吃到下一个潜在标记符。
    /// 对应 handleDelim + scanDelims。
    fn render(&self) -> Vec<Value> {
        self.root.iter().map(|index| self.render_node(*index)).collect()
    }

    fn render_node(&self, index: usize) -> Value {
        let node = &self.nodes[index];
        let mut object = serde_json::Map::new();
        object.insert("Type".into(), json!(node.node_type));
        if !node.children.is_empty() {
            let children: Vec<Value> = node.children.iter().map(|child| self.render_node(*child)).collect();
            object.insert("Children".into(), Value::Array(children));
        } else if !node.data.is_empty() {
            // 空内容不写 Data（对应 Go 的 omitempty）
            object.insert("Data".into(), json!(String::from_utf8_lossy(&node.data).to_string()));
        }
        for (key, value) in &node.attrs {
            object.insert((*key).to_string(), value.clone());
        }
        Value::Object(object)
    }
}

fn trim_right(tokens: &[u8]) -> Vec<u8> {
    let mut end = tokens.len();
    while end > 0 && matches!(tokens[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    tokens[..end].to_vec()
}

fn is_marker(token: u8) -> bool {
    matches!(
        token,
        b'*' | b'_' | b'[' | b'!' | b'\n' | b'\\' | b'\x60' | b'<' | b']' | b'&' | b'~' | b'$'
            | b'{' | b'(' | b'=' | b'#'
    )
}

/// 对应 lex.IsASCIIPunct。
fn is_ascii_punct(token: u8) -> bool {
    (0x21..=0x2f).contains(&token)
        || (0x3a..=0x40).contains(&token)
        || (0x5b..=0x60).contains(&token)
        || (0x7b..=0x7e).contains(&token)
}

/// 对应 scanDelims：判断分隔符能否开始/结束一个强调。
fn scan_delims(tokens: &[u8], start: usize, end: usize, typ: u8) -> (bool, bool) {
    let before = if start > 0 {
        last_char(&tokens[..start])
    } else {
        '\n'
    };
    let after = if end < tokens.len() {
        first_char(&tokens[end..])
    } else {
        '\n'
    };

    let after_is_whitespace = after.is_whitespace();
    let after_is_punct = is_punct(after);
    let before_is_whitespace = before.is_whitespace();
    let before_is_punct = is_punct(before);

    let is_left_flanking = !after_is_whitespace && (!after_is_punct || before_is_whitespace || before_is_punct);
    let is_right_flanking = !before_is_whitespace && (!before_is_punct || after_is_whitespace || after_is_punct);

    if typ == b'_' {
        (
            is_left_flanking && (!is_right_flanking || before_is_punct),
            is_right_flanking && (!is_left_flanking || after_is_punct),
        )
    } else {
        (is_left_flanking, is_right_flanking)
    }
}

fn first_char(bytes: &[u8]) -> char {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.chars().next().unwrap_or('\n'),
        Err(_) => bytes.first().map(|b| *b as char).unwrap_or('\n'),
    }
}

fn last_char(bytes: &[u8]) -> char {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.chars().next_back().unwrap_or('\n'),
        Err(_) => bytes.last().map(|b| *b as char).unwrap_or('\n'),
    }
}

/// 对应 unicode.IsPunct || unicode.IsSymbol 的近似实现。
fn is_punct(ch: char) -> bool {
    !ch.is_alphanumeric() && !ch.is_whitespace() && !ch.is_control()
}



/// 去掉标签两端空白并折成小写，作为引用定义的键。
pub(crate) fn normalize_label(label: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(label);
    let mut out = String::new();
    for ch in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(ch);
    }
    // Unicode 大小写折叠：ß 与 ẞ 都折成 ss（[SS] 能匹配 [ẞ]）
    out.to_lowercase().replace('ß', "ss").into_bytes()
}




impl<'a> Parser<'a> {
    /// 对应 newLink：尖括号链接与引用定义的节点形状（开括号不带数据）。
    fn new_link(&mut self, text: &[u8], dest: &[u8], title: Option<&[u8]>, link_type: i64) -> usize {
        let node = self.new_node("NodeLink");
        self.nodes[node].attrs.push(("LinkType", json!(link_type)));
        if link_type == 1 {
            self.nodes[node]
                .attrs
                .push(("LinkRefLabel", json!(base64(text))));
        }
        let open = self.new_node("NodeOpenBracket");
        let text_node = self.new_node("NodeLinkText");
        self.nodes[text_node].data = text.to_vec();
        let close = self.new_node("NodeCloseBracket");
        let paren = self.new_node("NodeOpenParen");
        let dest_node = self.new_node("NodeLinkDest");
        self.nodes[dest_node].data = dest.to_vec();
        let close_paren = self.new_node("NodeCloseParen");
        let mut children = vec![open, text_node, close, paren, dest_node];
        if let Some(title) = title {
            let title_node = self.new_node("NodeLinkTitle");
            self.nodes[title_node].data = title.to_vec();
            children.push(title_node);
        }
        children.push(close_paren);
        self.nodes[node].children = children;
        self.append(node);
        node
    }

    /// 对应 parseAutolink：尖括号里是不含空格的绝对 URI。
    fn parse_autolink(&mut self) -> bool {
        let start = self.pos;
        let mut scheme: Vec<u8> = Vec::new();
        let mut schemed = false;
        let mut dest: Vec<u8> = Vec::new();
        let mut i = start + 1;
        while i < self.tokens.len() && self.tokens[i] != b'>' {
            let token = self.tokens[i];
            if token == b' ' {
                return false;
            }
            dest.push(token);
            if !schemed {
                if token != b':' {
                    scheme.push(token);
                } else {
                    schemed = true;
                }
            }
            i += 1;
        }
        if !schemed || scheme.len() < 3 || i >= self.tokens.len() {
            return false;
        }
        self.pos = i + 1;
        let text = dest.clone();
        let dest = encode_destination(&dest);
        self.new_link(&text, &dest, None, 2);
        true
    }

    /// 对应 parseAutoEmailLink。
    fn parse_email_autolink(&mut self) -> bool {
        let start = self.pos;
        let mut dest: Vec<u8> = Vec::new();
        let mut i = start + 1;
        let mut at = false;
        while i < self.tokens.len() {
            let token = self.tokens[i];
            dest.push(token);
            i += 1;
            if token == b'@' {
                at = true;
                break;
            }
            if !is_email_atom(token) {
                return false;
            }
        }
        if !at || dest.len() < 2 {
            return false;
        }
        let mut closed = false;
        let mut domain_len = 0usize;
        while i < self.tokens.len() {
            let token = self.tokens[i];
            if token == b'>' {
                closed = true;
                break;
            }
            if !(token.is_ascii_alphanumeric() || token == b'-' || token == b'.') {
                return false;
            }
            dest.push(token);
            domain_len += 1;
            i += 1;
            if domain_len > 63 {
                return false;
            }
        }
        if !closed || domain_len == 0 {
            return false;
        }
        self.pos = i + 1;
        let text = dest.clone();
        let mut link_dest: Vec<u8> = b"mailto:".to_vec();
        link_dest.extend_from_slice(&text);
        self.new_link(&text, &link_dest, None, 2);
        true
    }
}

/// 对应 lex.IsWhitespace。
fn is_whitespace(token: u8) -> bool {
    matches!(token, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// 对应 lex.TrimWhitespace。
fn trim_whitespace(tokens: &[u8]) -> Vec<u8> {
    let mut start = 0usize;
    let mut end = tokens.len();
    while start < end && is_whitespace(tokens[start]) {
        start += 1;
    }
    while end > start && is_whitespace(tokens[end - 1]) {
        end -= 1;
    }
    tokens[start..end].to_vec()
}

fn find_sequence(tokens: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= tokens.len() || tokens.len() - from < needle.len() {
        return None;
    }
    (from..=tokens.len() - needle.len()).find(|start| &tokens[*start..*start + needle.len()] == needle)
}

fn is_email_atom(token: u8) -> bool {
    token.is_ascii_alphanumeric()
        || matches!(
            token,
            b'-' | b'.' | b'+' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'/' | b'='
                | b'?' | b'^' | b'_' | b'\x60' | b'{' | b'|' | b'}' | b'~'
        )
}

/// 对应 parseLinkRefDef：标签、冒号、目标、可选标题；标题可以另起一行。
///
/// 返回 (标签, 目标, 可选标题, 消耗的字节数)。
pub(crate) fn parse_link_ref_def(tokens: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Option<Vec<u8>>, usize)> {
    let (label, label_consumed) = scan_link_label(tokens, 0)?;
    if label.is_empty() {
        return None;
    }
    let mut i = label_consumed;
    if tokens.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    // 冒号之后跳过空白，最多允许一个换行
    let mut newlines = 0usize;
    while i < tokens.len() && is_whitespace(tokens[i]) {
        if tokens[i] == b'\n' {
            newlines += 1;
            if newlines > 1 {
                return None;
            }
        }
        i += 1;
    }
    let (dest, dest_consumed) = scan_link_dest(tokens, i)?;
    i = dest_consumed;
    // 目标之后：空白 + 可选标题
    let space_start = i;
    let mut space_newlines = 0usize;
    while i < tokens.len() && is_whitespace(tokens[i]) {
        if tokens[i] == b'\n' {
            space_newlines += 1;
            if space_newlines > 1 {
                break;
            }
        }
        i += 1;
    }
    let mut title: Option<Vec<u8>> = None;
    let title_start = i;
    let mut after_title = i;
    if i > space_start {
        if let Some((value, title_consumed)) = scan_link_title(tokens, i) {
            // 只有标题后面直接是行尾才算这个定义的标题
            let mut tail = title_consumed;
            while tail < tokens.len() && matches!(tokens[tail], b' ' | b'\t' | b'\r') {
                tail += 1;
            }
            if tail >= tokens.len() || tokens[tail] == b'\n' {
                title = Some(value);
                after_title = title_consumed;
            } else {
                // 标题后还有内容：标题仍属于定义，但剩余内容从标题处重新开始
                title = Some(value);
                after_title = title_start;
            }
        }
    }
    if title.is_some() && after_title == title_start {
        // 同一行上标题后还有内容 → 不是定义；有换行才按回退处理
        if space_newlines == 0 {
            return None;
        }
        let dest = encode_destination(&unescape_backslashes(&dest));
        return Some((label, dest, title, title_start));
    }
    if title.is_some() {
        i = after_title;
    } else {
        // 没有标题时，目标之后必须直接到行尾
        i = space_start;
        while i < tokens.len() && matches!(tokens[i], b' ' | b'\t' | b'\r') {
            i += 1;
        }
        if i < tokens.len() && tokens[i] != b'\n' {
            return None;
        }
    }
    while i < tokens.len() && matches!(tokens[i], b' ' | b'\t' | b'\r') {
        i += 1;
    }
    if i < tokens.len() && tokens[i] != b'\n' {
        return None;
    }
    let consumed = if i < tokens.len() { i + 1 } else { i };
    // 引用定义的目标同样要反转义并做百分号编码
    let dest = encode_destination(&unescape_backslashes(&dest));
    Some((label, dest, title, consumed))
}



