//! 复刻 Lute 的行内解析：单遍扫描 + 分隔符栈。
//!
//! 对应 parse/inlines.go（parseInline 主循环）、parse/inline.go（分派）、
//! parse/text.go（文本、转义、换行）、parse/delimiter.go（强调算法）。
//! 只实现 CommonMark 规范用例覆盖到的部分；未涉及的分支（链接、图片、内联 HTML、
//! 公式等）留作后续，字符会被当作普通文本。

mod auto_link;
mod autolink;
mod block_ref;
mod file_annotation_ref;
mod emoji;
mod bracket;
mod emphasis;
pub(crate) mod html;
mod link;
mod math;
mod text;

use std::fmt::Write as _;

use serde_json::{json, Value};

use super::util::write_json_string;
use super::Options;

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

/// 行内解析所在的块类型，某些语法只对特定块生效。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading,
    TableCell,
}

struct Parser<'a> {
    tokens: &'a [u8],
    /// 引擎配置，决定哪些行内语法启用。
    options: &'a Options,
    /// 当前块类型：HeadingID 只认标题，代码 span 的管道符处理只认表格单元格。
    kind: BlockKind,
    pos: usize,
    nodes: Vec<Node>,
    root: Vec<usize>,
    delims: Vec<Delim>,
    top: Option<usize>,
    brackets: Vec<Bracket>,
    defs: &'a std::collections::HashMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
    /// 脚注定义（标签 → 定义下标），识别 [^label] 用。
    footnotes: &'a [(Vec<u8>, usize)],
    /// 每个脚注定义已用次数，跨段落共享，用来算 FootnotesRefId。
    footnotes_used: &'a std::cell::RefCell<Vec<usize>>,
}

/// 解析一段块级 tokens 的行内内容。
pub fn parse(
    tokens: &[u8],
    defs: &std::collections::HashMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
    footnotes: &[(Vec<u8>, usize)],
    footnotes_used: &std::cell::RefCell<Vec<usize>>,
    options: &Options,
    kind: BlockKind,
) -> Vec<Value> {
    let mut parser = Parser {
        tokens,
        options,
        kind,
        pos: 0,
        nodes: Vec::new(),
        root: Vec::new(),
        delims: Vec::new(),
        top: None,
        brackets: Vec::new(),
        defs,
        footnotes,
        footnotes_used,
    };
    parser.run();
    if options.gfm_auto_link {
        parser.apply_auto_links();
    }
    if options.emoji {
        parser.apply_emoji();
    }
    parser.render()
}

