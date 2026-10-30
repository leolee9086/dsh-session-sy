//! 各块类型的「能否从这里开始」判定（对应 Lute 各块的 Start 方法）。

use serde_json::json;

use super::util::{base64, is_whitespace, lists_match, strip_closing_crosshatch, trim_whitespace};
use super::{inline, ListData, Parser};

/// 对应 Options.IsValidTaskListItemMarker：默认只认空格、x、X。
fn is_valid_task_marker(marker: u8, arbitrary: bool) -> bool {
    marker == b' '
        || marker == b'x'
        || marker == b'X'
        || (arbitrary && marker != b'[' && marker != b']')
}

impl Parser {

    /// 对应 YamlFrontMatterStart：只在文档最开头、当前行恰好以 3 个 - 开头时成立。
    pub(super) fn yaml_front_matter_start(&mut self, _container: usize) -> bool {
        if !self.options.yaml_front_matter || self.indented || !self.nodes[0].children.is_empty() {
            return false;
        }
        if !self.yaml_front_matter_marker() {
            return false;
        }
        let node = self.new_node("NodeYamlFrontMatter");
        self.append_child(0, node);
        self.tip = Some(node);
        true
    }

    /// 当前行是否恰好由 3 个 - 开头（不看缩进，对应 parseYamlFrontMatter）。
    fn yaml_front_matter_marker(&self) -> bool {
        if self.current_line.first() != Some(&b'-') {
            return false;
        }
        let mut count = 0usize;
        while count < self.current_line.len() && self.current_line[count] == b'-' {
            count += 1;
        }
        count == 3
    }

    /// 对应 isYamlFrontMatterClose：同样要求恰好 3 个 -。
    pub(super) fn yaml_front_matter_close(&self) -> bool {
        self.yaml_front_matter_marker()
    }

    pub(super) fn atx_heading_start(&mut self, _container: usize) -> bool {
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

    pub(super) fn fence_code_block_start(&mut self, _container: usize) -> bool {
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

    pub(super) fn thematic_break_start(&mut self, _container: usize) -> bool {
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

    pub(super) fn blockquote_start(&mut self, _container: usize) -> bool {
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

    /// 对应 BlockQueryEmbedStart：行首 `{{ ... }}` 是一个内容块查询嵌入。
    pub(super) fn block_query_embed_start(&mut self, _container: usize) -> bool {
        if !self.options.block_ref || self.indented {
            return false;
        }
        let Some(script) = self.parse_block_query_embed() else {
            return false;
        };
        self.close_unmatched_blocks();
        let block = self.add_child("NodeBlockQueryEmbed");
        // 子节点依次是 {{ 查询脚本 }}，每个花括号都是独立的节点
        for node_type in ["NodeOpenBrace", "NodeOpenBrace"] {
            let child = self.new_node(node_type);
            self.append_child(block, child);
        }
        let script_node = self.new_node("NodeBlockQueryEmbedScript");
        self.nodes[script_node].tokens = script;
        self.append_child(block, script_node);
        for node_type in ["NodeCloseBrace", "NodeCloseBrace"] {
            let child = self.new_node(node_type);
            self.append_child(block, child);
        }
        true
    }

    /// 对应 parseBlockQueryEmbed：从行首 `{{` 到行尾 `}}`，中间是查询脚本。
    fn parse_block_query_embed(&self) -> Option<Vec<u8>> {
        let start = self.next_nonspace.min(self.current_line.len());
        let tokens = &self.current_line[start..];
        if !tokens.starts_with(b"{{") {
            return None;
        }
        let body = trim_whitespace(&tokens[2..]);
        if !body.ends_with(b"}}") {
            return None;
        }
        Some(trim_whitespace(&body[..body.len() - 2]))
    }

    /// 对应 MathBlockStart：行首至少两个 `$` 就是数学公式块的开始。
    pub(super) fn math_block_start(&mut self, _container: usize) -> bool {
        if self.indented {
            return false;
        }
        let rest = self.rest_of_line().to_vec();
        if rest.first().copied() != Some(b'$') {
            return false;
        }
        let mut len = 0usize;
        while len < rest.len() && rest[len] == b'$' {
            len += 1;
        }
        if len < 2 {
            return false;
        }
        // `$` 前可能存在的缩进要记下来，续行时按它跳过
        let offset = self.indent;
        self.close_unmatched_blocks();
        let block = self.add_child("NodeMathBlock");
        // Lute 这个字段带 json:",omitempty"，值为 0 时不会出现在输出里
        if offset > 0 {
            self.nodes[block]
                .attrs
                .push(("MathBlockDollarOffset", json!(offset)));
        }
        self.advance_next_nonspace();
        self.advance_offset(offset, false);
        true
    }

    pub(super) fn indent_code_block_start(&mut self, _container: usize) -> bool {
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
    pub(super) fn list_start(&mut self, container: usize) -> bool {
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
            self.nodes[list].list_data = Some(Box::new(list_data));
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
        self.nodes[item].list_data = Some(Box::new(item_data));
        true
    }

    /// 对应 Lute 的 parseListMarker：在行上推进 offset/column 计算 Padding 与 Marker。
    pub(super) fn parse_list_marker(&mut self, container: usize) -> Option<ListData> {
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
        // 判定任务列表项：标记之后紧接 [ ]、[x] 或 [X]
        if !is_blank_item && self.options.gfm_task_list_item {
            let content = &ln[self.offset..];
            if content.len() >= 3
                && content[0] == b'['
                && content[1] != b']'
                && content[2] == b']'
                && (content.len() == 3 || is_whitespace(content[3]))
                && is_valid_task_marker(content[1], self.options.arbitrary_task_list_item_marker)
            {
                data.typ = 3;
                data.checked = Some(content[1] != b' ');
            }
        }
        data.marker = base64(&marker);
        Some(data)
    }
}
