//! 强调与加粗的分隔符栈算法（对应 Lute 的 delimiter.go），以及文本节点合并。

use super::{scan_delims, Delim, Parser};

impl<'a> Parser<'a> {
    pub(super) fn handle_delim(&mut self, typ: u8) {
        let start = self.pos;
        let mut count = 0i32;
        while self.pos < self.tokens.len() && self.tokens[self.pos] == typ {
            // 对应 Lute：标签只用单个井号，连续井号不合并，
            // 否则相邻标签 #foo##bar# 会被粘成一个
            if typ == b'#' && self.options.tag && count >= 1 {
                break;
            }
            count += 1;
            self.pos += 1;
        }
        let (can_open, can_close) = scan_delims(&self.options, self.tokens, start, self.pos, typ);
        let node = self.new_node("NodeText");
        self.nodes[node].data = self.tokens[start..self.pos].to_vec();
        self.append(node);
        if can_open || can_close {
            let index = self.delims.len();
            self.delims.push(Delim {
                typ,
                num: count,
                original_num: count,
                unit_len: 1,
                node,
                can_open,
                can_close,
                prev: self.top,
                next: None,
            });
            if let Some(prev) = self.top {
                self.delims[prev].next = Some(index);
            }
            self.top = Some(index);
        }
    }

    fn remove_delimiter(&mut self, index: usize) {
        let prev = self.delims[index].prev;
        let next = self.delims[index].next;
        if let Some(prev) = prev {
            self.delims[prev].next = next;
        }
        match next {
            Some(next) => self.delims[next].prev = prev,
            None => self.top = prev,
        }
    }

    /// 对应 processEmphasis 里「找到 opener 之后」的一串前置条件：
    /// 各种标记的开关、数量必须一致。
    fn pair_allowed(&self, typ: u8, opener: usize, closer: usize) -> bool {
        let opener_num = self.delims[opener].num;
        let closer_num = self.delims[closer].num;
        match typ {
            // 标签只要求数量一致：Lute 不检查内容是否为空，
            // 所以 #**foo**# 里两个 # 之间只有一个 Strong 节点也能配对。
            b'#' => self.options.tag && opener_num == closer_num,
            b'^' => self.options.sup && opener_num == closer_num,
            b'=' => self.options.mark && opener_num == closer_num,
            b'~' => {
                (self.options.gfm_strikethrough || self.options.sub) && opener_num == closer_num
            }
            b'*' => self.options.inline_asterisk,
            b'_' => self.options.inline_underscore,
            _ => true,
        }
    }

    pub(super) fn find_root(&self, node: usize) -> Option<usize> {
        self.root.iter().position(|item| *item == node)
    }

    /// 在整棵树里找节点所在的父级与下标（None 表示顶层）。
    fn find_parent_and_index(&self, node: usize) -> Option<(Option<usize>, usize)> {
        if let Some(index) = self.root.iter().position(|item| *item == node) {
            return Some((None, index));
        }
        for (parent_index, parent) in self.nodes.iter().enumerate() {
            if let Some(index) = parent.children.iter().position(|item| *item == node) {
                return Some((Some(parent_index), index));
            }
        }
        None
    }

