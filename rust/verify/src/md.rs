//! 复刻 Lute 的 markdown → 思源 AST 解析（块级部分）。
//!
//! 结构照搬 Lute：块级用「容器栈 + 逐行 incorporateLine」两阶段解析（对应
//! parse/blocks.go），行内解析在块解析完成后统一进行（对应 parse/inlines.go）。
//! 判据是 tests/commonmark-fixtures.json：同一段 markdown，Lute 产出什么 AST，
//! 这里就必须产出什么。

mod inline;

use serde_json::{json, Value};

/// 块节点。用 arena 存，子节点存下标，模拟 Lute 的 Tip / parent / LastChild。
#[derive(Default)]
struct Node {
    node_type: &'static str,
    children: Vec<usize>,
    parent: Option<usize>,
    close: bool,
    /// 累积的原始文本行（段落、标题、代码块在这里攒，行内解析时再拆）。
    tokens: Vec<u8>,
    /// 额外字段，序列化时原样带上。
    attrs: Vec<(&'static str, Value)>,
    /// 列表数据。
    list_data: Option<ListData>,
    /// 末行是否为空行，用于判断列表紧凑/松散。
    last_line_blank: bool,
}

#[derive(Default, Clone)]
struct ListData {
    typ: i64,
    tight: bool,
    bullet_char: u8,
    start: i64,
    delimiter: u8,
    padding: usize,
    marker_offset: usize,
    marker: String,
    num: i64,
}

struct Parser {
    nodes: Vec<Node>,
    tip: Option<usize>,
    old_tip: Option<usize>,
    last_matched_container: Option<usize>,

    /// 链接引用定义索引：规范化标签 → (目标, 可选标题)。
    link_ref_defs: std::collections::HashMap<Vec<u8>, (Vec<u8>, Option<Vec<u8>>)>,
    /// 本块解析出的引用定义，用于生成 NodeLinkRefDefBlock。

    current_line: Vec<u8>,
    offset: usize,
    column: usize,
    next_nonspace: usize,
    next_nonspace_column: usize,
    indent: usize,
    indented: bool,
    blank: bool,
    partially_consumed_tab: bool,
    all_closed: bool,
}

/// 解析 markdown，返回 NodeDocument。
pub fn parse(src: &str) -> Value {
    let mut parser = Parser::new();
    let root = parser.add_root();
    parser.tip = Some(root);
    parser.parse_blocks(src);
    parser.render(root)
}

impl Parser {
    fn new() -> Self {
        Parser {
            nodes: Vec::new(),
            tip: None,
            old_tip: None,
            last_matched_container: None,
            link_ref_defs: std::collections::HashMap::new(),
            current_line: Vec::new(),
            offset: 0,
            column: 0,
            next_nonspace: 0,
            next_nonspace_column: 0,
            indent: 0,
            indented: false,
            blank: false,
            partially_consumed_tab: false,
            all_closed: false,
        }
    }

    fn new_node(&mut self, node_type: &'static str) -> usize {
        self.nodes.push(Node {
            node_type,
            ..Default::default()
        });
        self.nodes.len() - 1
    }

    fn add_root(&mut self) -> usize {
        self.new_node("NodeDocument")
    }

    fn append_child(&mut self, parent: usize, child: usize) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    fn last_child(&self, index: usize) -> Option<usize> {
        self.nodes[index].children.last().copied()
    }

    /// 对应 Lute 的 parseBlocks：逐行喂给 incorporateLine，最后关闭所有未闭合块。
    fn parse_blocks(&mut self, src: &str) {
        let lines: Vec<Vec<u8>> = src.split_inclusive('\n').map(|line| line.as_bytes().to_vec()).collect();
        for line in &lines {
            self.incorporate_line(line);
        }
        while let Some(tip) = self.tip {
            self.finalize(tip);
        }
    }

    /// 对应 Lute 的 incorporateLine。
    fn incorporate_line(&mut self, line: &[u8]) {
        self.old_tip = self.tip;
        self.offset = 0;
        self.column = 0;
        self.blank = false;
        self.partially_consumed_tab = false;
        self.current_line = line.to_vec();

        let mut all_matched = true;
        let mut container = 0usize;
        let root = 0usize;
        while let Some(last) = self.last_child(container) {
            if self.nodes[last].close {
                break;
            }
            container = last;
            self.find_next_nonspace();
            match self.continue_block(container) {
                0 => {}
                1 => {
                    all_matched = false;
                }
                2 => return,
                _ => {}
            }
            if !all_matched {
                container = self.nodes[container].parent.unwrap_or(root);
                break;
            }
        }

        self.all_closed = Some(container) == self.old_tip;
        self.last_matched_container = Some(container);

        let mut matched_leaf =
            self.nodes[container].node_type != "NodeParagraph" && self.accepts_lines(container);

        while !matched_leaf {
            self.find_next_nonspace();
            if !self.starts_block_hint() {
                self.advance_next_nonspace();
                break;
            }
            match self.try_block_starts(container) {
                Some((kind, leaf)) => {
                    container = kind;
                    if leaf {
                        matched_leaf = true;
                    }
                }
                None => {
                    self.advance_next_nonspace();
                    break;
                }
            }
        }

        let tip_is_paragraph =
            self.nodes[self.tip.unwrap_or(root)].node_type == "NodeParagraph";
        if !self.all_closed && !self.blank && tip_is_paragraph {
            self.add_line();
        } else {
            self.close_unmatched_blocks();
            // 空行同时标到容器的最后一个子节点上（Lute 的 container.LastChild.LastLineBlank），
            // 列表的松散判定要看子节点自己的标记。
            if self.blank {
                if let Some(last) = self.last_child(container) {
                    self.nodes[last].last_line_blank = true;
                }
            }
            // 空行标记沿容器链向上传播：列表靠它判断紧凑模式。
            // 引用、围栏代码块、空列表项内部的空行都不计入（Lute 的例外表）。
            let container_type = self.nodes[container].node_type;
            let fenced = container_type == "NodeCodeBlock"
                && self.nodes[container]
                    .attrs
                    .iter()
                    .any(|(key, _)| *key == "IsFencedCodeBlock");
            let blank_marker = self.blank
                && !(container_type == "NodeBlockquote"
                    || container_type == "NodeCallout"
                    || container_type == "NodeMathBlock"
                    || fenced
                    || (container_type == "NodeListItem"
                        && self.nodes[container].children.is_empty()));
            let mut cursor = Some(container);
            while let Some(current) = cursor {
                self.nodes[current].last_line_blank = blank_marker;
                cursor = self.nodes[current].parent;
            }
            if self.accepts_lines(container) {
                self.add_line();
                // HTML 块 1-5 型靠行内闭合标记结束，必须在本行加入之后判断。
                if self.nodes[container].node_type == "NodeHTMLBlock" {
                    let html_type = self.attr_u64(container, "HtmlBlockType").unwrap_or(0);
                    if (1..=5).contains(&html_type) {
                        let rest = self.current_line[self.offset.min(self.current_line.len())..].to_vec();
                        if html_block_closes(&rest, html_type) {
                            self.finalize(container);
                        }
                    }
                }
            } else if self.offset < self.current_line.len() && !self.blank {
                self.add_child("NodeParagraph");
                self.advance_next_nonspace();
                self.add_line();
            }
        }
    }

