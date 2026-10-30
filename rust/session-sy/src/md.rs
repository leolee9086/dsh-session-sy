//! 复刻 Lute 的 markdown → 思源 AST 解析（块级部分）。
//!
//! 结构照搬 Lute：块级用「容器栈 + 逐行 incorporateLine」两阶段解析（对应
//! parse/blocks.go），行内解析在块解析完成后统一进行（对应 parse/inlines.go）。
//! 判据是 tests/commonmark-fixtures.json：同一段 markdown，Lute 产出什么 AST，
//! 这里就必须产出什么。

mod r#continue;
mod finalize;
mod html;
mod inline;
mod render;
mod start;
mod util;

use serde_json::Value;

use html::html_block_closes;

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