/// 直出路径的平铺版本：把行内节点顺次写进外层的数组，不额外套方括号。
pub fn parse_to_json_flat(
    tokens: &[u8],
    defs: &std::collections::HashMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
    footnotes: &[(Vec<u8>, usize)],
    footnotes_used: &std::cell::RefCell<Vec<usize>>,
    out: &mut String,
    first: &mut bool,
    options: &Options,
    kind: BlockKind,
) {
    let mut parser = Parser {
        tokens,
        options,
        kind,
        pos: 0,
        nodes: Vec::new(),
        root: Vec::new(),
        delims: Vec::new(),
        top: None,
        brackets: Vec::new(),
        defs,
        footnotes,
        footnotes_used,
    };
    parser.run();
    if options.gfm_auto_link {
        parser.apply_auto_links();
    }
    if options.emoji {
        parser.apply_emoji();
    }
    parser.write_nodes_flat_json(out, first);
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
                b'*' | b'_' | b'~' | b'=' | b'#' => self.handle_delim(token),
                b'^' => {
                    // Lute：上标只在开关打开时把 ^ 当分隔符，否则是普通文本
                    if self.options.sup {
                        self.handle_delim(token);
                    } else {
                        self.parse_text();
                    }
                }
                b'\n' => self.parse_newline(),
                b'[' => self.parse_open_bracket(),
                b']' => self.parse_close_bracket(),
                b'!' => self.parse_bang(),
                b'<' => {
                    // 依次尝试自动链接、邮件自动链接、文件注解引用、行内 HTML，
                    // 都不成立就是普通字符。
                    if !self.parse_autolink()
                        && !self.parse_email_autolink()
                        && !self.parse_file_annotation_ref()
                        && !self.parse_inline_html()
                    {
                        self.pos += 1;
                        self.push_text(vec![b'<']);
                    }
                }
                b'&' => self.parse_entity(),
                b'$' => self.parse_inline_math(),
                b'{' => self.parse_heading_id(),
                b'(' => self.parse_block_ref(),
                _ => self.parse_text(),
            }
        }
        self.process_emphasis(None);
        self.merge_text();
    }

    /// 对应 render：把根节点列表转成 Value 数组。
    fn render(&self) -> Vec<Value> {
        self.root.iter().map(|index| self.render_node(*index)).collect()
    }

    /// 把行内节点平铺写进一个已经在写的数组（标题的 Children 就是这样拼的）。
    pub(super) fn write_nodes_flat_json(&self, out: &mut String, first: &mut bool) {
        for index in &self.root {
            if !*first {
                out.push(',');
            }
            *first = false;
            self.write_node_json(*index, out);
        }
    }

    fn write_node_json(&self, index: usize, out: &mut String) {
        let node = &self.nodes[index];
        // 快速路径：纯文本节点是行内最常见的（普通段落里几乎只有它），
        // 直接写死 "NodeText" 外壳，省掉一次转义和几轮分支判断。
        if node.node_type == "NodeText"
            && node.children.is_empty()
            && node.attrs.is_empty()
            && !node.data.is_empty()
        {
            out.push_str("{\"Type\":\"NodeText\",\"Data\":");
            write_json_string(out, &String::from_utf8_lossy(&node.data));
            out.push('}');
            return;
        }
        out.push_str("{\"Type\":");
        write_json_string(out, node.node_type);
        if !node.children.is_empty() {
            out.push_str(",\"Children\":[");
            for (i, child) in node.children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                self.write_node_json(*child, out);
            }
            out.push(']');
        }
        // Data 和 Children 可以同时存在（Lute 是 json.Marshal，两者互不排斥）
        if !node.data.is_empty() {
            out.push_str(",\"Data\":");
            write_json_string(out, &String::from_utf8_lossy(&node.data));
        }
        for (key, value) in &node.attrs {
            out.push(',');
            write_json_string(out, key);
            out.push(':');
            // 这几个属性值是解析期就定下来的小 JSON，直接借 serde 的紧凑输出
            let _ = write!(out, "{}", value);
        }
        out.push('}');
    }

    fn render_node(&self, index: usize) -> Value {
        let node = &self.nodes[index];
        let mut object = serde_json::Map::new();
        object.insert("Type".into(), json!(node.node_type));
        if !node.children.is_empty() {
            let children: Vec<Value> = node.children.iter().map(|child| self.render_node(*child)).collect();
            object.insert("Children".into(), Value::Array(children));
        }
        if !node.data.is_empty() {
            // 空内容不写 Data（对应 Go 的 omitempty）；Data 与 Children 可以并存
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

fn is_marker(options: &Options, token: u8) -> bool {
    // 对应 Lute 的 isMarker：上标开关打开时 ^ 也算潜在标记符
    if options.sup && token == b'^' {
        return true;
    }
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
fn scan_delims(
    options: &Options,
    tokens: &[u8],
    start: usize,
    end: usize,
    typ: u8,
) -> (bool, bool) {
    let count = end.saturating_sub(start);
    // Lute 的 scanDelims 对各种标记还有额外的开关与数量限制，不满足就既不能开也不能关。
    // 少了这一层，等号/井号/插入符会被塞进分隔符链表，打乱星号的配对顺序。
    let allowed = match typ {
        b'=' => options.mark && count == 2,
        b'#' => options.tag && count == 1,
        b'^' => options.sup && count == 1,
        b'~' => {
            if options.sub {
                if options.gfm_strikethrough && count == 3 {
                    true
                } else if count != 1 {
                    // 下标模式下只有两个波浪号还能当删除线用
                    options.gfm_strikethrough && count == 2
                } else {
                    true
                }
            } else if options.gfm_strikethrough {
                count != 1 || options.gfm_strikethrough1
            } else {
                true
            }
        }
        _ => true,
    };
    if !allowed {
        return (false, false);
    }

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
    let mut after_is_punct = is_punct(after);
    let before_is_whitespace = before.is_whitespace();
    let mut before_is_punct = is_punct(before);

    // Lute 的特例：波浪号紧挨 +/-（或与星号互邻）、插入符紧挨 +/- 时都不算标点。
    // 少了这条，"--D-~5199~" 这种路径文本里的 ~ 就无法闭合，
    // 而 "foo^-^" 这种上标也配不上对。
    if (typ == b'*' && after == '~')
        || (typ == b'~' && after == '*')
        || (typ == b'~' && (after == '+' || after == '-'))
        || (typ == b'^' && (after == '+' || after == '-'))
    {
        after_is_punct = false;
    }
    if (typ == b'*' && before == '~')
        || (typ == b'~' && before == '*')
        || (typ == b'~' && (before == '+' || before == '-'))
        || (typ == b'^' && (before == '+' || before == '-'))
    {
        before_is_punct = false;
    }

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



