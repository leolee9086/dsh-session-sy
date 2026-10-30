//! GFM 自动链接，逐函数对照 Lute 的 `parse/auto_link.go`。
//!
//! Lute 把这一步放在**行内解析之后**：`walkParseInline` 里先 `parseGFMAutoEmailLink`
//! 再 `parseGFMAutoLink`，对已生成的 `NodeText` 做二次拆分。
//! 这里保持同样的顺序和同样的拆分规则，包括 `file://` 这类"前缀被吃掉"的行为。

use std::collections::HashSet;
use std::sync::OnceLock;

use serde_json::json;

use super::link::{decode_destination, encode_destination};
use super::{is_whitespace, Parser};

const HTTP_PROTO: &[u8] = b"http://";
const HTTPS_PROTO: &[u8] = b"https://";
const FTP_PROTO: &[u8] = b"ftp://";
const PROTO_SEP: &[u8] = b"://";
/// 最短可能的链接：`a://b` 或 `www.xxx.xx`
const MIN_LINK_LEN: usize = 5;

/// 顶级域名白名单。数据从 Lute 的 `allTLDs` 提取，见 `src/md/tlds.txt`。
fn valid_suffix() -> &'static HashSet<&'static str> {
    static SUFFIX: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SUFFIX.get_or_init(|| {
        include_str!("../tlds.txt")
            .lines()
            .filter(|line| !line.is_empty())
            .collect()
    })
}

/// 自动链接拆分出来的一段内容。
enum Piece {
    Text(Vec<u8>),
    /// 普通自动链接：带 OpenBracket/CloseBracket/OpenParen/CloseParen 外壳
    Link { text: Vec<u8>, dest: Vec<u8> },
    /// 邮箱自动链接：只有 LinkText + LinkDest，**没有括号节点**
    CompactLink { text: Vec<u8>, dest: Vec<u8> },
}

/// `tokens[at..]` 是否以 `needle` 开头。
fn starts_with_at(tokens: &[u8], at: usize, needle: &[u8]) -> bool {
    at + needle.len() <= tokens.len() && &tokens[at..at + needle.len()] == needle
}

/// 对应 `bytes.LastIndex`，返回 i64（找不到是 -1）。
fn last_index(tokens: &[u8], needle: &[u8]) -> i64 {
    if needle.len() > tokens.len() {
        return -1;
    }
    let mut found = -1i64;
    for at in 0..=tokens.len() - needle.len() {
        if &tokens[at..at + needle.len()] == needle {
            found = at as i64;
        }
    }
    found
}

impl<'a> Parser<'a> {
    /// 对应 `walkParseInline` 里那两段后处理：先邮箱，再 URL。
    pub(super) fn apply_auto_links(&mut self) {
        self.walk_auto_email(None);
        self.walk_auto_link(None);
    }

    pub(super) fn children_len(&self, parent: Option<usize>) -> usize {
        match parent {
            Some(index) => self.nodes[index].children.len(),
            None => self.root.len(),
        }
    }

    pub(super) fn children_get(&self, parent: Option<usize>, index: usize) -> usize {
        match parent {
            Some(node) => self.nodes[node].children[index],
            None => self.root[index],
        }
    }

    pub(super) fn children_replace(&mut self, parent: Option<usize>, index: usize, items: Vec<usize>) {
        let count = items.len();
        match parent {
            Some(node) => {
                self.nodes[node].children.splice(index..index + 1, items);
            }
            None => {
                self.root.splice(index..index + 1, items);
            }
        }
        let _ = count;
    }

    /// 把 NodeText 原地拆成 文本/链接 序列，返回新节点列表；没有链接时返回 None。
    fn split_text_into_links(&mut self, node: usize, kind: AutoKind) -> Option<Vec<usize>> {
        let pieces = match kind {
            AutoKind::Url => self.split_auto_url(node),
            AutoKind::Email => self.split_auto_email(node),
        }?;
        Some(self.materialize(node, pieces))
    }

