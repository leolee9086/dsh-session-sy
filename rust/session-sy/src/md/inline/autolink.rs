//! 尖括号自动链接（对应 Lute 的 autolink.go）。

use serde_json::json;

use super::link::{base64, encode_destination};
use super::Parser;

impl<'a> Parser<'a> {
    /// 对应 newLink：尖括号链接与引用定义的节点形状（开括号不带数据）。
    fn new_link(&mut self, text: &[u8], dest: &[u8], title: Option<&[u8]>, link_type: i64) -> usize {
        let node = self.new_node("NodeLink");
        self.nodes[node].attrs.push(("LinkType", json!(link_type)));
        if link_type == 1 {
            self.nodes[node]
                .attrs
                .push(("LinkRefLabel", json!(base64(text))));
        }
        let open = self.new_node("NodeOpenBracket");
        let text_node = self.new_node("NodeLinkText");
        self.nodes[text_node].data = text.to_vec();
        let close = self.new_node("NodeCloseBracket");
        let paren = self.new_node("NodeOpenParen");
        let dest_node = self.new_node("NodeLinkDest");
        self.nodes[dest_node].data = dest.to_vec();
        let close_paren = self.new_node("NodeCloseParen");
        let mut children = vec![open, text_node, close, paren, dest_node];
        if let Some(title) = title {
            let title_node = self.new_node("NodeLinkTitle");
            self.nodes[title_node].data = title.to_vec();
            children.push(title_node);
        }
        children.push(close_paren);
        self.nodes[node].children = children;
        self.append(node);
        node
    }

    /// 对应 parseAutolink：尖括号里是不含空格的绝对 URI。
    pub(super) fn parse_autolink(&mut self) -> bool {
        let start = self.pos;
        let mut scheme: Vec<u8> = Vec::new();
        let mut schemed = false;
        let mut dest: Vec<u8> = Vec::new();
        let mut i = start + 1;
        while i < self.tokens.len() && self.tokens[i] != b'>' {
            let token = self.tokens[i];
            if token == b' ' {
                return false;
            }
            dest.push(token);
            if !schemed {
                if token != b':' {
                    scheme.push(token);
                } else {
                    schemed = true;
                }
            }
            i += 1;
        }
        if !schemed || scheme.len() < 3 || i >= self.tokens.len() {
            return false;
        }
        self.pos = i + 1;
        let text = dest.clone();
        let dest = encode_destination(&dest);
        self.new_link(&text, &dest, None, 2);
        true
    }

    /// 对应 parseAutoEmailLink。
    pub(super) fn parse_email_autolink(&mut self) -> bool {
        let start = self.pos;
        let mut dest: Vec<u8> = Vec::new();
        let mut i = start + 1;
        let mut at = false;
        while i < self.tokens.len() {
            let token = self.tokens[i];
            dest.push(token);
            i += 1;
            if token == b'@' {
                at = true;
                break;
            }
            if !is_email_atom(token) {
                return false;
            }
        }
        if !at || dest.len() < 2 {
            return false;
        }
        let mut closed = false;
        let mut domain_len = 0usize;
        while i < self.tokens.len() {
            let token = self.tokens[i];
            if token == b'>' {
                closed = true;
                break;
            }
            if !(token.is_ascii_alphanumeric() || token == b'-' || token == b'.') {
                return false;
            }
            dest.push(token);
            domain_len += 1;
            i += 1;
            if domain_len > 63 {
                return false;
            }
        }
        if !closed || domain_len == 0 {
            return false;
        }
        self.pos = i + 1;
        let text = dest.clone();
        let mut link_dest: Vec<u8> = b"mailto:".to_vec();
        link_dest.extend_from_slice(&text);
        self.new_link(&text, &link_dest, None, 2);
        true
    }
}

fn is_email_atom(token: u8) -> bool {
    token.is_ascii_alphanumeric()
        || matches!(
            token,
            b'-' | b'.' | b'+' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'/' | b'='
                | b'?' | b'^' | b'_' | b'\x60' | b'{' | b'|' | b'}' | b'~'
        )
}