    /// 行首可能是块标记的快速判断（对应 Lute 的 maybeMarker 检查）。
    fn starts_block_hint(&self) -> bool {
        // 缩进行不按标记字符预判，交给 IndentCodeBlockStart 决定。
        if self.indented {
            return true;
        }
        let Some(&marker) = self.current_line.get(self.next_nonspace) else {
            return false;
        };
        matches!(
            marker,
            b'#' | b'>' | b'-' | b'*' | b'+' | b'_' | b'=' | b'\x60' | b'~' | b'<' | b'[' | b'{'
        ) || marker.is_ascii_digit()
    }

    /// 返回 (新的容器下标, 是否叶子块)。
    fn try_block_starts(&mut self, container: usize) -> Option<(usize, bool)> {
        // 顺序照搬 Lute 的 blockStarts：容器块先试，叶子块按 Setext / HTML / 分隔线 / 列表排。
        if self.blockquote_start(container) {
            return Some((self.tip?, false));
        }
        if self.atx_heading_start(container) {
            return Some((self.tip?, true));
        }
        if self.fence_code_block_start(container) {
            return Some((self.tip?, true));
        }
        if self.setext_heading_start(container) {
            return Some((self.tip?, true));
        }
        if self.html_block_start(container) {
            return Some((self.tip?, true));
        }
        if self.thematic_break_start(container) {
            return Some((self.tip?, true));
        }
        if self.list_start(container) {
            return Some((self.tip?, false));
        }
        if self.indent_code_block_start(container) {
            return Some((self.tip?, true));
        }
        None
    }

    fn accepts_lines(&self, index: usize) -> bool {
        matches!(
            self.nodes[index].node_type,
            "NodeParagraph" | "NodeCodeBlock" | "NodeHTMLBlock"
        )
    }

    fn add_line(&mut self) {
        let Some(tip) = self.tip else { return };
        let mut start = self.offset;
        if self.partially_consumed_tab {
            start += 1;
            let spaces = 4 - (self.column % 4);
            self.nodes[tip].tokens.extend(std::iter::repeat(b' ').take(spaces));
        }
        if start < self.current_line.len() {
            let tail = self.current_line[start..].to_vec();
            self.nodes[tip].tokens.extend_from_slice(&tail);
        }
    }

    /// 对应 Lute 的 addChild：末梢不能容纳时先 finalize，直到找到能容纳的容器。
    fn add_child(&mut self, node_type: &'static str) -> usize {
        while let Some(tip) = self.tip {
            if self.can_contain(tip, node_type) {
                break;
            }
            self.finalize(tip);
        }
        let node = self.new_node(node_type);
        let parent = self.tip.unwrap_or(0);
        self.append_child(parent, node);
        self.tip = Some(node);
        node
    }

    fn can_contain(&self, parent: usize, node_type: &'static str) -> bool {
        match self.nodes[parent].node_type {
            "NodeDocument" | "NodeBlockquote" | "NodeListItem" => {
                !matches!(node_type, "NodeListItem" | "NodeBlockquoteMarker")
            }
            "NodeList" => node_type == "NodeListItem",
            _ => false,
        }
    }

    fn close_unmatched_blocks(&mut self) {
        if self.all_closed {
            return;
        }
        while self.old_tip != self.last_matched_container {
            let Some(old) = self.old_tip else { break };
            let parent = self.nodes[old].parent;
            self.finalize(old);
            self.old_tip = parent;
        }
        self.all_closed = true;
    }

    fn finalize(&mut self, block: usize) {
        let parent = self.nodes[block].parent;
        self.nodes[block].close = true;
        match self.nodes[block].node_type {
            "NodeCodeBlock" => self.code_block_finalize(block),
            "NodeHTMLBlock" => self.html_block_finalize(block),
            "NodeList" => self.list_finalize(block),
            "NodeParagraph" => self.paragraph_finalize(block),
            _ => {}
        }
        self.tip = parent;
    }

