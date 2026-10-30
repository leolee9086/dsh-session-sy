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
mod ial;
mod options;
mod render;
mod start;
mod table;
mod util;

use serde_json::Value;

use html::html_block_closes;
pub use options::Options;

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
    /// 列表数据。放 Box 里：nodes 是最热的数组，内嵌 ListData 会把每个元素
    /// 撑大 60 多字节，而只有列表/列表项节点才真的用它。
    list_data: Option<Box<ListData>>,
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

    /// 引擎配置，决定哪些语法被启用。对应 Lute 的 ParseOption。
    options: Options,
}

/// 解析 markdown，直接输出 Lute 风格的 JSON 文本。
///
/// 这是最快路径：不构造 serde_json::Value 树，直接把 JSON 写进一个 String。
/// Value 树里每个 key 都是一次堆分配，一个段落节点要十几次；直出把这部分
/// 摊平成一次缓冲区追加。
pub fn parse_to_json(src: &str) -> String {
    parse_to_json_with(src, &Options::commonmark())
}

/// 用指定配置解析，直接输出 JSON 文本。
pub fn parse_to_json_with(src: &str, options: &Options) -> String {
    let mut parser = Parser::new(options.clone());
    let root = parser.add_root();
    parser.tip = Some(root);
    parser.parse_blocks(src);
    // 段落形式的块级 IAL 要在行内解析前并到前一个块上
    parser.merge_paragraph_ial();
    // 预留：Lute 风格的 JSON 一般是原文的 4-17 倍（列表/内联比纯文本膨胀得多）。
    // 取 8 倍折中，不够时 String 自己翻倍，总 memcpy 也就两倍最终大小。
    let mut out = String::with_capacity(src.len() * 8 + 256);
    parser.write_json(root, &mut out);
    out
}

/// 解析 markdown，返回 NodeDocument。
///
/// 需要操作树（往文档里塞节点）时用这个；只想要 JSON 文本时用 `parse_to_json`。
pub fn parse(src: &str) -> Value {
    parse_with(src, &Options::commonmark())
}

/// 用指定配置解析，返回 NodeDocument。
pub fn parse_with(src: &str, options: &Options) -> Value {
    let mut parser = Parser::new(options.clone());
    let root = parser.add_root();
    parser.tip = Some(root);
    parser.parse_blocks(src);
    // 段落形式的块级 IAL 要在行内解析前并到前一个块上
    parser.merge_paragraph_ial();
    parser.render(root)
}

/// 对应 Lute 的 `lex.NewLexer` + `Lexer.NextLine` 对输入的预处理：
///
/// - 末尾没有换行就补一个；
/// - `\r` 换成 `\n`，`\r\n` 只算一个换行；
/// - NUL 字节换成 U+FFFD。
///
/// 绝大多数输入（尤其是我们自己生成的）不需要动，这时直接借用原文，零拷贝。
fn normalize_source(src: &str) -> std::borrow::Cow<'_, str> {
    let needs_fixup = src.bytes().any(|b| b == b'\r' || b == 0) || !src.ends_with('\n');
    if !needs_fixup {
        return std::borrow::Cow::Borrowed(src);
    }
    let mut out = String::with_capacity(src.len() + 1);
    let mut chars = src.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                out.push('\n');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\0' => out.push('\u{FFFD}'),
            other => out.push(other),
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    std::borrow::Cow::Owned(out)
}

