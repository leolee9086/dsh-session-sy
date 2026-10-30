//! 各块类型的「能否延续」判定（对应 Lute 各块的 Continue 方法）。

use serde_json::json;

use super::util::{base64, fence_len_rest, trim_right_whitespace};
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