    fn find_next_nonspace(&mut self) {
        let mut i = self.offset;
        let mut cols = self.column;
        while i < self.current_line.len() {
            match self.current_line[i] {
                b' ' => {
                    i += 1;
                    cols += 1;
                }
                b'\t' => {
                    i += 1;
                    cols += 4 - (cols % 4);
                }
                _ => break,
            }
        }
        // 行尾是换行符，所以不能只比较下标是否到长度——空行必须靠字符判定。
        self.blank = i >= self.current_line.len()
            || matches!(self.current_line[i], b'\n' | b'\r');
        self.next_nonspace = i;
        self.next_nonspace_column = cols;
        self.indent = cols.saturating_sub(self.column);
        self.indented = self.indent >= 4;
    }

    fn advance_next_nonspace(&mut self) {
        self.offset = self.next_nonspace;
        self.column = self.next_nonspace_column;
        self.partially_consumed_tab = false;
    }

    fn advance_offset(&mut self, count: usize, columns: bool) {
        let mut count = count;
        while count > 0 {
            let Some(&c) = self.current_line.get(self.offset) else {
                break;
            };
            if c == b'\t' {
                let chars_to_tab = 4 - (self.column % 4);
                if columns {
                    self.partially_consumed_tab = chars_to_tab > count;
                    let chars_to_advance = if self.partially_consumed_tab {
                        count
                    } else {
                        chars_to_tab
                    };
                    self.column += chars_to_advance;
                    count -= chars_to_advance;
                    if !self.partially_consumed_tab {
                        self.offset += 1;
                    }
                } else {
                    self.partially_consumed_tab = false;
                    self.column += chars_to_tab;
                    self.offset += 1;
                    count -= 1;
                }
            } else {
                self.partially_consumed_tab = false;
                self.offset += 1;
                self.column += 1;
                count -= 1;
            }
        }
    }

    fn rest_of_line(&self) -> &[u8] {
        &self.current_line[self.next_nonspace..]
    }

    fn attr_u64(&self, node: usize, key: &str) -> Option<u64> {
        self.nodes[node]
            .attrs
            .iter()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value.as_u64())
    }
}

impl Parser {
    fn continue_block(&mut self, node: usize) -> i32 {
        match self.nodes[node].node_type {
            "NodeParagraph" => {
                if self.blank {
                    1
                } else {
                    0
                }
            }
            "NodeCodeBlock" => self.code_block_continue(node),
            "NodeHTMLBlock" => self.html_block_continue(node),
            "NodeBlockquote" => self.blockquote_continue(node),
            "NodeListItem" => self.list_item_continue(node),
            "NodeHeading" | "NodeThematicBreak" => 1,
            _ => 0,
        }
    }

    fn code_block_continue(&mut self, node: usize) -> i32 {
        let fenced = self.nodes[node]
            .attrs
            .iter()
            .any(|(key, _)| *key == "IsFencedCodeBlock");
        if !fenced {
            if self.indent >= 4 {
                self.advance_offset(4, true);
                return 0;
            }
            if self.blank {
                self.advance_next_nonspace();
                return 0;
            }
            return 1;
        }
        let fence_len = self.attr_u64(node, "FenceLen").unwrap_or(3) as usize;
        let fence_char = self.attr_u64(node, "FenceChar").unwrap_or(96) as u8;
        let fence_offset = self.attr_u64(node, "FenceOffset").unwrap_or(0) as usize;
        if self.indent <= 3 {
            let rest = self.rest_of_line().to_vec();
            if fence_len_rest(&rest, fence_char, fence_len) {
                let close = String::from_utf8_lossy(trim_right_whitespace(&rest)).to_string();
                self.nodes[node]
                    .attrs
                    .push(("CloseFence", json!(base64(close.as_bytes()))));
                self.finalize(node);
                return 2;
            }
        }
        let mut i = 0usize;
        while i < fence_offset {
            match self.current_line.get(self.offset) {
                Some(b' ') | Some(b'\t') => {
                    self.advance_offset(1, true);
                    i += 1;
                }
                _ => break,
            }
        }
        0
    }

    fn blockquote_continue(&mut self, _node: usize) -> i32 {
        if self.indented {
            return 1;
        }
        if self.current_line.get(self.next_nonspace) == Some(&b'>') {
            self.advance_next_nonspace();
            self.advance_offset(1, false);
            if matches!(self.current_line.get(self.offset), Some(b' ') | Some(b'\t')) {
                self.advance_offset(1, true);
            }
            return 0;
        }
        1
    }

    fn list_item_continue(&mut self, node: usize) -> i32 {
        let Some(data) = self.nodes[node].list_data.clone() else {
            return 1;
        };
        if self.blank {
            if self.nodes[node].children.is_empty() {
                return 1;
            }
            self.advance_next_nonspace();
            return 0;
        }
        if self.indent >= data.marker_offset + data.padding {
            self.advance_offset(data.marker_offset + data.padding, true);
            return 0;
        }
        1
    }
}

impl Parser {
    fn atx_heading_start(&mut self, _container: usize) -> bool {
        if self.indented {
            return false;
        }
        let rest = self.rest_of_line().to_vec();
        if rest.first() != Some(&b'#') {
            return false;
        }
        let mut level = 0usize;
        while level < rest.len() && rest[level] == b'#' {
            level += 1;
        }
        if level > 6 {
            return false;
        }
        if level < rest.len() && !matches!(rest[level], b' ' | b'\t' | b'\n' | b'\r') {
            return false;
        }
        let marker_end = (self.next_nonspace + level + 1).min(self.current_line.len());
        let markers = self.current_line[self.next_nonspace..marker_end].to_vec();
        self.advance_next_nonspace();
        self.advance_offset(level, false);
        // 标记之后的前导空白不属于标题内容。
        while matches!(self.current_line.get(self.offset), Some(b' ') | Some(b'\t')) {
            self.offset += 1;
        }
        let mut content: Vec<u8> = Vec::new();
        while let Some(&c) = self.current_line.get(self.offset) {
            if matches!(c, b'\n' | b'\r') {
                break;
            }
            content.push(c);
            self.offset += 1;
        }
        let content = strip_closing_crosshatch(&content);
        self.close_unmatched_blocks();
        let heading = self.add_child("NodeHeading");
        self.nodes[heading].attrs.push(("HeadingLevel", json!(level)));
        self.nodes[heading].tokens = content;
        let marker_node = self.new_node("NodeHeadingC8hMarker");
        self.nodes[marker_node].tokens = markers;
        self.nodes[marker_node].close = true;
        self.append_child(heading, marker_node);
        self.advance_offset(self.current_line.len() - self.offset, false);
        true
    }

