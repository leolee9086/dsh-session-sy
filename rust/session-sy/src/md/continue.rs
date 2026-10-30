//! 各块类型的「能否延续」判定（对应 Lute 各块的 Continue 方法）。

use serde_json::json;

use super::util::{base64, fence_len_rest, trim_right_whitespace, trim_whitespace};
use super::Parser;

impl Parser {
    /// 0 = 匹配成功，1 = 不匹配，2 = 已闭合（围栏代码块）。
    pub(super) fn continue_block(&mut self, node: usize) -> i32 {
        match self.nodes[node].node_type {
            "NodeParagraph" => {
                if self.blank {
                    1
                } else {
                    0
                }
            }
            "NodeCodeBlock" => self.code_block_continue(node),
            "NodeMathBlock" => self.math_block_continue(node),
            "NodeHTMLBlock" => self.html_block_continue(node),
            "NodeBlockquote" => self.blockquote_continue(node),
            "NodeListItem" => self.list_item_continue(node),
            "NodeFootnotesDef" => {
                // 对应 FootnotesContinue：空行继续，缩进不足 4 就结束
                if self.blank {
                    0
                } else if self.indent < 4 {
                    1
                } else {
                    self.advance_offset(4, true);
                    0
                }
            }
            "NodeHeading" | "NodeThematicBreak" => 1,
            "NodeYamlFrontMatter" => {
                if self.yaml_front_matter_close() {
                    self.finalize(node);
                    2
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

/// 对应 MathBlockContinue：遇到闭合行就收尾（返回 2），否则跳过 `$` 前可能的空格。
    fn math_block_continue(&mut self, node: usize) -> i32 {
        let rest = self.rest_of_line().to_vec();
        if self.indent <= 3 && is_math_block_close(&rest) {
            self.finalize(node);
            return 2;
        }
        let mut remaining = self.attr_u64(node, "MathBlockDollarOffset").unwrap_or(0) as usize;
        while remaining > 0 {
            match self.current_line.get(self.offset) {
                Some(b' ') | Some(b'\t') => {
                    self.advance_offset(1, true);
                    remaining -= 1;
                }
                _ => break,
            }
        }
        0
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
        // 只要两个缩进数字，不必把整个 ListData 连 String 一起 clone 一份——
        // 这个函数每行、每个列表项容器都会被调到。
        let Some(indent_needed) = self.nodes[node]
            .list_data
            .as_ref()
            .map(|data| data.marker_offset + data.padding)
        else {
            return 1;
        };
        if self.blank {
            if self.nodes[node].children.is_empty() {
                return 1;
            }
            self.advance_next_nonspace();
            return 0;
        }
        if self.indent >= indent_needed {
            self.advance_offset(indent_needed, true);
            return 0;
        }
        1
    }
}

/// 对应 isMathBlockClose：整行只能是两个以上的 `$`。
///
/// 注意照抄 Lute 的判断顺序：先看开头至少两个 `$`，再把首尾空白裁掉，
/// 剩下必须全是 `$`。所以 `$$ $$`（中间有空格）不算闭合。
fn is_math_block_close(tokens: &[u8]) -> bool {
    if tokens.first().copied() != Some(b'$') {
        return false;
    }
    let mut len = 0usize;
    while len < tokens.len() && tokens[len] == b'$' {
        len += 1;
    }
    if len < 2 {
        return false;
    }
    let trimmed = trim_whitespace(tokens);
    trimmed.iter().all(|&byte| byte == b'$')
}