    /// 把 pieces 变成节点：第一段文本会并回前一个文本兄弟节点（对应 addPreviousText）。
    fn materialize(&mut self, node: usize, pieces: Vec<Piece>) -> Vec<usize> {
        let previous_text = self.previous_text_sibling(node);
        let mut out: Vec<usize> = Vec::new();
        let mut first_text_taken = false;

        for piece in pieces {
            match piece {
                Piece::Text(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    // 并到上一个文本节点：先看已有的前兄弟，再看本次生成的最后一个
                    if !first_text_taken {
                        first_text_taken = true;
                        if let Some(prev) = previous_text {
                            self.nodes[prev].data.extend_from_slice(&data);
                            continue;
                        }
                    } else if let Some(last) = out.last().copied() {
                        if self.nodes[last].node_type == "NodeText" {
                            self.nodes[last].data.extend_from_slice(&data);
                            continue;
                        }
                    }
                    let created = self.new_node("NodeText");
                    self.nodes[created].data = data;
                    out.push(created);
                }
                Piece::Link { text, dest } => {
                    let link = self.new_link_node(&text, &dest);
                    out.push(link);
                }
                Piece::CompactLink { text, dest } => {
                    let link = self.new_node("NodeLink");
                    self.nodes[link].attrs.push(("LinkType", json!(2)));
                    let link_text = self.new_node("NodeLinkText");
                    self.nodes[link_text].data = text;
                    let dest_node = self.new_node("NodeLinkDest");
                    self.nodes[dest_node].data = dest;
                    self.nodes[link].children = vec![link_text, dest_node];
                    out.push(link);
                }
            }
        }
        out
    }

    /// 对应 addPreviousText 里的 `node.Previous`：找同级的、紧挨在前面的文本节点。
    fn previous_text_sibling(&self, node: usize) -> Option<usize> {
        for parent in 0..self.nodes.len() {
            let children = &self.nodes[parent].children;
            if let Some(position) = children.iter().position(|child| *child == node) {
                if position == 0 {
                    return None;
                }
                let prev = children[position - 1];
                if self.nodes[prev].node_type == "NodeText" {
                    return Some(prev);
                }
                return None;
            }
        }
        if let Some(position) = self.root.iter().position(|child| *child == node) {
            if position == 0 {
                return None;
            }
            let prev = self.root[position - 1];
            if self.nodes[prev].node_type == "NodeText" {
                return Some(prev);
            }
        }
        None
    }

    /// 对应 `newLink`：LinkType=2，括号节点不带 Data。
    fn new_link_node(&mut self, text: &[u8], dest: &[u8]) -> usize {
        // 对应 Lute newLink 里的 text = html.DecodeDestination(text)
        let text = decode_destination(text);
        let link = self.new_node("NodeLink");
        self.nodes[link].attrs.push(("LinkType", json!(2)));
        let mut children: Vec<usize> = Vec::with_capacity(6);
        children.push(self.new_node("NodeOpenBracket"));
        let link_text = self.new_node("NodeLinkText");
        self.nodes[link_text].data = text;
        children.push(link_text);
        children.push(self.new_node("NodeCloseBracket"));
        children.push(self.new_node("NodeOpenParen"));
        let dest_node = self.new_node("NodeLinkDest");
        self.nodes[dest_node].data = dest.to_vec();
        children.push(dest_node);
        children.push(self.new_node("NodeCloseParen"));
        self.nodes[link].children = children;
        link
    }

    fn walk_auto_link(&mut self, parent: Option<usize>) {
        let mut index = 0usize;
        while index < self.children_len(parent) {
            let child = self.children_get(parent, index);
            if self.nodes[child].node_type == "NodeText" {
                if let Some(items) = self.split_text_into_links(child, AutoKind::Url) {
                    let count = items.len();
                    self.children_replace(parent, index, items);
                    index += count;
                    continue;
                }
            } else {
                self.walk_auto_link(Some(child));
            }
            index += 1;
        }
    }