    fn fence_code_block_start(&mut self, _container: usize) -> bool {
        if self.indented {
            return false;
        }
        let rest = self.rest_of_line().to_vec();
        let Some(ch) = rest.first().copied() else {
            return false;
        };
        if ch != b'\x60' && ch != b'~' {
            return false;
        }
        let mut len = 0usize;
        while len < rest.len() && rest[len] == ch {
            len += 1;
        }
        if len < 3 {
            return false;
        }
        let mut info: Vec<u8> = Vec::new();
        let mut i = len;
        while i < rest.len() && !matches!(rest[i], b'\n' | b'\r') {
            info.push(rest[i]);
            i += 1;
        }
        if ch == b'\x60' && info.contains(&b'\x60') {
            return false;
        }
        self.close_unmatched_blocks();
        let block = self.add_child("NodeCodeBlock");
        self.nodes[block].attrs.push(("IsFencedCodeBlock", json!(true)));
        self.nodes[block].attrs.push(("FenceChar", json!(ch)));
        self.nodes[block].attrs.push(("FenceLen", json!(len)));
        self.nodes[block].attrs.push(("FenceOffset", json!(self.indent)));
        let open_fence = String::from_utf8_lossy(&rest[..len]).to_string();
        self.nodes[block]
            .attrs
            .push(("OpenFence", json!(base64(open_fence.as_bytes()))));
        let mut info_text = String::from_utf8_lossy(&info).trim().to_owned();
        if let Some(index) = info_text.find(' ') {
            info_text.truncate(index);
        }
        // 围栏信息里的反斜杠转义与 HTML 实体都要解开
        let info_text = inline::unescape_backslashes(info_text.as_bytes());
        let info_text = String::from_utf8_lossy(&info_text).to_string();
        if !info_text.is_empty() {
            self.nodes[block]
                .attrs
                .push(("Info", json!(base64(info_text.as_bytes()))));
        }
        self.advance_next_nonspace();
        self.advance_offset(len, false);
        true
    }

    fn thematic_break_start(&mut self, _container: usize) -> bool {
        if self.indented {
            return false;
        }
        let rest = self.rest_of_line().to_vec();
        let Some(ch) = rest.first().copied() else {
            return false;
        };
        if ch != b'-' && ch != b'*' && ch != b'_' {
            return false;
        }
        let mut count = 0usize;
        for &b in &rest {
            if b == ch {
                count += 1;
            } else if !matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                return false;
            }
        }
        if count < 3 {
            return false;
        }
        self.close_unmatched_blocks();
        self.add_child("NodeThematicBreak");
        self.advance_offset(self.current_line.len() - self.offset, false);
        true
    }

    fn blockquote_start(&mut self, _container: usize) -> bool {
        if self.indented {
            return false;
        }
        if self.current_line.get(self.next_nonspace) != Some(&b'>') {
            return false;
        }
        self.advance_next_nonspace();
        self.advance_offset(1, false);
        // 标记本身记录真实消耗的字节：">" 加紧随其后的可选空白。
        let mut markers = vec![b'>'];
        if let Some(&whitespace) = self.current_line.get(self.offset) {
            if whitespace == b' ' || whitespace == b'\t' {
                self.advance_offset(1, true);
                markers.push(whitespace);
            }
        }
        self.close_unmatched_blocks();
        let quote = self.add_child("NodeBlockquote");
        let marker = self.new_node("NodeBlockquoteMarker");
        self.nodes[marker].tokens = markers;
        self.nodes[marker].close = true;
        self.append_child(quote, marker);
        true
    }

    fn indent_code_block_start(&mut self, _container: usize) -> bool {
        if self.indent < 4 || self.blank {
            return false;
        }
        // 缩进代码块不能打断正在延续的段落（Lute: Tip.Type != NodeParagraph）
        let tip_is_paragraph = self
            .tip
            .map(|index| self.nodes[index].node_type == "NodeParagraph")
            .unwrap_or(false);
        if tip_is_paragraph {
            return false;
        }
        self.advance_offset(4, true);
        self.close_unmatched_blocks();
        self.add_child("NodeCodeBlock");
        true
    }

    /// 对应 ListStart。
    fn list_start(&mut self, container: usize) -> bool {
        if self.nodes[container].node_type != "NodeList" && self.indented {
            return false;
        }
        let Some(data) = self.parse_list_marker(container) else {
            return false;
        };
        self.close_unmatched_blocks();
        let matches_previous = self.nodes[container].node_type == "NodeList"
            && self.nodes[container]
                .list_data
                .as_ref()
                .is_some_and(|list| lists_match(list, &data));
        if self.tip != Some(container) || !matches_previous {
            let list = self.add_child("NodeList");
            // 列表自身的 ListData 保留首项的记号与起始序号
            let mut list_data = data.clone();
            if list_data.typ == 1 {
                list_data.num = data.start;
            }
            self.nodes[list].list_data = Some(list_data);
        }
        let item = self.add_child("NodeListItem");
        // 有序列表的序号：首项用标记里的起始号，其余按前一项递增。
        let mut item_data = data.clone();
        if item_data.typ == 1 {
            let mut number = data.start;
            if let Some(parent) = self.nodes[item].parent {
                if let Some(position) = self.nodes[parent]
                    .children
                    .iter()
                    .position(|child| *child == item)
                {
                    if position > 0 {
                        let prev = self.nodes[parent].children[position - 1];
                        if let Some(prev_data) = self.nodes[prev].list_data.as_ref() {
                            number = prev_data.num + 1;
                        }
                    }
                }
            }
            item_data.num = number;
        }
        self.nodes[item].list_data = Some(item_data);
        true
    }

    /// 对应 Lute 的 parseListMarker：在行上推进 offset/column 计算 Padding 与 Marker。
    fn parse_list_marker(&mut self, container: usize) -> Option<ListData> {
        if self.indent >= 4 {
            return None;
        }
        let ln = self.current_line.clone();
        let mut data = ListData {
            marker_offset: self.indent,
            tight: true,
            num: -1,
            ..Default::default()
        };
        let tokens = &ln[self.next_nonspace..];
        let first = *tokens.first()?;
        let mut marker_length = 1usize;
        let mut marker: Vec<u8> = vec![first];
        if first == b'+' || first == b'-' || first == b'*' {
            data.bullet_char = first;
        } else {
            let mut digits: Vec<u8> = Vec::new();
            let mut delimiter = 0u8;
            for (i, &token) in tokens.iter().enumerate() {
                if !token.is_ascii_digit() || i > 8 {
                    delimiter = token;
                    break;
                }
                digits.push(token);
            }
            if digits.is_empty() || (delimiter != b'.' && delimiter != b')') {
                return None;
            }
            if self.nodes[container].node_type == "NodeParagraph" && digits != b"1" {
                return None;
            }
            data.typ = 1;
            data.start = std::str::from_utf8(&digits).ok()?.parse().ok()?;
            marker_length = digits.len() + 1;
            data.delimiter = delimiter;
            marker = digits;
        }
        if data.typ == 1 {
            marker.push(b'.');
        }
        let token = *ln.get(self.next_nonspace + marker_length)?;
        if !is_whitespace(token) {
            return None;
        }
        if self.nodes[container].node_type == "NodeParagraph" && token == b'\n' {
            return None;
        }
        self.advance_next_nonspace();
        self.advance_offset(marker_length, true);
        let spaces_start_col = self.column;
        let spaces_start_offset = self.offset;
        loop {
            self.advance_offset(1, true);
            let next = self.current_line.get(self.offset).copied().unwrap_or(0);
            if self.column - spaces_start_col >= 5 || next == 0 || (next != b' ' && next != b'\t') {
                break;
            }
        }
        let next = self.current_line.get(self.offset).copied().unwrap_or(0);
        let is_blank_item = next == 0 || next == b'\n';
        let spaces_after_marker = self.column - spaces_start_col;
        if spaces_after_marker >= 5 || spaces_after_marker < 1 || is_blank_item {
            data.padding = marker_length + 1;
            self.column = spaces_start_col;
            self.offset = spaces_start_offset;
            if matches!(self.current_line.get(self.offset), Some(b' ') | Some(b'\t')) {
                self.advance_offset(1, true);
            }
        } else {
            data.padding = marker_length + spaces_after_marker;
        }
        data.marker = base64(&marker);
        Some(data)
    }
}

