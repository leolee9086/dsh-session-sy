//! 各块类型的收尾处理（对应 Lute 各块的 Finalize 方法）。

use serde_json::json;

use super::util::{base64, trim_trailing_newline_spaces, trim_whitespace};
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