    fn walk_auto_email(&mut self, parent: Option<usize>) {
        let mut index = 0usize;
        while index < self.children_len(parent) {
            let child = self.children_get(parent, index);
            let mut handled = false;
            if self.nodes[child].node_type == "NodeText" {
                // 不处理链接 label 里的文本，也不处理 <a>...</a> 里面的文本
                let inside_link = self.parent_is_link(child);
                if !inside_link && !self.inside_anchor_tag(parent, index) {
                    if let Some(items) = self.split_text_into_links(child, AutoKind::Email) {
                        let count = items.len();
                        self.children_replace(parent, index, items);
                        index += count;
                        handled = true;
                    }
                }
            }
            if !handled {
                if self.nodes[child].node_type != "NodeText" {
                    self.walk_auto_email(Some(child));
                }
                index += 1;
            }
        }
    }

    /// 对应 Lute parseGFMAutoEmailLink0 开头那段：
    /// 文本夹在 `<a ...>` 与 `</a>` 两个行内 HTML 之间时，不做邮箱自动链接。
    fn inside_anchor_tag(&self, parent: Option<usize>, index: usize) -> bool {
        if index == 0 || index + 1 >= self.children_len(parent) {
            return false;
        }
        let previous = self.children_get(parent, index - 1);
        let next = self.children_get(parent, index + 1);
        self.nodes[previous].node_type == "NodeInlineHTML"
            && self.nodes[next].node_type == "NodeInlineHTML"
            && self.nodes[previous].data.starts_with(b"<a ")
            && self.nodes[next].data == b"</a>"
    }

    fn parent_is_link(&self, node: usize) -> bool {
        for parent in 0..self.nodes.len() {
            if self.nodes[parent].children.contains(&node) {
                return self.nodes[parent].node_type == "NodeLink";
            }
        }
        false
    }
}

enum AutoKind {
    Url,
    Email,
}

