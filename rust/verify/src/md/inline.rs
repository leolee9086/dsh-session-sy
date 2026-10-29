//! 复刻 Lute 的行内解析：单遍扫描 + 分隔符栈。
//!
//! 对应 parse/inlines.go（parseInline 主循环）、parse/inline.go（分派）、
//! parse/text.go（文本、转义、换行）、parse/delimiter.go（强调算法）。
//! 只实现 CommonMark 规范用例覆盖到的部分；未涉及的分支（链接、图片、内联 HTML、
//! 公式等）留作后续，字符会被当作普通文本。

mod link;

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
    fn parse_text(&mut self) {
        let start = self.pos;
        while self.pos < self.tokens.len() && !is_marker(self.tokens[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            // 当前字节是标记符但这一层不处理它，按普通文本吃掉一个字符。
            self.pos += 1;
        }
        let node = self.new_node("NodeText");
        self.nodes[node].data = self.tokens[start..self.pos].to_vec();
        self.append(node);
    }

    /// 对应 parseBackslash。
    fn parse_backslash(&mut self) {
        if self.pos == self.tokens.len() - 1 {
            self.pos += 1;
            let node = self.new_node("NodeText");
            self.nodes[node].data = vec![b'\\'];
            self.append(node);
            return;
        }
        self.pos += 1;
        let token = self.tokens[self.pos];
        if token == b'\n' {
            self.pos += 1;
            let node = self.new_node("NodeHardBreak");
            self.nodes[node].data = vec![token];
            self.append(node);
            return;
        }
        if is_ascii_punct(token) {
            self.pos += 1;
            let wrapper = self.new_node("NodeBackslash");
            let content = self.new_node("NodeBackslashContent");
            self.nodes[content].data = vec![token];
            self.nodes[wrapper].children = vec![content];
            self.append(wrapper);
            return;
        }
        let node = self.new_node("NodeText");
        self.nodes[node].data = vec![b'\\'];
        self.append(node);
    }

    /// 对应 parseNewline：前一个文本节点以两个以上空格结尾就是硬换行。
    fn parse_newline(&mut self) {
        let pos = self.pos;
        self.pos += 1;
        let mut hard = false;
        if let Some(&last) = self.root.last() {
            if self.nodes[last].node_type == "NodeText" {
                let tokens = self.nodes[last].data.clone();
                if tokens.last() == Some(&b' ') {
                    let trimmed = trim_right(&tokens);
                    let len = tokens.len();
                    if len > 1 && tokens[len - 2] == b' ' {
                        hard = true;
                    }
                    self.nodes[last].data = trimmed;
                }
            }
        }
        let node = self.new_node(if hard { "NodeHardBreak" } else { "NodeSoftBreak" });
        self.nodes[node].data = vec![self.tokens[pos]];
        self.append(node);
    }

    /// 对应 parseCodeSpan：反引号数量必须成对，首尾各去掉一个空格。
    fn parse_code_span(&mut self) {
        let start = self.pos;
        let mut run = 0usize;
        while start + run < self.tokens.len() && self.tokens[start + run] == b'\x60' {
            run += 1;
        }
        let mut i = start + run;
        let mut found = None;
        while i < self.tokens.len() {
            if self.tokens[i] == b'\x60' {
                let mut closing = 0usize;
                while i + closing < self.tokens.len() && self.tokens[i + closing] == b'\x60' {
                    closing += 1;
                }
                if closing == run {
                    found = Some(i);
                    break;
                }
                i += closing;
                continue;
            }
            i += 1;
        }
        let Some(end) = found else {
            // 没有闭合：整串反引号作为一个文本节点，交给后续合并
            let node = self.new_node("NodeText");
            self.nodes[node].data = self.tokens[start..start + run].to_vec();
            self.append(node);
            self.pos = start + run;
            return;
        };
        let raw = &self.tokens[start + run..end];
        let mut content = raw.to_vec();
        // 行内代码里的换行等价于空格
        for byte in content.iter_mut() {
            if *byte == b'\n' {
                *byte = b' ';
            }
        }
        let mut open_marker_data = vec![b'\x60'; run];
        let mut close_marker_data = vec![b'\x60'; run];
        // 首尾都是空格且内容不是全空白时，各去掉一个，并把空格并进标记
        if content.len() > 2
            && content[0] == b' '
            && content[content.len() - 1] == b' '
            && content.iter().any(|b| *b != b' ')
        {
            open_marker_data.push(content[0]);
            // 闭合标记里的那个空格同样来自替换换行后的内容
            close_marker_data = vec![content[content.len() - 1]];
            close_marker_data.extend(std::iter::repeat(b'`').take(run));
            content = content[1..content.len() - 1].to_vec();
        }
        let span = self.new_node("NodeCodeSpan");
        self.nodes[span].attrs.push(("CodeMarkerLen", json!(run)));
        let open = self.new_node("NodeCodeSpanOpenMarker");
        self.nodes[open].data = open_marker_data;
        let body = self.new_node("NodeCodeSpanContent");
        self.nodes[body].data = content;
        let close = self.new_node("NodeCodeSpanCloseMarker");
        self.nodes[close].data = close_marker_data;
        self.nodes[span].children = vec![open, body, close];
        self.append(span);
        self.pos = end + run;
    }

    /// 对应 handleDelim + scanDelims。
    fn handle_delim(&mut self, typ: u8) {
        let start = self.pos;
        let mut count = 0i32;
        while self.pos < self.tokens.len() && self.tokens[self.pos] == typ {
            count += 1;
            self.pos += 1;
        }
        let (can_open, can_close) = scan_delims(self.tokens, start, self.pos, typ);
        let node = self.new_node("NodeText");
        self.nodes[node].data = self.tokens[start..self.pos].to_vec();
        self.append(node);
        if can_open || can_close {
            let index = self.delims.len();
            self.delims.push(Delim {
                typ,
                num: count,
                original_num: count,
                unit_len: 1,
                node,
                can_open,
                can_close,
                prev: self.top,
                next: None,
            });
            if let Some(prev) = self.top {
                self.delims[prev].next = Some(index);
            }
            self.top = Some(index);
        }
    }

    fn remove_delimiter(&mut self, index: usize) {
        let prev = self.delims[index].prev;
        let next = self.delims[index].next;
        if let Some(prev) = prev {
            self.delims[prev].next = next;
        }
        match next {
            Some(next) => self.delims[next].prev = prev,
            None => self.top = prev,
        }
    }

    fn find_root(&self, node: usize) -> Option<usize> {
        self.root.iter().position(|item| *item == node)
    }

    /// 在整棵树里找节点所在的父级与下标（None 表示顶层）。
    fn find_parent_and_index(&self, node: usize) -> Option<(Option<usize>, usize)> {
        if let Some(index) = self.root.iter().position(|item| *item == node) {
            return Some((None, index));
        }
        for (parent_index, parent) in self.nodes.iter().enumerate() {
            if let Some(index) = parent.children.iter().position(|item| *item == node) {
                return Some((Some(parent_index), index));
            }
        }
        None
    }

    /// 对应 processEmphasis（commonmark.js 的 process_emphasis）。
    /// stack_bottom 之上的分隔符才参与，链接构造时会以 opener.previousDelimiter 为界。
    fn process_emphasis(&mut self, stack_bottom: Option<usize>) {
        let mut openers_bottom: [Option<usize>; 128] = [stack_bottom; 128];
        let mut closer = {
            let mut index = self.top;
            loop {
                match index {
                    Some(current) => {
                        if self.delims[current].prev == stack_bottom {
                            break;
                        }
                        index = self.delims[current].prev;
                    }
                    None => break,
                }
            }
            index
        };
        while let Some(current) = closer {
            let typ = self.delims[current].typ;
            if !self.delims[current].can_close {
                closer = self.delims[current].next;
                continue;
            }
            let mut opener = self.delims[current].prev;
            let mut found = false;
            let mut odd_match = false;
            while let Some(candidate) = opener {
                if Some(candidate) == openers_bottom[typ as usize] {
                    break;
                }
                odd_match = (self.delims[current].can_open || self.delims[candidate].can_close)
                    && self.delims[current].original_num % 3 != 0
                    && (self.delims[candidate].original_num + self.delims[current].original_num) % 3 == 0;
                if self.delims[candidate].typ == typ && self.delims[candidate].can_open && !odd_match {
                    found = true;
                    opener = Some(candidate);
                    break;
                }
                opener = self.delims[candidate].prev;
            }
            let old_closer = current;
            if !found {
                // Lute 只在「没找到 且 不是 oddMatch」时才设查找下限并回收该分隔符
                if !odd_match {
                    openers_bottom[typ as usize] = self.delims[old_closer].prev;
                    if !self.delims[old_closer].can_open {
                        self.remove_delimiter(old_closer);
                    }
                }
                closer = self.delims[current].next;
                continue;
            }
            let opener = opener.unwrap();
            let use_delims = if self.delims[current].num >= 2 && self.delims[opener].num >= 2 {
                2
            } else {
                1
            };
            let opener_node = self.delims[opener].node;
            let closer_node = self.delims[current].node;
            self.delims[opener].num -= use_delims;
            self.delims[current].num -= use_delims;
            let opener_len = self.delims[opener].unit_len * use_delims as usize;
            let closer_len = self.delims[current].unit_len * use_delims as usize;
            let opener_tokens: Vec<u8> = {
                let node = &mut self.nodes[opener_node];
                let at = node.data.len().saturating_sub(opener_len);
                node.data.split_off(at)
            };
            let closer_tokens: Vec<u8> = {
                let node = &mut self.nodes[closer_node];
                let at = node.data.len().saturating_sub(closer_len);
                node.data.split_off(at)
            };

            let (wrapper, open_type, close_type) = match (typ, use_delims) {
                (b'*', 1) => ("NodeEmphasis", "NodeEmA6kOpenMarker", "NodeEmA6kCloseMarker"),
                (b'_', 1) => ("NodeEmphasis", "NodeEmU8eOpenMarker", "NodeEmU8eCloseMarker"),
                (b'*', _) => ("NodeStrong", "NodeStrongA6kOpenMarker", "NodeStrongA6kCloseMarker"),
                (b'_', _) => ("NodeStrong", "NodeStrongU8eOpenMarker", "NodeStrongU8eCloseMarker"),
                _ => {
                    closer = self.delims[current].next;
                    continue;
                }
            };

            let mark = self.new_node(wrapper);
            let open_marker = self.new_node(open_type);
            self.nodes[open_marker].data = opener_tokens;
            let close_marker = self.new_node(close_type);
            self.nodes[close_marker].data = closer_tokens;

            if let (Some((opener_parent, open_index)), Some((closer_parent, close_index))) = (
                self.find_parent_and_index(opener_node),
                self.find_parent_and_index(closer_node),
            ) {
                // 同一层级才搬移；跨层级说明嵌套结构已变，跳过
                if opener_parent == closer_parent && open_index < close_index {
                    let inner: Vec<usize> = match opener_parent {
                        None => self.root.drain(open_index + 1..close_index).collect(),
                        Some(parent) => {
                            self.nodes[parent].children.drain(open_index + 1..close_index).collect()
                        }
                    };
                    let mut children = vec![open_marker];
                    children.extend(inner);
                    children.push(close_marker);
                    self.nodes[mark].children = children;
                    match opener_parent {
                        None => self.root.insert(open_index + 1, mark),
                        Some(parent) => {
                            self.nodes[parent].children.insert(open_index + 1, mark);
                        }
                    }
                }
            }

            // 匹配成功后，opener 与 closer 之间的分隔符要从链上摘掉
            if self.delims[opener].next != Some(current) {
                self.delims[opener].next = Some(current);
                self.delims[current].prev = Some(opener);
            }

            if self.delims[opener].num == 0 {
                if let Some((parent, index)) = self.find_parent_and_index(opener_node) {
                    match parent {
                        None => {
                            self.root.remove(index);
                        }
                        Some(parent) => {
                            self.nodes[parent].children.remove(index);
                        }
                    }
                }
                self.remove_delimiter(opener);
            }
            if self.delims[current].num == 0 {
                if let Some((parent, index)) = self.find_parent_and_index(closer_node) {
                    match parent {
                        None => {
                            self.root.remove(index);
                        }
                        Some(parent) => {
                            self.nodes[parent].children.remove(index);
                        }
                    }
                }
                let next = self.delims[current].next;
                self.remove_delimiter(current);
                closer = next;
            }
        }
        // 处理结束后清掉本次范围内的分隔符（对应 Lute 末尾的 removeDelimiter 循环）
        while let Some(current) = self.top {
            if Some(current) == stack_bottom {
                break;
            }
            self.remove_delimiter(current);
        }
    }

    
/// 合并文本时同样要把链接文本算进去（对应 Lute 的 mergeText）。
fn is_text_like(node_type: &str) -> bool {
    node_type == "NodeText" || node_type == "NodeLinkText"
}

/// 对应 mergeText：递归合并相邻的文本节点。
    fn merge_text(&mut self) {
        self.merge_root_text();
        let roots = self.root.clone();
        for node in roots {
            self.merge_descendant_text(node);
        }
    }

    fn merge_root_text(&mut self) {
        let mut index = 0usize;
        while index < self.root.len() {
            let current = self.root[index];
            if !Self::is_text_like(self.nodes[current].node_type) {
                index += 1;
                continue;
            }
            while index + 1 < self.root.len() {
                let next = self.root[index + 1];
                if self.nodes[next].node_type != self.nodes[current].node_type {
                    break;
                }
                let extra = self.nodes[next].data.clone();
                self.nodes[current].data.extend_from_slice(&extra);
                self.root.remove(index + 1);
            }
            index += 1;
        }
    }

    fn merge_descendant_text(&mut self, node: usize) {
        let mut index = 0usize;
        loop {
            let length = self.nodes[node].children.len();
            if index >= length {
                break;
            }
            let current = self.nodes[node].children[index];
            if !Self::is_text_like(self.nodes[current].node_type) {
                index += 1;
                continue;
            }
            loop {
                let length = self.nodes[node].children.len();
                if index + 1 >= length {
                    break;
                }
                let next = self.nodes[node].children[index + 1];
                if self.nodes[next].node_type != self.nodes[current].node_type {
                    break;
                }
                let extra = self.nodes[next].data.clone();
                self.nodes[current].data.extend_from_slice(&extra);
                self.nodes[node].children.remove(index + 1);
            }
            index += 1;
        }
        let children = self.nodes[node].children.clone();
        for child in children {
            self.merge_descendant_text(child);
        }
    }

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

impl<'a> Parser<'a> {
    fn push_text(&mut self, data: Vec<u8>) {
        let node = self.new_node("NodeText");
        self.nodes[node].data = data;
        self.append(node);
    }

    fn add_bracket(&mut self, node: usize, index: usize, image: bool) {
        if let Some(last) = self.brackets.last_mut() {
            last.bracket_after = true;
        }
        self.brackets.push(Bracket {
            node,
            index,
            image,
            active: true,
            previous_delimiter: self.top,
            bracket_after: false,
        });
    }

    /// 对应 parseOpenBracket。
    fn parse_open_bracket(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let node = self.new_node("NodeText");
        self.nodes[node].data = self.tokens[start..self.pos].to_vec();
        self.append(node);
        self.add_bracket(node, start, false);
    }

    /// 对应 parseBang：感叹号紧跟左方括号才算图片起始。
    fn parse_bang(&mut self) {
        let start = self.pos;
        self.pos += 1;
        if self.tokens.get(self.pos) == Some(&b'[') {
            self.pos += 1;
            let node = self.new_node("NodeText");
            self.nodes[node].data = self.tokens[start..self.pos].to_vec();
            self.append(node);
            self.add_bracket(node, start + 1, true);
            return;
        }
        self.push_text(vec![b'!']);
    }

    /// 对应 parseCloseBracket。
    fn parse_close_bracket(&mut self) {
        self.pos += 1;
        let start_pos = self.pos;
        let Some(opener) = self.brackets.last().cloned() else {
            self.push_text(vec![b']']);
            return;
        };
        if !opener.active {
            self.brackets.pop();
            self.push_text(vec![b']']);
            return;
        }
        let is_image = opener.image;
        let mut dest: Vec<u8> = Vec::new();
        let mut title: Option<Vec<u8>> = None;
        let mut space: Option<Vec<u8>> = None;
        let mut link_type: i64 = 0;
        let mut close_paren_data: Option<Vec<u8>> = None;
        let mut open_paren_data: Option<Vec<u8>> = None;
        let mut reflabel: Option<Vec<u8>> = None;
        let mut matched = false;
        let save_pos = self.pos;

        // 1) 内联链接：文字后紧跟圆括号目标
        if self.pos + 1 < self.tokens.len() && self.tokens[self.pos] == b'(' {
            let paren_pos = self.pos;
            // Lute 一进入内联分支就记下 openParen，失败时也保留
            open_paren_data = Some(b"(".to_vec());
            if let Some((passed_len, value)) = parse_inline_link_dest(&self.tokens[paren_pos..]) {
                let after = paren_pos + passed_len;
                let ends_with_paren =
                    passed_len > 1 && self.tokens.get(after.wrapping_sub(1)).copied() == Some(b')');
                if ends_with_paren {
                    let okay = if after < self.tokens.len() {
                        !is_whitespace(self.tokens[after])
                            && self.tokens.get(after + 1).copied() != Some(b'"')
                    } else {
                        true
                    };
                    if okay {
                        dest = value.clone();
                        close_paren_data = Some(b")".to_vec());
                        self.pos = after;
                        matched = true;
                    }
                } else {
                    // 目标后跟空白：可能还有标题
                    let mut cursor = after;
                    let space_start = cursor;
                    while cursor < self.tokens.len() && is_whitespace(self.tokens[cursor]) {
                        cursor += 1;
                    }
                    if cursor > space_start {
                        if let Some((title_value, title_end)) =
                            scan_link_title(&self.tokens, cursor)
                        {
                            let mut tail = title_end;
                            while tail < self.tokens.len() && is_whitespace(self.tokens[tail]) {
                                tail += 1;
                            }
                            if self.tokens.get(tail) == Some(&b')') {
                                dest = value.clone();
                                title = Some(title_value);
                                space = Some(self.tokens[space_start..cursor].to_vec());
                                // Lute 在带标题的分支里把整段剩余都放进 CloseParen
                                close_paren_data = Some(self.tokens[tail..].to_vec());
                                self.pos = tail + 1;
                                matched = true;
                            }
                        } else if cursor < self.tokens.len() {
                            // 标题解析失败：Lute 的 closeParen 残留为目标后的第一个字符
                            space = Some(self.tokens[space_start..cursor].to_vec());
                            close_paren_data = Some(vec![self.tokens[cursor]]);
                        }
                        if !matched && self.tokens.get(cursor) == Some(&b')') {
                            dest = value.clone();
                            space = Some(self.tokens[space_start..cursor].to_vec());
                            close_paren_data = Some(b")".to_vec());
                            self.pos = cursor + 1;
                            matched = true;
                        }
                    }
                }
            }
            if !matched {
                // 回退位置，但 space / close_paren 保留内联尝试时的中间值（Lute 的行为）
                self.pos = save_pos;
                title = None;
                dest.clear();
            }
        }

        // 2) 引用链接：[text][label] 或 [text][]
        if !matched {
            let before = self.pos;
            let mut consumed = 0usize;
            let mut label: Option<Vec<u8>> = None;
            if let Some((text, next)) = scan_link_label(self.tokens, before) {
                if next > 2 {
                    label = Some(text);
                    consumed = next;
                }
            }
            if label.is_none() && !opener.bracket_after {
                if let Some(open_index) = self.find_root(opener.node) {
                    if before > 0 && open_index + 1 < self.root.len() {
                        // 快捷引用的标签取自原文，避免反斜杠转义在解析后丢失
                        let text = if opener.index + 1 <= before - 1 && before - 1 <= self.tokens.len() {
                            self.tokens[opener.index + 1..before - 1].to_vec()
                        } else {
                            Vec::new()
                        };
                        label = Some(text);
                        // 只有紧跟 [] 时才消耗两个字符；纯快捷引用保持原位
                        consumed = if self.tokens.get(before) == Some(&b'[')
                            && self.tokens.get(before + 1) == Some(&b']')
                        {
                            2
                        } else {
                            0
                        };
                    }
                }
            }
            if let Some(label) = label {
                let key = normalize_label(&label);
                if let Some((value, title_value)) = self.defs.get(&key) {
                    dest = value.clone();
                    title = title_value.clone();
                    // LinkRefLabel 保留原文（含大小写），只有查找用规范化键
                    reflabel = Some(label.clone());
                    link_type = 3;
                    matched = true;
                    self.pos += consumed;
                } else if consumed > 0 {
                    self.pos = start_pos;
                }
            }
        }

        if !matched {
            self.brackets.pop();
            self.pos = start_pos;
            self.push_text(vec![b']']);
            return;
        }

        // 先把区间内的顶层文本改类型，再处理链接内部强调——这样强调里的文本也是 LinkText
        if let Some(open_index) = self.find_root(opener.node) {
            for index in open_index + 1..self.root.len() {
                let node = self.root[index];
                if self.nodes[node].node_type == "NodeText" {
                    self.nodes[node].node_type = "NodeLinkText";
                }
            }
        }
        self.process_emphasis(opener.previous_delimiter);

        // 3) 组装链接节点
        let node = self.new_node(if is_image { "NodeImage" } else { "NodeLink" });
        if link_type != 0 {
            self.nodes[node].attrs.push(("LinkType", json!(link_type)));
        }
        if let Some(label) = &reflabel {
            self.nodes[node].attrs.push(("LinkRefLabel", json!(base64(label))));
        }
        let mut children: Vec<usize> = Vec::new();
        if is_image {
            let bang = self.new_node("NodeBang");
            self.nodes[bang].data = vec![b'!'];
            children.push(bang);
        }
        let open_marker = self.new_node("NodeOpenBracket");
        self.nodes[open_marker].data = b"[".to_vec();
        children.push(open_marker);

        // 把 opener 与 closer 之间的节点收进链接，文本统一按 LinkText 呈现
        if let Some(open_index) = self.find_root(opener.node) {
            let inner: Vec<usize> = self.root.drain(open_index + 1..).collect();
            // 保持原有顺序，只把文本节点改类型为 LinkText（强调等节点原样保留）
            for index in inner {
                if self.nodes[index].node_type == "NodeText" {
                    self.nodes[index].node_type = "NodeLinkText";
                }
                children.push(index);
            }
            self.root.remove(open_index);
        }

        let close_marker = self.new_node("NodeCloseBracket");
        self.nodes[close_marker].data = b"]".to_vec();
        children.push(close_marker);
        let open_paren = self.new_node("NodeOpenParen");
        if let Some(value) = &open_paren_data {
            self.nodes[open_paren].data = value.clone();
        }
        children.push(open_paren);
        let dest_node = self.new_node("NodeLinkDest");
        self.nodes[dest_node].data = dest;
        children.push(dest_node);
        if let Some(space) = space {
            let space_node = self.new_node("NodeLinkSpace");
            self.nodes[space_node].data = space;
            children.push(space_node);
        }
        if let Some(title) = title {
            let title_node = self.new_node("NodeLinkTitle");
            self.nodes[title_node].data = title;
            children.push(title_node);
        }
        let close_paren = self.new_node("NodeCloseParen");
        if let Some(value) = &close_paren_data {
            self.nodes[close_paren].data = value.clone();
        }
        children.push(close_paren);
        self.nodes[node].children = children;
        self.root.push(node);
        self.brackets.pop();

        // 链接里不能再嵌套链接
        if !is_image {
            for bracket in self.brackets.iter_mut() {
                if !bracket.image {
                    bracket.active = false;
                }
            }
        }
    }
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

    /// 对应 parseInlineHTML：开标签、闭标签、注释、处理指令、声明、CDATA。
    fn parse_inline_html(&mut self) -> bool {
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
    fn parse_entity(&mut self) {
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



