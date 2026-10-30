//! 内联公式与行内数学块，逐函数对照 Lute 的 `parse/inline_math.go`。

use super::{trim_whitespace, Parser};

impl<'a> Parser<'a> {
    /// 对应 `parseInlineMath`。
    pub(super) fn parse_inline_math(&mut self) {
        let tokens_len = self.tokens.len();
        if tokens_len < 3 || !self.options.inline_math {
            self.pos += 1;
            self.push_text(vec![b'$']);
            return;
        }

        let start_pos = self.pos;
        let mut block_start = start_pos;
        let mut dollars = 0usize;
        while block_start < tokens_len && self.tokens[block_start] == b'$' {
            dollars += 1;
            block_start += 1;
        }

        if dollars >= 2 {
            if self.options.protyle_wysiwyg {
                // Protyle 不允许从行级派生块级
                self.pos += 1;
                self.push_text(vec![b'$']);
                return;
            }
            // 尝试块级公式 $$...$$
            let mut matched = false;
            let mut block_end = block_start + dollars;
            while block_end < tokens_len {
                if self.tokens[block_end] == b'$'
                    && block_end < tokens_len - 1
                    && self.tokens[block_end + 1] == b'$'
                {
                    matched = true;
                    break;
                }
                block_end += 1;
            }
            if matched {
                let node = self.new_node("NodeMathBlock");
                let open = self.new_node("NodeMathBlockOpenMarker");
                let content = self.new_node("NodeMathBlockContent");
                self.nodes[content].data = self.tokens[block_start..block_end].to_vec();
                let close = self.new_node("NodeMathBlockCloseMarker");
                self.nodes[node].children = vec![open, content, close];
                self.append(node);
                self.pos = block_end + 2;
                return;
            }
        }

        // 起始 $ 后紧跟数字时默认不当公式
        if !self.options.inline_math_allow_digit_after_open_marker
            && tokens_len > start_pos + 1
            && self.tokens[start_pos + 1].is_ascii_digit()
        {
            self.pos += 3;
            if self.tokens.len() < start_pos + 3 {
                let text = self.tokens[start_pos..].to_vec();
                self.push_text(text);
                return;
            }
            let text = self.tokens[start_pos..start_pos + 3].to_vec();
            self.push_text(text);
            return;
        }

        let end_pos = self.match_inline_math_end(start_pos + 1);
        if end_pos < 1 {
            self.pos += 1;
            self.push_text(vec![b'$']);
            return;
        }

        if self.options.text_mark {
            let inner = &self.tokens[start_pos + 1..start_pos + end_pos as usize + 1];
            if inner.windows(5).any(|w| w == b"<span") {
                self.pos += 1;
                self.push_text(vec![b'$']);
                return;
            }
        }

        let end_pos = start_pos + end_pos as usize + 2;
        let content = self.tokens[start_pos + 1..end_pos - 1].to_vec();
        if trim_whitespace(&content).is_empty() {
            self.pos += 1;
            self.push_text(vec![b'$']);
            return;
        }

        let node = self.new_node("NodeInlineMath");
        let open = self.new_node("NodeInlineMathOpenMarker");
        let content_node = self.new_node("NodeInlineMathContent");
        self.nodes[content_node].data = content;
        let close = self.new_node("NodeInlineMathCloseMarker");
        self.nodes[node].children = vec![open, content_node, close];
        self.append(node);
        self.pos = end_pos;
    }

    /// 对应 `matchInlineMathEnd`：从 start 起找结尾 $，返回相对下标，找不到是 -1。
    fn match_inline_math_end(&self, start: usize) -> i64 {
        let tokens = &self.tokens[start..];
        let length = tokens.len();
        let mut pos = 0usize;
        while pos < length {
            if tokens[pos] == b'$' && pos > 0 && tokens[pos - 1] != b'\\' {
                if pos < length - 1 {
                    if !tokens[pos + 1].is_ascii_digit()
                        || self.options.inline_math_allow_digit_after_open_marker
                    {
                        return pos as i64;
                    }
                } else {
                    return pos as i64;
                }
            } else if tokens[pos] == b'\n' {
                return -1;
            }
            pos += 1;
        }
        -1
    }
}