    /// 对应 processEmphasis（commonmark.js 的 process_emphasis）。
    /// stack_bottom 之上的分隔符才参与，链接构造时会以 opener.previousDelimiter 为界。
    pub(super) fn process_emphasis(&mut self, stack_bottom: Option<usize>) {
        let mut openers_bottom: [Option<usize>; 128] = [stack_bottom; 128];
        let mut closer = {
            let mut index = self.top;
            loop {
                match index {
                    Some(current) => {
                        if self.delims[current].prev == stack_bottom {
                            break;
                        }
                        index = self.delims[current].prev;
                    }
                    None => break,
                }
            }
            index
        };
        while let Some(current) = closer {
            let typ = self.delims[current].typ;
            if !self.delims[current].can_close {
                closer = self.delims[current].next;
                continue;
            }
            let mut opener = self.delims[current].prev;
            let mut found = false;
            let mut odd_match = false;
            while let Some(candidate) = opener {
                if Some(candidate) == openers_bottom[typ as usize] {
                    break;
                }
                odd_match = (self.delims[current].can_open || self.delims[candidate].can_close)
                    && self.delims[current].original_num % 3 != 0
                    && (self.delims[candidate].original_num + self.delims[current].original_num) % 3 == 0;
                if self.delims[candidate].typ == typ && self.delims[candidate].can_open && !odd_match {
                    found = true;
                    opener = Some(candidate);
                    break;
                }
                opener = self.delims[candidate].prev;
            }
            let old_closer = current;
            if !found {
                // Lute 只在「没找到 且 不是 oddMatch」时才设查找下限并回收该分隔符
                if !odd_match {
                    openers_bottom[typ as usize] = self.delims[old_closer].prev;
                    if !self.delims[old_closer].can_open {
                        self.remove_delimiter(old_closer);
                    }
                }
                closer = self.delims[current].next;
                continue;
            }
            let opener = opener.unwrap();
            // Lute 在找到 opener 之后还有一串开关与数量条件，全过不了就跳过这个 closer
            if !self.pair_allowed(typ, opener, current) {
                closer = self.delims[current].next;
                continue;
            }
            let use_delims = if self.delims[current].num >= 2 && self.delims[opener].num >= 2 {
                2
            } else {
                1
            };
            let opener_node = self.delims[opener].node;
            let closer_node = self.delims[current].node;
            self.delims[opener].num -= use_delims;
            self.delims[current].num -= use_delims;
            let opener_len = self.delims[opener].unit_len * use_delims as usize;
            let closer_len = self.delims[current].unit_len * use_delims as usize;
            let opener_tokens: Vec<u8> = {
                let node = &mut self.nodes[opener_node];
                let at = node.data.len().saturating_sub(opener_len);
                node.data.split_off(at)
            };
            let closer_tokens: Vec<u8> = {
                let node = &mut self.nodes[closer_node];
                let at = node.data.len().saturating_sub(closer_len);
                node.data.split_off(at)
            };

            let (wrapper, open_type, close_type) = match (typ, use_delims) {
                (b'*', 1) => ("NodeEmphasis", "NodeEmA6kOpenMarker", "NodeEmA6kCloseMarker"),
                (b'_', 1) => ("NodeEmphasis", "NodeEmU8eOpenMarker", "NodeEmU8eCloseMarker"),
                (b'*', _) => ("NodeStrong", "NodeStrongA6kOpenMarker", "NodeStrongA6kCloseMarker"),
                (b'_', _) => ("NodeStrong", "NodeStrongU8eOpenMarker", "NodeStrongU8eCloseMarker"),
                // 单个波浪号：下标优先于删除线
                (b'~', 1) if self.options.sub => ("NodeSub", "NodeSubOpenMarker", "NodeSubCloseMarker"),
                (b'~', 1) if self.options.gfm_strikethrough && self.options.gfm_strikethrough1 => (
                    "NodeStrikethrough",
                    "NodeStrikethrough1OpenMarker",
                    "NodeStrikethrough1CloseMarker",
                ),
                (b'=', 1) if self.options.mark => (
                    "NodeMark",
                    "NodeMark1OpenMarker",
                    "NodeMark1CloseMarker",
                ),
                (b'=', _) if self.options.mark => (
                    "NodeMark",
                    "NodeMark2OpenMarker",
                    "NodeMark2CloseMarker",
                ),
                (b'#', 1) if self.options.tag => ("NodeTag", "NodeTagOpenMarker", "NodeTagCloseMarker"),
                (b'^', 1) if self.options.sup => ("NodeSup", "NodeSupOpenMarker", "NodeSupCloseMarker"),
                (b'~', _) if self.options.gfm_strikethrough => (
                    "NodeStrikethrough",
                    "NodeStrikethrough2OpenMarker",
                    "NodeStrikethrough2CloseMarker",
                ),
                _ => {
                    closer = self.delims[current].next;
                    continue;
                }
            };

            let mark = self.new_node(wrapper);
            let open_marker = self.new_node(open_type);
            self.nodes[open_marker].data = opener_tokens;
            let close_marker = self.new_node(close_type);
            self.nodes[close_marker].data = closer_tokens;

            if let (Some((opener_parent, open_index)), Some((closer_parent, close_index))) = (
                self.find_parent_and_index(opener_node),
                self.find_parent_and_index(closer_node),
            ) {
                // 同一层级才搬移；跨层级说明嵌套结构已变，跳过
                if opener_parent == closer_parent && open_index < close_index {
                    let inner: Vec<usize> = match opener_parent {
                        None => self.root.drain(open_index + 1..close_index).collect(),
                        Some(parent) => {
                            self.nodes[parent].children.drain(open_index + 1..close_index).collect()
                        }
                    };
                    let mut children = vec![open_marker];
                    children.extend(inner);
                    children.push(close_marker);
                    self.nodes[mark].children = children;
                    match opener_parent {
                        None => self.root.insert(open_index + 1, mark),
                        Some(parent) => {
                            self.nodes[parent].children.insert(open_index + 1, mark);
                        }
                    }
                }
            }

            // 匹配成功后，opener 与 closer 之间的分隔符要从链上摘掉
            if self.delims[opener].next != Some(current) {
                self.delims[opener].next = Some(current);
                self.delims[current].prev = Some(opener);
            }

            if self.delims[opener].num == 0 {
                if let Some((parent, index)) = self.find_parent_and_index(opener_node) {
                    match parent {
                        None => {
                            self.root.remove(index);
                        }
                        Some(parent) => {
                            self.nodes[parent].children.remove(index);
                        }
                    }
                }
                self.remove_delimiter(opener);
            }
            if self.delims[current].num == 0 {
                if let Some((parent, index)) = self.find_parent_and_index(closer_node) {
                    match parent {
                        None => {
                            self.root.remove(index);
                        }
                        Some(parent) => {
                            self.nodes[parent].children.remove(index);
                        }
                    }
                }
                let next = self.delims[current].next;
                self.remove_delimiter(current);
                closer = next;
            }
        }
        // 处理结束后清掉本次范围内的分隔符（对应 Lute 末尾的 removeDelimiter 循环）
        while let Some(current) = self.top {
            if Some(current) == stack_bottom {
                break;
            }
            self.remove_delimiter(current);
        }
    }