impl<'a> Parser<'a> {
    /// 对应 `parseGFMAutoLink0`：把一个 NodeText 拆成 文本/链接 序列。
    fn split_auto_url(&mut self, node: usize) -> Option<Vec<Piece>> {
        let tokens = self.nodes[node].data.clone();
        let length = tokens.len();
        if length < MIN_LINK_LEN {
            return None;
        }

        let custom_end = last_index(&tokens, PROTO_SEP);
        let mut custom_start = custom_end;
        while custom_start > 0 && tokens[(custom_start - 1) as usize].is_ascii_alphanumeric() {
            custom_start -= 1;
        }
        let mut last_standard = last_index(&tokens, HTTP_PROTO);
        let https = last_index(&tokens, HTTPS_PROTO);
        if last_standard < https {
            last_standard = https;
        }
        let ftp = last_index(&tokens, FTP_PROTO);
        if last_standard < ftp {
            last_standard = ftp;
        }

        let mut pieces: Vec<Piece> = Vec::new();
        let mut i = 0usize;
        let mut text_start = 0usize;
        let mut text_end = 0usize;
        let mut www = false;
        let mut need_unlink = false;

        while i < length {
            let tmp_len = length - i;
            let mut proto_start = 0usize;
            let mut proto_end = 0usize;
            let mut matched_proto = true;

            if tmp_len >= 10
                && tokens[i] == b'w'
                && tokens[i + 1] == b'w'
                && tokens[i + 2] == b'w'
                && tokens[i + 3] == b'.'
            {
                www = true;
            } else if tmp_len >= 13 && starts_with_at(&tokens, i, b"http://") {
                proto_start = i;
                proto_end = i + 7;
                i += 7;
            } else if tmp_len >= 14 && starts_with_at(&tokens, i, b"https://") {
                proto_start = i;
                proto_end = i + 8;
                i += 8;
            } else if tmp_len >= 12 && starts_with_at(&tokens, i, b"ftp://") {
                proto_start = i;
                proto_end = i + 6;
                i += 6;
            } else if custom_start <= i as i64
                && (i as i64) < custom_end
                && last_standard < i as i64
                && custom_end + 3 < length as i64
            {
                proto_start = i;
                proto_end = (custom_end + 3) as usize;
                i = (custom_end + 3) as usize;
            } else {
                matched_proto = false;
            }

            if !matched_proto {
                text_end += 1;
                if length - i < MIN_LINK_LEN {
                    if need_unlink {
                        if text_start < text_end {
                            pieces.push(Piece::Text(tokens[text_start..].to_vec()));
                        } else {
                            pieces.push(Piece::Text(tokens[text_end..].to_vec()));
                        }
                        return Some(pieces);
                    }
                    return None;
                }
                i += 1;
                continue;
            }

            let protocol: Vec<u8> = if www {
                HTTP_PROTO.to_vec()
            } else {
                tokens[proto_start..proto_end].to_vec()
            };

            if text_start < text_end {
                pieces.push(Piece::Text(tokens[text_start..text_end].to_vec()));
                need_unlink = true;
                text_start = text_end;
            }

            let mut url: Vec<u8> = Vec::new();
            let mut colon_seen = false;
            let mut path_seen = false;
            let mut j = i;
            let mut break_token = 0u8;
            while j < length {
                let token = tokens[j];
                if is_whitespace(token) || token == b'<' {
                    break_token = token;
                    break;
                }
                if colon_seen && !path_seen {
                    if !token.is_ascii_digit() && token != b'/' && token != b'?' && token != b'#' {
                        break_token = token;
                        break;
                    }
                }
                if !colon_seen && token == b':' && !starts_with_at(&tokens, j, PROTO_SEP) {
                    colon_seen = true;
                }
                if token == b'/' || token == b'?' || token == b'#' {
                    path_seen = true;
                }
                url.push(token);
                j += 1;
            }

            if i == j {
                if break_token >= 0x80 {
                    if !www {
                        url.extend_from_slice(&protocol);
                    }
                    while i < length {
                        let token = tokens[i];
                        if token < 0x80 {
                            break;
                        }
                        url.push(token);
                        i += 1;
                    }
                } else {
                    url.push(break_token);
                    i += 1;
                }
                if !url.is_empty() {
                    pieces.push(Piece::Text(url));
                }
                text_start = i;
                text_end = i;
                continue;
            }
            i = j;

            let mut k = 0usize;
            while k < url.len() && url[k] != b'/' {
                k += 1;
            }
            let mut domain: Vec<u8> = url[..k].to_vec();
            let mut port: Vec<u8> = Vec::new();
            if let Some(idx) = domain.iter().position(|&b| b == b':') {
                if idx > 0 {
                    port = domain[idx..].to_vec();
                    domain.truncate(idx);
                }
            }

            if !is_valid_domain(&protocol, &domain) {
                pieces.push(Piece::Text(tokens[text_start..i].to_vec()));
                need_unlink = true;
                text_start = i;
                text_end = i;
                continue;
            }

            let mut path: Vec<u8> = url[k..].to_vec();
            let path_len = path.len();
            if path_len > 0 {
                let mut open_parens = 0i64;
                let mut close_parens = 0i64;
                for &t in &path {
                    if t == b'(' {
                        open_parens += 1;
                    } else if t == b')' {
                        close_parens += 1;
                    }
                }
                let last_token = path[path_len - 1];
                let mut trimmed = false;
                if last_token == b')' {
                    let unmatches = close_parens - open_parens;
                    if unmatches > 0 {
                        let mut l = path_len as i64 - 1;
                        let mut un = unmatches;
                        while un > 0 {
                            if path[l as usize] != b')' {
                                break;
                            }
                            un -= 1;
                            i = i.saturating_sub(1);
                            l -= 1;
                        }
                        path.truncate((l + 1).max(0) as usize);
                        trimmed = true;
                    } else {
                        trimmed = true;
                    }
                } else if last_token == b';' {
                    let mut amp = -1i64;
                    let mut l = path_len as i64 - 1;
                    while l >= 0 {
                        if path[l as usize] == b'&' {
                            amp = l;
                            break;
                        }
                        l -= 1;
                    }
                    if amp >= 0 {
                        let entity_start = amp as usize;
                        let mut is_entity = true;
                        if path_len - entity_start < 3 {
                            is_entity = false;
                        } else {
                            for item in &path[entity_start + 1..path_len - 1] {
                                if !item.is_ascii_alphanumeric() {
                                    is_entity = false;
                                    break;
                                }
                            }
                        }
                        if is_entity {
                            path.truncate(entity_start);
                            trimmed = true;
                            i = i.saturating_sub(path_len - entity_start);
                        }
                    }
                }
                if !trimmed
                    && matches!(last_token, b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~')
                {
                    path.truncate(path_len - 1);
                    i = i.saturating_sub(1);
                }
            } else {
                let last_token = domain[domain.len() - 1];
                if is_ascii_punct(last_token) {
                    domain.pop();
                    i = i.saturating_sub(1);
                }
            }

            let mut dest: Vec<u8> = Vec::new();
            dest.extend_from_slice(&protocol);
            dest.extend_from_slice(&domain);
            dest.extend_from_slice(&port);
            dest.extend_from_slice(&path);

            let mut addr: Vec<u8> = Vec::new();
            if !www {
                addr.extend_from_slice(&protocol);
            }
            addr.extend_from_slice(&domain);
            addr.extend_from_slice(&port);
            addr.extend_from_slice(&path);

            let link_text = github_special_text(addr);

            pieces.push(Piece::Link {
                text: link_text,
                dest: encode_destination(&dest),
            });
            need_unlink = true;
            text_start = i;
            text_end = i;
        }

        if text_start < text_end {
            pieces.push(Piece::Text(tokens[text_start..text_end].to_vec()));
            need_unlink = true;
        }
        if need_unlink {
            Some(pieces)
        } else {
            None
        }
    }

    /// 对应 `parseGFMAutoEmailLink0`：把一个含 @ 的 NodeText 拆成 文本/邮箱链接 序列。
    fn split_auto_email(&mut self, node: usize) -> Option<Vec<Piece>> {
        let tokens = self.nodes[node].data.clone();
        let length = tokens.len();
        if length == 0 {
            return None;
        }
        // 没有 @ 或者 @ 就在开头，都不可能构成邮箱
        match tokens.iter().position(|&b| b == b'@') {
            None | Some(0) => return None,
            Some(_) => {}
        }

        let mut pieces: Vec<Piece> = Vec::new();
        let mut i = 0usize;
        while i < length {
            let mut group: Vec<u8> = Vec::new();
            let mut at_index = 0usize;
            let mut has_at = false;
            let mut j = i;
            while j < length {
                let token = tokens[j];
                if is_whitespace(token) {
                    break;
                }
                group.push(token);
                if token == b'@' {
                    at_index = j - i;
                    has_at = true;
                }
                j += 1;
            }

            if i == j {
                // 第一个字符就是空白：把空白本身当文本
                pieces.push(Piece::Text(vec![tokens[j]]));
                i += 1;
                continue;
            }
            i = j;

            if !has_at || at_index == 0 {
                pieces.push(Piece::Text(group));
                continue;
            }

            // @ 之前只允许字母数字 - . + _
            let mut ok = true;
            for &token in &group[..at_index] {
                if !is_valid_email_segment1(token) {
                    ok = false;
                    break;
                }
            }
            if ok {
                for &token in &group[at_index + 1..] {
                    if !is_valid_email_segment2(token) {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                pieces.push(Piece::Text(group));
                continue;
            }

            let last = *group.last().unwrap_or(&0);
            if last == b'.' {
                // 以 . 结尾：剔除这个点，点本身作为文本跟在后头。
                // 注意这一支走 Lute 的 newLink（六个子节点带括号），
                // 和下面手工构造的紧凑形式不同。
                group.pop();
                pieces.push(Piece::Link {
                    text: group.clone(),
                    dest: with_mailto(&group),
                });
                pieces.push(Piece::Text(vec![b'.']));
            } else if last == b'-' || last == b'_' {
                // 以 - 或 _ 结尾的整串都不算邮箱
                pieces.push(Piece::Text(group));
            } else {
                pieces.push(Piece::CompactLink {
                    text: group.clone(),
                    dest: with_mailto(&group),
                });
            }
        }

        if pieces.is_empty() {
            None
        } else {
            Some(pieces)
        }
    }

}

fn is_valid_domain(protocol: &[u8], domain: &[u8]) -> bool {
    if !protocol.is_empty()
        && !protocol.windows(HTTP_PROTO.len()).any(|w| w == HTTP_PROTO)
        && !protocol.windows(HTTPS_PROTO.len()).any(|w| w == HTTPS_PROTO)
        && !protocol.windows(FTP_PROTO.len()).any(|w| w == FTP_PROTO)
    {
        return true;
    }

    let mut total = domain.iter().filter(|&&b| b == b'.').count() + 1;
    if !domain.is_empty() && domain[domain.len() - 1] == b'.' {
        total -= 1;
    }
    if total < 2 {
        return false;
    }

    let mut segments = 0usize;
    let mut i = 0usize;
    let length = domain.len();
    loop {
        let start = i;
        while i < length && domain[i] != b'.' {
            i += 1;
        }
        let segment = &domain[start..i];
        if !segment.is_empty() {
            for &token in segment.iter() {
                if !is_ascii_letter_num_hyphen(token) {
                    return false;
                }
                if segments > 2 && (segments == total - 2 || segments == total - 1) && token == b'_' {
                    return false;
                }
            }
            if segments == total - 1 {
                let suffix_is_digit = segment.iter().all(|b| b.is_ascii_digit());
                let valid = if suffix_is_digit {
                    true
                } else {
                    valid_suffix().contains(std::str::from_utf8(segment).unwrap_or(""))
                };
                if !valid {
                    return false;
                }
            }
        }
        segments += 1;
        if i >= length {
            break;
        }
        i += 1;
    }
    true
}

fn is_ascii_letter_num_hyphen(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// 对应 `isValidEmailSegment1`：@ 之前的字符集。
fn is_valid_email_segment1(token: u8) -> bool {
    is_ascii_letter_num_hyphen(token)
        || token == b'.'
        || token == b'+'
        || token == b'_'
}

/// 对应 `isValidEmailSegment2`：@ 之后的字符集。
fn is_valid_email_segment2(token: u8) -> bool {
    is_ascii_letter_num_hyphen(token) || token == b'.' || token == b'_'
}

/// 邮箱链接的目标要加 `mailto:` 前缀。
fn with_mailto(group: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(7 + group.len());
    out.extend_from_slice(b"mailto:");
    out.extend_from_slice(group);
    out
}


fn is_ascii_punct(token: u8) -> bool {
    (0x21..=0x2f).contains(&token)
        || (0x3a..=0x40).contains(&token)
        || (0x5b..=0x60).contains(&token)
        || (0x7b..=0x7e).contains(&token)
}

/// 对应 `newLink` 前对 GitHub Issue / PR 链接文本的改写。
fn github_special_text(addr: Vec<u8>) -> Vec<u8> {
    const PREFIX: &[u8] = b"https://github.com/";
    if !addr.starts_with(PREFIX) {
        return addr;
    }
    let rest = &addr[PREFIX.len()..];
    let markers: [(&[u8], &str); 2] = [(b"/issues/", "Issue #"), (b"/pull/", "Pull Request #")];
    for (marker, label) in markers {
        if let Some(at) = rest.windows(marker.len()).position(|w| w == marker) {
            let repo = &rest[..at];
            let tail = &rest[at + marker.len()..];
            let num_end = tail.iter().position(|&b| b == b'?').unwrap_or(tail.len());
            let num = &tail[..num_end];
            if !num.is_empty() && num.iter().all(|b| b.is_ascii_digit()) {
                let mut out = Vec::new();
                out.extend_from_slice(label.as_bytes());
                out.extend_from_slice(num);
                out.extend_from_slice(" · ".as_bytes());
                out.extend_from_slice(repo);
                return out;
            }
        }
    }
    addr
}

