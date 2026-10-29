//! 普通文本、反斜杠、换行与行内代码（对应 Lute 的 text.go / code_span.go）。

use serde_json::json;

use super::{is_ascii_punct, is_marker, trim_right, Parser};

impl<'a> Parser<'a> {
    pub(super) fn parse_text(&mut self) {
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
    pub(super) fn parse_backslash(&mut self) {
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
    pub(super) fn parse_newline(&mut self) {
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
    pub(super) fn parse_code_span(&mut self) {
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
            close_marker_data.extend(std::iter::repeat(0x60u8 as u8).take(run));
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
}
