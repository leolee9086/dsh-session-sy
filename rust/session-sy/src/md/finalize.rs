//! 各块类型的收尾处理（对应 Lute 各块的 Finalize 方法）。

use serde_json::json;

use super::util::{base64, is_whitespace, trim_trailing_newline_spaces, trim_whitespace};
use super::{inline, Parser};

impl Parser {
/// 对应 yamlFrontMatterFinalize：剔除首尾的 ---，内容进三个子节点。
    pub(super) fn yaml_front_matter_finalize(&mut self, node: usize) {
        let tokens = self.nodes[node].tokens.clone();
        // 开头是 "---\n"，占 4 个字节；这里按 Lute 的 tokens[3:] 处理
        let mut body: Vec<u8> = if tokens.len() >= 3 {
            tokens[3..].to_vec()
        } else {
            Vec::new()
        };
        body = trim_whitespace(&body);
        if body.ends_with(b"---") {
            body.truncate(body.len() - 3);
        }
        self.nodes[node].tokens = body.clone();
        let open = self.new_node("NodeYamlFrontMatterOpenMarker");
        let content = self.new_node("NodeYamlFrontMatterContent");
        self.nodes[content].tokens = body;
        let close = self.new_node("NodeYamlFrontMatterCloseMarker");
        self.append_child(node, open);
        self.append_child(node, content);
        self.append_child(node, close);
    }

    /// 对应 mathBlockFinalize：把 `$$ ... $$` 拆成 开标记 / 内容 / 关标记 三个子节点。
    pub(super) fn math_block_finalize(&mut self, node: usize) {
        let tokens = self.nodes[node].tokens.clone();
        if tokens.len() < 2 {
            // 空内容也照样生成三个子节点，但 Tokens 保留原样（Lute 这一支不改 Tokens）
            let open = self.new_node("NodeMathBlockOpenMarker");
            let content = self.new_node("NodeMathBlockContent");
            let close = self.new_node("NodeMathBlockCloseMarker");
            self.append_child(node, open);
            self.append_child(node, content);
            self.append_child(node, close);
            return;
        }
        let mut body = trim_whitespace(&tokens[2..]);
        if body.ends_with(b"$$") {
            body.truncate(body.len() - 2);
        }
        self.nodes[node].tokens = Vec::new();
        let open = self.new_node("NodeMathBlockOpenMarker");
        let content = self.new_node("NodeMathBlockContent");
        self.nodes[content].tokens = body;
        let close = self.new_node("NodeMathBlockCloseMarker");
        self.append_child(node, open);
        self.append_child(node, content);
        self.append_child(node, close);
    }

    /// 对应 Lute 的 codeBlockFinalize：围栏块丢掉首行，缩进块裁掉尾部空行。
    pub(super) fn code_block_finalize(&mut self, block: usize) {
        let fenced = self.nodes[block]
            .attrs
            .iter()
            .any(|(key, _)| *key == "IsFencedCodeBlock");
        if fenced {
            let tokens = self.nodes[block].tokens.clone();
            if let Some(index) = tokens.iter().position(|b| *b == b'\n') {
                self.nodes[block].tokens = tokens[index + 1..].to_vec();
            }
            return;
        }
        let tokens = self.nodes[block].tokens.clone();
        self.nodes[block].tokens = trim_trailing_newline_spaces(&tokens);
    }

    /// 摘出段落开头的链接引用定义，返回解析到的定义列表。
    pub(super) fn extract_link_ref_defs(&mut self, block: usize) -> Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> {
        let mut defs: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> = Vec::new();
        loop {
            let tokens = self.nodes[block].tokens.clone();
            if tokens.first() != Some(&b'[') {
                break;
            }
            let Some((label, dest, title, consumed)) = inline::parse_link_ref_def(&tokens) else {
                break;
            };
            // 重复定义以第一个为准
            self.link_ref_defs
                .entry(inline::normalize_label(&label))
                .or_insert((dest.clone(), title.clone()));
            defs.push((label, dest, title));
            self.nodes[block].tokens = tokens[consumed..].to_vec();
        }
        defs
    }

