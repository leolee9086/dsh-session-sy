//! HTML 块的类型判定与标签校验（对应 Lute 的 html_block.go 里的纯函数部分）。

use serde_json::json;

use super::util::{is_whitespace, trim_right_whitespace, trim_trailing_newline_spaces, trim_whitespace};
use super::Parser;

/// 对应 lex.AcceptTokens：前缀精确匹配（区分大小写），返回匹配的标签长度。
fn accept_tokens(remains: &[u8], tag: &[u8]) -> Option<usize> {
    if remains.len() < tag.len() || &remains[..tag.len()] != tag {
        return None;
    }
    Some(tag.len())
}

/// 对应 lex.AcceptTokenss：在整行的任意位置找这些标签。
///
/// 注意它返回的是**标签长度**，不是命中的位置——Lute 随后拿这个长度去索引
/// 原 tokens 判断边界，所以这里必须照抄，不能改成返回位置。
fn accept_tokenss(tokens: &[u8], tags: &[&[u8]]) -> Option<usize> {
    for i in 0..tokens.len() {
        let remains = &tokens[i..];
        for tag in tags {
            if let Some(len) = accept_tokens(remains, tag) {
                return Some(len);
            }
        }
    }
    None
}
/// 对应 HtmlBlockStart 的判定：给一行内容，返回它是哪一型 HTML 块。
pub(super) fn parse_html_block_type(tokens: &[u8], in_paragraph: bool) -> Option<u64> {
    let tokens = trim_left_whitespace(tokens);
    if tokens.len() < 3 || tokens[0] != b'<' {
        return None;
    }
    // 这两组标签都是「整行任意位置出现」就算数，对应 AcceptTokenss。
    // 例如 `<div ...><pre>echo</pre></div>` 里有 `<pre`，所以这一行是 1 型块，
    // 不是 6 型——Lute 就是这么判的。
    if let Some(len) = accept_tokenss(tokens, &[b"<script", b"<pre", b"<style", b"<textarea"]) {
        if tokens
            .get(len)
            .is_some_and(|b| is_whitespace(*b) || *b == b'>')
        {
            return Some(1);
        }
    }
    if let Some(len) = accept_tokenss(
        tokens,
        &HTML_BLOCK_TAGS.iter().map(|tag| tag.as_bytes()).collect::<Vec<_>>(),
    ) {
        if tokens
            .get(len)
            .is_some_and(|b| is_whitespace(*b) || *b == b'>')
        {
            return Some(6);
        }
        if tokens.get(len) == Some(&b'/') && tokens.get(len + 1) == Some(&b'>') {
            return Some(6);
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


/// 对应 isOpenTag：形如 <a>、<a href="x">、<br /> 的完整开标签。
pub(super) fn is_open_tag(tokens: &[u8]) -> bool {
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
pub(super) fn is_close_tag(tokens: &[u8]) -> bool {
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
pub(super) fn split_whitespace(tokens: &[u8]) -> Vec<Vec<u8>> {
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

pub(super) fn is_ascii_letter_num_hyphen(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// 对应 isHTMLBlockClose 的 1-5 型判定。
pub(super) fn html_block_closes(tokens: &[u8], html_type: u64) -> bool {
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

pub(super) fn contains_sequence(tokens: &[u8], needle: &[u8]) -> bool {
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

pub(super) fn trim_left_whitespace(tokens: &[u8]) -> &[u8] {
    let mut start = 0usize;
    while start < tokens.len() && matches!(tokens[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    &tokens[start..]
}

pub(super) const HTML_BLOCK_TAGS: &[&str] = &[
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

impl Parser {
    /// 对应 HtmlBlockStart：判定 HTML 块类型并开块。
    pub(super) fn html_block_start(&mut self, _container: usize) -> bool {
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
    pub(super) fn html_block_continue(&mut self, node: usize) -> i32 {
        let html_type = self.attr_u64(node, "HtmlBlockType").unwrap_or(0);
        if (html_type == 6 || html_type == 7) && self.blank {
            return 1;
        }
        0
    }

    /// 对应 htmlBlockFinalize。
    pub(super) fn html_block_finalize(&mut self, block: usize) {
        let tokens = self.nodes[block].tokens.clone();
        let trimmed = trim_trailing_newline_spaces(&tokens);
        self.nodes[block].tokens = trim_right_whitespace(&trimmed).to_vec();
    }

    /// 对应 SetextHeadingStart：段落后面跟 === 或 --- 就升级成标题。
    pub(super) fn setext_heading_start(&mut self, container: usize) -> bool {
        if self.indented || self.nodes[container].node_type != "NodeParagraph" || !self.options.setext
        {
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

        // Lute 在把段落升级成标题之前，先试着把它解析成表格。
        // 「0\n-:\n1\n-」这种，前两行本身就能构成一个单列表格，
        // 「| foo |\n| :-: |\n| bar |\n=======」也是表格优先。
        // 成功就返回 false——当前这行没被消费，交给后面的列表等分支处理。
        if self.options.gfm_table && self.try_parse_table(container) {
            // 对应 Lute SetextHeadingStart 里的 container.Tokens = nil：
            // 走这条路变成的表不带 Data。
            let table = self.tip.unwrap_or(container);
            self.nodes[table].tokens.clear();
            return false;
        }

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