impl Parser {
    fn new(options: Options) -> Self {
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
            options,
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
        // 对应 Lute 的 lex.Lexer 预处理：CRLF 规范化、NUL 替换、末尾补换行。
        let normalized = normalize_source(src);
        let src: &str = &normalized;
        // 预留节点容量：节点数组是全程最热的分配，按经验每 12 字节原文约一个节点。
        // 一次给足，省掉十几轮翻倍扩容和整表 memcpy。
        self.nodes.reserve(src.len() / 12);
        // 行缓冲也先按最长的一行给个底，后面每行都复用这块内存。
        self.current_line.reserve(256);
        // 逐行切片直接喂进去，不为每行做一次 Vec 分配：
        // 大文档动辄上万行，每行一次 malloc 会变成主导成本。
        for line in src.split_inclusive('\n') {
            self.incorporate_line(line.as_bytes());
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
        // 复用 current_line 的容量：clear 不释放堆内存，后面的行直接覆盖写入。
        self.current_line.clear();
        self.current_line.extend_from_slice(line);

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
        if self.yaml_front_matter_start(container) {
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

/// 对应 walkParseInline 里对段落 IAL 的处理：以 `{: ` 开头的段落会把属性
    /// 合并到前一个兄弟块，然后自身消失。
    ///
    /// 注意 Lute 这一处**不检查** `KramdownBlockIAL` 开关，所以 default 配置下也生效。
    fn merge_paragraph_ial(&mut self) {
        self.merge_paragraph_ial_in(0);
    }

    fn merge_paragraph_ial_in(&mut self, parent: usize) {
        let mut index = 0usize;
        while index < self.nodes[parent].children.len() {
            let child = self.nodes[parent].children[index];
            if self.nodes[child].node_type == "NodeParagraph" && index > 0 {
                let tokens = self.nodes[child].tokens.clone();
                if let Some(pairs) = ial::parse_block_ial(&tokens) {
                    let previous = self.nodes[parent].children[index - 1];
                    self.set_ial_attrs(previous, &pairs);
                    self.nodes[parent].children.remove(index);
                    continue;
                }
            }
            index += 1;
        }
        // 递归处理容器块内部
        let children = self.nodes[parent].children.clone();
        for child in children {
            if !self.nodes[child].children.is_empty() {
                self.merge_paragraph_ial_in(child);
            }
        }
    }

    /// 把 IAL 键值合并进节点的 `Properties` 属性（同名覆盖）。
    fn set_ial_attrs(&mut self, node: usize, pairs: &[(Vec<u8>, Vec<u8>)]) {
        let mut map = match self.nodes[node]
            .attrs
            .iter()
            .find(|(key, _)| *key == "Properties")
        {
            Some((_, value)) => value.as_object().cloned().unwrap_or_default(),
            None => serde_json::Map::new(),
        };
        for (key, value) in pairs {
            map.insert(
                String::from_utf8_lossy(key).to_string(),
                Value::String(String::from_utf8_lossy(value).to_string()),
            );
        }
        self.nodes[node]
            .attrs
            .retain(|(key, _)| *key != "Properties");
        self.nodes[node]
            .attrs
            .push(("Properties", Value::Object(map)));
    }

    /// 对应 ast.Node.AcceptLines：能吸收原始行的块类型。
    fn accepts_lines(&self, index: usize) -> bool {
        matches!(
            self.nodes[index].node_type,
            "NodeParagraph"
                | "NodeCodeBlock"
                | "NodeHTMLBlock"
                | "NodeMathBlock"
                | "NodeYamlFrontMatter"
                | "NodeBlockQueryEmbed"
                | "NodeGitConflict"
                | "NodeIFrame"
                | "NodeWidget"
                | "NodeVideo"
                | "NodeAudio"
                | "NodeAttributeView"
                | "NodeCustomBlock"
        )
    }

    fn add_line(&mut self) {
        let Some(tip) = self.tip else { return };
        let mut start = self.offset;
        if self.partially_consumed_tab {
            start += 1;
            let spaces = 4 - (self.column % 4);
            let tokens = &mut self.nodes[tip].tokens;
            // 首次写入时一次给足：Vec 默认从 8 字节起步，一个 24 字节的段落
            // 要经历 8→16→32 三次分配，而绝大多数块只有一两行。
            if tokens.capacity() == 0 {
                tokens.reserve(spaces + 32);
            }
            tokens.extend(std::iter::repeat(b' ').take(spaces));
        }
        if start < self.current_line.len() {
            // 字段级分借：nodes 要可变、current_line 只读，避免再复制一份临时 Vec。
            let tail_len = self.current_line.len() - start;
            let nodes = &mut self.nodes;
            let tokens = &mut nodes[tip].tokens;
            if tokens.capacity() == 0 {
                tokens.reserve(tail_len + 32);
            }
            tokens.extend_from_slice(&self.current_line[start..]);
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
            "NodeYamlFrontMatter" => self.yaml_front_matter_finalize(block),
            "NodeParagraph" => {
                // 段落可能被解析成表格，那时 Tip 已经指向表节点，不能再复位
                if self.paragraph_finalize(block) {
                    return;
                }
            }
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








/// 仅供基准诊断：只跑块解析这一段，返回纳秒数。
///
/// `examples/bench_stages.rs` 用它把「块解析」和「JSON 渲染」分开看。
/// 正常路径不经过这里，改解析逻辑时不需要管它。
#[doc(hidden)]
pub fn bench_blocks_nanos(src: &str) -> u64 {
    let mut parser = Parser::new(Options::commonmark());
    let root = parser.add_root();
    parser.tip = Some(root);
    let start = std::time::Instant::now();
    parser.parse_blocks(src);
    let blocks = start.elapsed().as_nanos() as u64;
    // 防止整段被优化掉
    std::hint::black_box(&parser.nodes);
    blocks
}