    /// 每个引用定义各自建一个 NodeLinkRefDefBlock，追加到文档末尾
    /// （Lute 的 parseLinkRefDef 每次都新建块，Tip.Parent.AppendChild 到文档）。
    pub(super) fn append_def_block(
        &mut self,
        defs: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)>,
        parent: Option<usize>,
    ) {
        for (label, dest, title) in defs {
            let def_block = self.new_node("NodeLinkRefDefBlock");
            let def = self.new_node("NodeLinkRefDef");
            self.nodes[def].tokens = label.clone();
            let link = self.new_node("NodeLink");
            self.nodes[link].attrs.push(("LinkType", json!(1)));
            self.nodes[link]
                .attrs
                .push(("LinkRefLabel", json!(base64(&label))));
            let text = self.new_node("NodeLinkText");
            self.nodes[text].tokens = label.clone();
            let dest_node = self.new_node("NodeLinkDest");
            self.nodes[dest_node].tokens = dest;
            let mut link_children = vec![
                self.new_node("NodeOpenBracket"),
                text,
                self.new_node("NodeCloseBracket"),
                self.new_node("NodeOpenParen"),
                dest_node,
            ];
            if let Some(title) = title {
                let title_node = self.new_node("NodeLinkTitle");
                self.nodes[title_node].tokens = title;
                link_children.push(title_node);
            }
            link_children.push(self.new_node("NodeCloseParen"));
            self.nodes[link].children = link_children;
            self.nodes[def].children = vec![link];
            self.nodes[def_block].children = vec![def];
            // 定义块挂到所在容器的父节点下（引用块、列表项内部同理）
            if let Some(parent) = parent {
                self.append_child(parent, def_block);
            } else {
                self.append_child(0, def_block);
            }
        }
    }

    /// 对应 paragraphFinalize：裁空白，摘出链接引用定义；段落空了就从父节点摘除。
    /// 返回 true 表示这一段已经变成表格，调用方不要再把 Tip 复位。
    /// 对应 Lute paragraphFinalize 里的任务列表处理。
    ///
    /// 段落是任务列表项的首孩子时，把开头的 `[ ]` 摘成独立节点；如果剩余内容
    /// 重新解析后不再是段落（引用、标题这类），就把列表项改造成
    /// `[标记, 一个空格文本, 新块]` 的形状——Lute 就是这么处理
    /// `- [ ] > [!NOTE]` 的。
    fn split_task_list_marker(&mut self, block: usize) -> bool {
        if !self.options.gfm_task_list_item {
            return false;
        }
        let Some(parent) = self.nodes[block].parent else {
            return false;
        };
        if self.nodes[parent].node_type != "NodeListItem" {
            return false;
        }
        let Some(data) = self.nodes[parent].list_data.as_ref() else {
            return false;
        };
        if data.typ != 3 {
            return false;
        }
        if self.nodes[parent].children.first() != Some(&block) {
            return false;
        }
        let tokens = self.nodes[block].tokens.clone();
        // 至少要有三个标记字节加一个空白字符
        if tokens.len() <= 3 {
            return false;
        }
        if tokens[0] != b'[' || tokens[2] != b']' || tokens[1] == b']' {
            return false;
        }
        if !is_whitespace(tokens[3]) {
            return false;
        }
        let marker = tokens[1];
        // 对应 IsValidTaskListItemMarker：开了任意标记就接受方括号以外的任意字符
        let valid = matches!(marker, b' ' | b'x' | b'X')
            || (self.options.arbitrary_task_list_item_marker
                && marker != b'['
                && marker != b']');
        if !valid {
            return false;
        }
        // 摘掉开头的 [ ]、[x] 或 [X]
        self.nodes[block].tokens = tokens[3..].to_vec();

        // 段落里先放上标记节点，对应 Lute 的 PrependChild
        let marker_node = self.new_node("NodeTaskListItemMarker");
        self.nodes[marker_node].tokens = vec![b'[', marker, b']'];
        if marker != b' ' {
            self.nodes[marker_node]
                .attrs
                .push(("TaskListItemChecked", json!(true)));
        }
        self.nodes[marker_node]
            .attrs
            .push(("TaskListItemMarker", json!(marker)));
        self.nodes[block].children.insert(0, marker_node);
        self.nodes[marker_node].parent = Some(block);

        let rest = self.nodes[block].tokens.clone();
        if rest.is_empty() {
            return false;
        }
        let rest_text = String::from_utf8_lossy(&rest).to_string();
        let (sub, sub_root) = super::parse_into(&rest_text, &self.options);
        let sub_children = sub.nodes[sub_root].children.clone();
        let Some(&first) = sub_children.first() else {
            return false;
        };
        if sub.nodes[first].node_type == "NodeParagraph" {
            return false;
        }

        // 1. 列表项最前面放一个空格文本
        let space = self.new_node("NodeText");
        self.nodes[space].tokens = b" ".to_vec();
        self.nodes[parent].children.insert(0, space);
        self.nodes[space].parent = Some(parent);
        // 2. 段落已有的孩子（就是刚放的标记）挪到空格前面
        let own = self.nodes[block].children.clone();
        for (offset, child) in own.iter().enumerate() {
            self.nodes[parent].children.insert(offset, *child);
            self.nodes[*child].parent = Some(parent);
        }
        self.nodes[block].children.clear();
        // 3. 新块接到段落后面，然后把段落自己摘掉
        let Some(position) = self.nodes[parent]
            .children
            .iter()
            .position(|child| *child == block)
        else {
            return true;
        };
        let mut insert_at = position + 1;
        for child in sub_children {
            let copy = self.graft(&sub, child);
            self.nodes[parent].children.insert(insert_at, copy);
            self.nodes[copy].parent = Some(parent);
            insert_at += 1;
        }
        if let Some(position) = self.nodes[parent]
            .children
            .iter()
            .position(|child| *child == block)
        {
            self.nodes[parent].children.remove(position);
        }
        true
    }

    pub(super) fn paragraph_finalize(&mut self, block: usize) -> bool {
        let tokens = trim_whitespace(&self.nodes[block].tokens);
        self.nodes[block].tokens = tokens;
        let defs = self.extract_link_ref_defs(block);
        if !defs.is_empty() {
            let remaining_blank = self.nodes[block]
                .tokens
                .iter()
                .all(|byte| byte.is_ascii_whitespace());
            let parent = self.nodes[block].parent;
            self.append_def_block(defs, parent);
            if remaining_blank {
                if let Some(parent) = self.nodes[block].parent {
                    if let Some(position) = self.nodes[parent]
                        .children
                        .iter()
                        .position(|child| *child == block)
                    {
                        self.nodes[parent].children.remove(position);
                    }
                }
            }
        }

        // 任务列表项：段落收尾时把开头的 [ ] 摘成独立节点
        if self.split_task_list_marker(block) {
            return false;
        }

        // Lute 在 paragraphFinalize 末尾尝试把段落解析成表格
        if self.options.gfm_table && self.try_parse_table(block) {
            return true;
        }
        false
    }

    /// 对应 Lute 的 listFinalize：项之间出现空行即松散的，并把结果同步给首项。
    pub(super) fn list_finalize(&mut self, list: usize) {
        let items = self.nodes[list].children.clone();
        let mut tight = true;
        for (index, item) in items.iter().enumerate() {
            if self.ends_with_blank_line(*item) && index + 1 < items.len() {
                tight = false;
                break;
            }
            let subs = self.nodes[*item].children.clone();
            for (sub_index, sub) in subs.iter().enumerate() {
                if self.ends_with_blank_line(*sub) && (index + 1 < items.len() || sub_index + 1 < subs.len()) {
                    tight = false;
                    break;
                }
            }
        }
        if let Some(data) = &mut self.nodes[list].list_data {
            data.tight = tight;
        }
        // Lute 里列表与首项共享同一份 ListData，其余项保留自己的默认值（紧凑）。
        if let Some(first) = items.first().copied() {
            if let Some(data) = &mut self.nodes[first].list_data {
                data.tight = tight;
            }
        }
    }

    /// 对应 Lute 的 endsWithBlankLine。
    fn ends_with_blank_line(&self, block: usize) -> bool {
        let mut current = block;
        loop {
            if self.nodes[current].last_line_blank {
                return true;
            }
            let node_type = self.nodes[current].node_type;
            if node_type == "NodeList" || node_type == "NodeListItem" {
                if let Some(last) = self.nodes[current].children.last() {
                    current = *last;
                    continue;
                }
            }
            return false;
        }
    }
}
