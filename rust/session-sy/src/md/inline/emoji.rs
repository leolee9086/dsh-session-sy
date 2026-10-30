//! Emoji 别名替换，逐函数对照 Lute 的 `parse/emoji.go`。
//!
//! 映射表从 `parse/emoji_map.go` 的 `EmojiAliasUnicode` 提取，见 `src/md/emoji_map.tsv`。

use std::collections::HashMap;
use std::sync::OnceLock;

use super::Parser;

/// Lute 的默认 `EmojiSite`（NewOptions 里就是这个 Vditor 的 CDN 路径）。
const EMOJI_SITE: &str = "https://cdn.jsdelivr.net/npm/vditor/dist/images/emoji";
const EMOJI_SITE_PLACEHOLDER: &str = "${emojiSite}";

/// 别名 → Unicode 或图片占位路径。
fn alias_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        include_str!("../emoji_map.tsv")
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(2, '\t');
                let alias = parts.next()?;
                let value = parts.next()?;
                if alias.is_empty() {
                    None
                } else {
                    Some((alias, value))
                }
            })
            .collect()
    })
}

enum EmojiPiece {
    Text(Vec<u8>),
    /// alias 是含冒号的原文（`:smile:`），value 是表里的值。
    Emoji { alias: Vec<u8>, value: Vec<u8> },
}

impl<'a> Parser<'a> {
    /// 对应 `emoji`：递归处理 NodeText 和 NodeLinkText。
    pub(super) fn apply_emoji(&mut self) {
        self.walk_emoji(None);
    }

    fn walk_emoji(&mut self, parent: Option<usize>) {
        let mut index = 0usize;
        while index < self.children_len(parent) {
            let child = self.children_get(parent, index);
            let ty = self.nodes[child].node_type;
            if ty == "NodeText" || ty == "NodeLinkText" {
                if let Some(pieces) = self.split_emoji(child) {
                    let items = self.materialize_emoji(child, pieces);
                    let count = items.len();
                    self.children_replace(parent, index, items);
                    index += count;
                    continue;
                }
            } else {
                self.walk_emoji(Some(child));
            }
            index += 1;
        }
    }

    /// 对应 `emoji0`：把一个文本节点拆成 文本/Emoji 序列。
    fn split_emoji(&mut self, node: usize) -> Option<Vec<EmojiPiece>> {
        let tokens = self.nodes[node].data.clone();
        if tokens.is_empty() || !tokens.contains(&b':') {
            return None;
        }
        let length = tokens.len();
        let mut pieces: Vec<EmojiPiece> = Vec::new();
        let mut pos = 0usize;
        let mut i = 0usize;
        let mut changed = false;

        while i < length {
            if i == length - 1 {
                pieces.push(EmojiPiece::Text(tokens[pos..].to_vec()));
                break;
            }
            if tokens[i] != b':' {
                i += 1;
                continue;
            }
            pieces.push(EmojiPiece::Text(tokens[pos..i].to_vec()));

            let mut matched = false;
            pos = i + 1;
            while pos < length {
                let token = tokens[pos];
                if super::is_whitespace(token) {
                    break;
                }
                if token == b':' {
                    matched = true;
                    break;
                }
                pos += 1;
            }
            if !matched {
                pieces.push(EmojiPiece::Text(tokens[i..pos].to_vec()));
                i += 1;
                continue;
            }

            let alias_inner = &tokens[i + 1..pos];
            if alias_inner.is_empty() {
                pieces.push(EmojiPiece::Text(vec![tokens[pos]]));
                i += 1;
                continue;
            }

            let Some(value) = alias_map().get(std::str::from_utf8(alias_inner).unwrap_or("")) else {
                // 不在表里：原样保留 :alias:
                pieces.push(EmojiPiece::Text(tokens[i..pos + 1].to_vec()));
                pos += 1;
                i = pos;
                continue;
            };

            changed = true;
            pieces.push(EmojiPiece::Emoji {
                alias: tokens[i..pos + 1].to_vec(),
                value: value.as_bytes().to_vec(),
            });
            pos += 1;
            i = pos;
        }

        if changed {
            Some(pieces)
        } else {
            None
        }
    }

    /// 把 pieces 落成节点：第一段文本写回原节点，其余新建。
    fn materialize_emoji(&mut self, node: usize, pieces: Vec<EmojiPiece>) -> Vec<usize> {
        let original_type = self.nodes[node].node_type;
        let mut out: Vec<usize> = Vec::new();
        let mut first = true;
        for piece in pieces {
            match piece {
                EmojiPiece::Text(data) => {
                    if first {
                        self.nodes[node].data = data;
                        out.push(node);
                        first = false;
                    } else if !data.is_empty() {
                        let created = self.new_node(original_type);
                        self.nodes[created].data = data;
                        out.push(created);
                    }
                }
                EmojiPiece::Emoji { alias, value } => {
                    let emoji = self.new_emoji_node(&alias, &value);
                    out.push(emoji);
                    first = false;
                }
            }
        }
        // Lute 会丢掉空的文本节点
        out.retain(|item| {
            let node = &self.nodes[*item];
            node.node_type != "NodeText" && node.node_type != "NodeLinkText"
                || !node.data.is_empty()
                || !node.children.is_empty()
        });
        out
    }

    /// 对应 `emoji0` 里构造 NodeEmoji 的部分。
    fn new_emoji_node(&mut self, alias: &[u8], value: &[u8]) -> usize {
        let emoji = self.new_node("NodeEmoji");
        let text = std::str::from_utf8(value).unwrap_or("");
        let (kind, payload) = if text.contains(EMOJI_SITE_PLACEHOLDER) {
            let name = std::str::from_utf8(alias).unwrap_or("").trim_matches(':');
            let suffix = if name == "huaji" {
                ".gif"
            } else if name == "siyuan" {
                ".svg"
            } else {
                ".png"
            };
            let src = format!("{EMOJI_SITE}/{name}{suffix}");
            ("NodeEmojiImg", emoji_img_tokens(name, &src))
        } else if text.contains('.') {
            let name = std::str::from_utf8(alias).unwrap_or("").trim_matches(':');
            ("NodeEmojiImg", emoji_img_tokens(name, text))
        } else {
            ("NodeEmojiUnicode", value.to_vec())
        };

        let inner = self.new_node(kind);
        self.nodes[inner].data = payload;
        let alias_node = self.new_node("NodeEmojiAlias");
        self.nodes[alias_node].data = alias.to_vec();
        self.nodes[inner].children = vec![alias_node];
        self.nodes[emoji].children = vec![inner];
        emoji
    }
}

/// 对应 `EmojiImgTokens`：图片型 Emoji 的 Data 就是这段 HTML。
fn emoji_img_tokens(alias: &str, src: &str) -> Vec<u8> {
    format!("<img alt=\"{alias}\" class=\"emoji\" src=\"{src}\" title=\"{alias}\" />").into_bytes()
}
