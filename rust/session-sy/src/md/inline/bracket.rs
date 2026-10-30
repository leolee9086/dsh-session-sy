//! 方括号与链接组装（对应 Lute 的 parseOpenBracket / parseCloseBracket）。

use serde_json::json;

use super::link::{base64, parse_inline_link_dest, scan_link_label, scan_link_title};
use super::{is_whitespace, normalize_label, Bracket, Parser};

impl<'a> Parser<'a> {
    pub(super) fn push_text(&mut self, data: Vec<u8>) {
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
    pub(super) fn parse_open_bracket(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let node = self.new_node("NodeText");
        self.nodes[node].data = self.tokens[start..self.pos].to_vec();
        self.append(node);
        self.add_bracket(node, start, false);
    }

    /// 对应 parseBang：感叹号紧跟左方括号才算图片起始。
    pub(super) fn parse_bang(&mut self) {
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
    pub(super) fn parse_close_bracket(&mut self) {
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
            if let Some((passed_len, remains_offset, value)) =
                parse_inline_link_dest(&self.tokens[paren_pos..])
            {
                let after = paren_pos + passed_len;
                let remains_start = paren_pos + remains_offset;
                let ends_with_paren =
                    passed_len > 1 && self.tokens.get(after.wrapping_sub(1)).copied() == Some(b')');
                if ends_with_paren {
                    // 对应 Lute 的 `matched && 1 < len(remains)`：
                    // 用 remains[0] 和 remains[1] 判断后面跟的是什么。
                    // 注意 remains 起点可能与 passed 末尾重叠一个 `)`，不能混用。
                    let okay = if remains_start + 1 < self.tokens.len() {
                        !is_whitespace(self.tokens[remains_start])
                            && self.tokens.get(remains_start + 1).copied() != Some(b'"')
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