impl Parser {
    /// 对应 Lute 的 codeBlockFinalize：围栏块丢掉首行，缩进块裁掉尾部空行。
    fn code_block_finalize(&mut self, block: usize) {
        let fenced = self.nodes[block]
            .attrs
            .iter()
            .any(|(key, _)| *key == "IsFencedCodeBlock");
        if fenced {
            let tokens = self.nodes[block].tokens.clone();
            if let Some(index) = tokens.iter().position(|b| *b == b'\n') {
                self.nodes[block].tokens = tokens[index + 1..].to_vec();
            }
            return;
        }
        let tokens = self.nodes[block].tokens.clone();
        self.nodes[block].tokens = trim_trailing_newline_spaces(&tokens);
    }

    /// 摘出段落开头的链接引用定义，返回解析到的定义列表。
    fn extract_link_ref_defs(&mut self, block: usize) -> Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> {
        let mut defs: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> = Vec::new();
        loop {
            let tokens = self.nodes[block].tokens.clone();
            if tokens.first() != Some(&b'[') {
                break;
            }
            let Some((label, dest, title, consumed)) = inline::parse_link_ref_def(&tokens) else {
                break;
            };
            // 重复定义以第一个为准
            self.link_ref_defs
                .entry(inline::normalize_label(&label))
                .or_insert((dest.clone(), title.clone()));
            defs.push((label, dest, title));
            self.nodes[block].tokens = tokens[consumed..].to_vec();
        }
        defs
    }