    /// 合并文本时同样要把链接文本算进去（对应 Lute 的 mergeText）。
    fn is_text_like(node_type: &str) -> bool {
        node_type == "NodeText" || node_type == "NodeLinkText"
    }

    /// 对应 mergeText：递归合并相邻的文本节点。
    pub(super) fn merge_text(&mut self) {
        self.merge_root_text();
        let roots = self.root.clone();
        for node in roots {
            self.merge_descendant_text(node);
        }
    }

    fn merge_root_text(&mut self) {
        let mut index = 0usize;
        while index < self.root.len() {
            let current = self.root[index];
            if !Self::is_text_like(self.nodes[current].node_type) {
                index += 1;
                continue;
            }
            while index + 1 < self.root.len() {
                let next = self.root[index + 1];
                if self.nodes[next].node_type != self.nodes[current].node_type {
                    break;
                }
                let extra = self.nodes[next].data.clone();
                self.nodes[current].data.extend_from_slice(&extra);
                self.root.remove(index + 1);
            }
            index += 1;
        }
    }

    fn merge_descendant_text(&mut self, node: usize) {
        let mut index = 0usize;
        loop {
            let length = self.nodes[node].children.len();
            if index >= length {
                break;
            }
            let current = self.nodes[node].children[index];
            if !Self::is_text_like(self.nodes[current].node_type) {
                index += 1;
                continue;
            }
            loop {
                let length = self.nodes[node].children.len();
                if index + 1 >= length {
                    break;
                }
                let next = self.nodes[node].children[index + 1];
                if self.nodes[next].node_type != self.nodes[current].node_type {
                    break;
                }
                let extra = self.nodes[next].data.clone();
                self.nodes[current].data.extend_from_slice(&extra);
                self.nodes[node].children.remove(index + 1);
            }
            index += 1;
        }
        let children = self.nodes[node].children.clone();
        for child in children {
            self.merge_descendant_text(child);
        }
    }
}