    /// 每个引用定义各自建一个 NodeLinkRefDefBlock，追加到文档末尾
    /// （Lute 的 parseLinkRefDef 每次都新建块，Tip.Parent.AppendChild 到文档）。
    fn append_def_block(
        &mut self,
        defs: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)>,
        parent: Option<usize>,
    ) {
        for (label, dest, title) in defs {
            let def_block = self.new_node("NodeLinkRefDefBlock");
            let def = self.new_node("NodeLinkRefDef");
            self.nodes[def].tokens = label.clone();
            let link = self.new_node("NodeLink");
            self.nodes[link].attrs.push(("LinkType", json!(1)));
            self.nodes[link]
                .attrs
                .push(("LinkRefLabel", json!(base64(&label))));
            let text = self.new_node("NodeLinkText");
            self.nodes[text].tokens = label.clone();
            let dest_node = self.new_node("NodeLinkDest");
            self.nodes[dest_node].tokens = dest;
            let mut link_children = vec![
                self.new_node("NodeOpenBracket"),
                text,
                self.new_node("NodeCloseBracket"),
                self.new_node("NodeOpenParen"),
                dest_node,
            ];
            if let Some(title) = title {
                let title_node = self.new_node("NodeLinkTitle");
                self.nodes[title_node].tokens = title;
                link_children.push(title_node);
            }
            link_children.push(self.new_node("NodeCloseParen"));
            self.nodes[link].children = link_children;
            self.nodes[def].children = vec![link];
            self.nodes[def_block].children = vec![def];
            // 定义块挂到所在容器的父节点下（引用块、列表项内部同理）
            if let Some(parent) = parent {
                self.append_child(parent, def_block);
            } else {
                self.append_child(0, def_block);
            }
        }
    }

    /// 对应 paragraphFinalize：裁空白，摘出链接引用定义；段落空了就从父节点摘除。
    fn paragraph_finalize(&mut self, block: usize) {
        let tokens = trim_whitespace(&self.nodes[block].tokens);
        self.nodes[block].tokens = tokens;
        let defs = self.extract_link_ref_defs(block);
        if defs.is_empty() {
            return;
        }
        let remaining_blank = self.nodes[block]
            .tokens
            .iter()
            .all(|byte| byte.is_ascii_whitespace());
        let parent = self.nodes[block].parent;
        self.append_def_block(defs, parent);
        if remaining_blank {
            if let Some(parent) = self.nodes[block].parent {
                if let Some(position) = self.nodes[parent]
                    .children
                    .iter()
                    .position(|child| *child == block)
                {
                    self.nodes[parent].children.remove(position);
                }
            }
        }
    }

    /// 对应 Lute 的 listFinalize：项之间出现空行即松散的，并把结果同步给所有项。
    fn list_finalize(&mut self, list: usize) {
        let items = self.nodes[list].children.clone();
        let mut tight = true;
        for (index, item) in items.iter().enumerate() {
            if self.ends_with_blank_line(*item) && index + 1 < items.len() {
                tight = false;
                break;
            }
            let subs = self.nodes[*item].children.clone();
            for (sub_index, sub) in subs.iter().enumerate() {
                if self.ends_with_blank_line(*sub) && (index + 1 < items.len() || sub_index + 1 < subs.len()) {
                    tight = false;
                    break;
                }
            }
        }
        if let Some(data) = &mut self.nodes[list].list_data {
            data.tight = tight;
        }
        // Lute 里列表与首项共享同一份 ListData，其余项保留自己的默认值（紧凑）。
        if let Some(first) = items.first().copied() {
            if let Some(data) = &mut self.nodes[first].list_data {
                data.tight = tight;
            }
        }
    }

    /// 对应 Lute 的 endsWithBlankLine。
    fn ends_with_blank_line(&self, block: usize) -> bool {
        let mut current = block;
        loop {
            if self.nodes[current].last_line_blank {
                return true;
            }
            let node_type = self.nodes[current].node_type;
            if node_type == "NodeList" || node_type == "NodeListItem" {
                if let Some(last) = self.nodes[current].children.last() {
                    current = *last;
                    continue;
                }
            }
            return false;
        }
    }
}

impl Parser {
    /// 序列化成 Lute 的 JSON 结构。
    fn render(&self, index: usize) -> Value {
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

fn lists_match(list: &ListData, item: &ListData) -> bool {
    list.typ == item.typ
        && ((list.delimiter == 0 && item.delimiter == 0) || list.delimiter == item.delimiter)
        && list.bullet_char == item.bullet_char
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn strip_closing_crosshatch(content: &[u8]) -> Vec<u8> {
    let mut end = content.len();
    while end > 0 && matches!(content[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let mut hash_end = end;
    while hash_end > 0 && content[hash_end - 1] == b'#' {
        hash_end -= 1;
    }
    if hash_end == end {
        return content[..end].to_vec();
    }
    if hash_end == 0 {
        return Vec::new();
    }
    if matches!(content[hash_end - 1], b' ' | b'\t') {
        let mut trimmed = hash_end - 1;
        while trimmed > 0 && matches!(content[trimmed - 1], b' ' | b'\t') {
            trimmed -= 1;
        }
        return content[..trimmed].to_vec();
    }
    content[..end].to_vec()
}

/// 对应 Lute 的 ReplaceNewlineSpace：只裁掉尾部多余的换行/空格。
fn trim_trailing_newline_spaces(tokens: &[u8]) -> Vec<u8> {
    let mut end = tokens.len();
    while end >= 2 {
        let last = tokens[end - 1];
        if last != b'\n' && last != b' ' {
            break;
        }
        if tokens[end - 2] == b'\n' {
            end -= 1;
        } else {
            break;
        }
    }
    tokens[..end].to_vec()
}

fn trim_right_whitespace(tokens: &[u8]) -> &[u8] {
    let mut end = tokens.len();
    while end > 0 && matches!(tokens[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    &tokens[..end]
}

fn trim_whitespace(tokens: &[u8]) -> Vec<u8> {
    let mut start = 0usize;
    let mut end = tokens.len();
    while start < end && matches!(tokens[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    while end > start && matches!(tokens[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    tokens[start..end].to_vec()
}

fn trim_trailing_blank(tokens: &[u8]) -> Vec<u8> {
    let mut end = tokens.len();
    while end > 0 && matches!(tokens[end - 1], b'\n' | b'\r' | b' ' | b'\t') {
        end -= 1;
    }
    tokens[..end].to_vec()
}

fn fence_len_rest(rest: &[u8], ch: u8, len: usize) -> bool {
    let mut run = 0usize;
    while run < rest.len() && rest[run] == ch {
        run += 1;
    }
    run >= len && rest[run..].iter().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

fn decode_marker(marker: &str) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = marker.as_bytes();
    let mut out = String::new();
    let mut buffer = 0u32;
    let mut bits = 0usize;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        let Some(value) = TABLE.iter().position(|item| *item == b) else {
            continue;
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8 as char);
        }
    }
    out
}

/// 标准 base64（带填充）：Lute 把标记按 base64 存进字段。
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

impl Parser {
    /// 对应 HtmlBlockStart：判定 HTML 块类型并开块。
    fn html_block_start(&mut self, _container: usize) -> bool {
        if self.indented {
            return false;
        }
        if self.current_line.get(self.next_nonspace) != Some(&b'<') {
            return false;
        }
        let tokens = self.rest_of_line().to_vec();
        // 7 型（完整标签）只在不是段落延续时才成立，否则应留给行内 HTML。
        let tip_type = self
            .tip
            .map(|index| self.nodes[index].node_type)
            .unwrap_or("NodeDocument");
        let in_paragraph = tip_type == "NodeParagraph";
        let Some(html_type) = parse_html_block_type(&tokens, in_paragraph) else {
            return false;
        };
        self.close_unmatched_blocks();
        let block = self.add_child("NodeHTMLBlock");
        self.nodes[block].attrs.push(("HtmlBlockType", json!(html_type)));
        true
    }

    /// 对应 HtmlBlockContinue：6/7 型遇空行结束，其余靠闭合标记。
    fn html_block_continue(&mut self, node: usize) -> i32 {
        let html_type = self.attr_u64(node, "HtmlBlockType").unwrap_or(0);
        if (html_type == 6 || html_type == 7) && self.blank {
            return 1;
        }
        0
    }

    /// 对应 htmlBlockFinalize。
    fn html_block_finalize(&mut self, block: usize) {
        let tokens = self.nodes[block].tokens.clone();
        let trimmed = trim_trailing_newline_spaces(&tokens);
        self.nodes[block].tokens = trim_right_whitespace(&trimmed).to_vec();
    }

    /// 对应 SetextHeadingStart：段落后面跟 === 或 --- 就升级成标题。
    fn setext_heading_start(&mut self, container: usize) -> bool {
        if self.indented || self.nodes[container].node_type != "NodeParagraph" {
            return false;
        }
        let line = trim_whitespace(&self.current_line);
        if line.is_empty() {
            return false;
        }
        let first = line[0];
        if first != b'=' && first != b'-' {
            return false;
        }
        if !line.iter().all(|b| *b == first) {
            return false;
        }
        let level = if first == b'=' { 1 } else { 2 };
        self.close_unmatched_blocks();
        // 段落开头的链接引用定义先摘走，剩下的才是标题内容
        let defs = self.extract_link_ref_defs(container);
        if !defs.is_empty() {
            let parent = self.nodes[container].parent;
            self.append_def_block(defs, parent);
        }
        let tokens = trim_whitespace(&self.nodes[container].tokens);
        if tokens.is_empty() {
            return false;
        }
        self.nodes[container].node_type = "NodeHeading";
        self.nodes[container].tokens = tokens;
        self.nodes[container]
            .attrs
            .push(("HeadingLevel", json!(level)));
        self.nodes[container]
            .attrs
            .push(("HeadingSetext", json!(true)));
        self.tip = Some(container);
        self.advance_offset(self.current_line.len() - self.offset, false);
        true
    }
}

/// 对应 parseHTML：判断 HTML 块类型，None 表示不是。
///
/// in_paragraph 对应 Lute 里对 Context.Tip.Type != NodeParagraph 的检查：
/// 只有不在段落延续中，完整标签才算 HTML 块（否则留给行内 HTML）。
fn parse_html_block_type(tokens: &[u8], in_paragraph: bool) -> Option<u64> {
    let tokens = trim_left_whitespace(tokens);
    if tokens.len() < 3 || tokens[0] != b'<' {
        return None;
    }
    for tag in ["<script", "<pre", "<style", "<textarea"] {
        if let Some(rest) = starts_with_ignore_case(tokens, tag.as_bytes()) {
            if rest.first().is_some_and(|b| is_whitespace(*b) || *b == b'>') {
                return Some(1);
            }
        }
    }
    for tag in HTML_BLOCK_TAGS {
        // 块级标签按字节精确匹配（区分大小写，对应 Lute 的 AcceptTokenss）
        if let Some(rest) = tokens.strip_prefix(tag.as_bytes()) {
            if rest.first().is_some_and(|b| is_whitespace(*b) || *b == b'>') {
                return Some(6);
            }
            if rest.starts_with(b"/>") {
                return Some(6);
            }
        }
    }
    let tag = trim_whitespace(tokens);
    if !in_paragraph && (is_open_tag(&tag) || is_close_tag(&tag)) {
        return Some(7);
    }
    if tokens.starts_with(b"<!--") {
        return Some(2);
    }
    if tokens.starts_with(b"<?") {
        return Some(3);
    }
    if tokens.len() > 2 && tokens.starts_with(b"<!") {
        let following = &tokens[2..];
        if following[0].is_ascii_uppercase() {
            return Some(4);
        }
        if following.starts_with(b"[CDATA[") {
            return Some(5);
        }
    }
    None
}

fn starts_with_ignore_case<'a>(tokens: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if tokens.len() < prefix.len() {
        return None;
    }
    for (index, byte) in prefix.iter().enumerate() {
        if !tokens[index].eq_ignore_ascii_case(byte) {
            return None;
        }
    }
    Some(&tokens[prefix.len()..])
}

/// 对应 isOpenTag：形如 <a>、<a href="x">、<br /> 的完整开标签。
fn is_open_tag(tokens: &[u8]) -> bool {
    let length = tokens.len();
    if length < 3 {
        return false;
    }
    if tokens[0] != b'<' {
        return false;
    }
    if tokens[length - 1] != b'>' {
        return false;
    }
    let inner = if tokens[length - 2] == b'/' {
        &tokens[1..length - 2]
    } else {
        &tokens[1..length - 1]
    };
    if inner.is_empty() {
        return false;
    }
    // 小于号后面不能跟空白
    if is_whitespace(inner[0]) {
        return false;
    }
    let parts = split_whitespace(inner);
    let Some(name) = parts.first() else {
        return false;
    };
    if name.is_empty() || !name[0].is_ascii_alphabetic() {
        return false;
    }
    for byte in &name[1..] {
        if !is_ascii_letter_num_hyphen(*byte) {
            return false;
        }
    }
    for attr in &parts[1..] {
        if attr.len() <= 1 {
            continue;
        }
        let mut split = attr.splitn(2, |b| *b == b'=');
        let name = split.next().unwrap_or(&[]);
        if name.is_empty() {
            continue;
        }
        if !name[0].is_ascii_alphabetic() && name[0] != b'_' && name[0] != b':' {
            return false;
        }
        for byte in &name[1..] {
            if !(is_ascii_letter_num_hyphen(*byte)
                || *byte == b'_'
                || *byte == b'.'
                || *byte == b':')
            {
                return false;
            }
        }
        let Some(value) = split.next() else { continue };
        if value.len() >= 2 && value[0] == b'\'' && value[value.len() - 1] == b'\'' {
            let inner = &value[1..value.len() - 1];
            if inner.is_empty() || inner.contains(&b'\'') {
                return false;
            }
            continue;
        }
        if value.len() >= 2 && value[0] == b'"' && value[value.len() - 1] == b'"' {
            let inner = &value[1..value.len() - 1];
            if inner.is_empty() || inner.contains(&b'"') {
                return false;
            }
            continue;
        }
        if value.iter().any(|b| matches!(b, b' ' | b'\t' | b'\n')) {
            return false;
        }
        if value
            .iter()
            .any(|b| matches!(b, b'"' | b'\'' | b'=' | b'<' | b'>' | b'\x60'))
        {
            return false;
        }
    }
    true
}

/// 对应 isCloseTag：形如 </a> 的完整闭标签。
fn is_close_tag(tokens: &[u8]) -> bool {
    let tokens = trim_whitespace(tokens);
    let length = tokens.len();
    if length < 4 {
        return false;
    }
    if tokens[0] != b'<' || tokens[1] != b'/' {
        return false;
    }
    if tokens[length - 1] != b'>' {
        return false;
    }
    let inner = &tokens[2..length - 1];
    if inner.is_empty() {
        return false;
    }
    if !inner[0].is_ascii_alphabetic() {
        return false;
    }
    for byte in &inner[1..] {
        if !is_ascii_letter_num_hyphen(*byte) {
            return false;
        }
    }
    true
}

/// 对应 lex.SplitWhitespace。
fn split_whitespace(tokens: &[u8]) -> Vec<Vec<u8>> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    for byte in tokens {
        if is_whitespace(*byte) {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(*byte);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn is_ascii_letter_num_hyphen(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}


/// 对应 isHTMLBlockClose 的 1-5 型判定。
fn html_block_closes(tokens: &[u8], html_type: u64) -> bool {
    match html_type {
        1 => ["</script>", "</pre>", "</style>", "</textarea>"]
            .iter()
            .any(|tag| contains_ignore_case(tokens, tag.as_bytes())),
        2 => contains_sequence(tokens, b"-->"),
        3 => contains_sequence(tokens, b"?>"),
        4 => tokens.contains(&b'>'),
        5 => contains_sequence(tokens, b"]]>"),
        _ => false,
    }
}

fn contains_sequence(tokens: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || tokens.len() < needle.len() {
        return false;
    }
    (0..=tokens.len() - needle.len()).any(|start| &tokens[start..start + needle.len()] == needle)
}

fn contains_ignore_case(tokens: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || tokens.len() < needle.len() {
        return false;
    }
    (0..=tokens.len() - needle.len()).any(|start| {
        tokens[start..start + needle.len()]
            .iter()
            .zip(needle.iter())
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

fn trim_left_whitespace(tokens: &[u8]) -> &[u8] {
    let mut start = 0usize;
    while start < tokens.len() && matches!(tokens[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    &tokens[start..]
}

const HTML_BLOCK_TAGS: &[&str] = &[
    "<address", "<article", "<aside", "<base", "<basefont", "<blockquote", "<body", "<caption",
    "<center", "<col", "<colgroup", "<dd", "<details", "<dialog", "<dir", "<div", "<dl", "<dt",
    "<fieldset", "<figcaption", "<figure", "<footer", "<form", "<frame", "<frameset", "<h1", "<h2",
    "<h3", "<h4", "<h5", "<h6", "<head", "<header", "<hr", "<html", "<iframe", "<legend", "<li",
    "<link", "<main", "<menu", "<menuitem", "<nav", "<noframes", "<ol", "<optgroup", "<option",
    "<p", "<param", "<section", "<source", "<summary", "<table", "<tbody", "<td", "<tfoot", "<th",
    "<thead", "<title", "<tr", "<track", "<ul", "<video", "<audio",
    "</address", "</article", "</aside", "</base", "</basefont", "</blockquote", "</body",
    "</caption", "</center", "</col", "</colgroup", "</dd", "</details", "</dialog", "</dir",
    "</div", "</dl", "</dt", "</fieldset", "</figcaption", "</figure", "</footer", "</form",
    "</frame", "</frameset", "</h1", "</h2", "</h3", "</h4", "</h5", "</h6", "</head", "</header",
    "</hr", "</html", "</iframe", "</legend", "</li", "</link", "</main", "</menu", "</menuitem",
    "</nav", "</noframes", "</ol", "</optgroup", "</option", "</p", "</param", "</section",
    "</source", "</summary", "</table", "</tbody", "</td", "</tfoot", "</th", "</thead", "</title",
    "</tr", "</track", "</ul", "</video", "</audio",
];

