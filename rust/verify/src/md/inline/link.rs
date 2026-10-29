//! 链接与标题的扫描函数（对应 Lute 的 link.go / inline_link.go 里的纯函数部分）。

use super::{is_ascii_punct, is_whitespace, trim_whitespace};

/// 对应 parseLinkLabel：返回标签内容和消耗的字节数（相对 from）。
pub(super) fn scan_link_label(tokens: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    let rest = &tokens[from..];
    if rest.len() < 2 || rest[0] != b'[' {
        return None;
    }
    let mut label: Vec<u8> = Vec::new();
    let mut closed = false;
    let mut i = 1usize;
    while i < rest.len() {
        let token = rest[i];
        label.push(token);
        if token == b']' && !is_backslash_escaped(rest, i) {
            closed = true;
            label.pop();
            break;
        }
        if token == b'[' && !is_backslash_escaped(rest, i) {
            return None;
        }
        i += 1;
    }
    if !closed || trim_whitespace(&label).is_empty() || label.len() > 999 {
        return None;
    }
    // 规范化：换行折成空格，连续空格合并（对应 Lute 里对 label 的处理）。
    let mut label = trim_whitespace(&label);
    for byte in label.iter_mut() {
        if *byte == b'\n' {
            *byte = b' ';
        }
    }
    let mut collapsed: Vec<u8> = Vec::with_capacity(label.len());
    for byte in label {
        if byte == b' ' && collapsed.last() == Some(&b' ') {
            continue;
        }
        collapsed.push(byte);
    }
    Some((collapsed, i + 1))
}

/// 对应 html.UnescapeBytes：反斜杠转义的 ASCII 标点还原成原字符。
pub(crate) fn unescape_backslashes(tokens: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i] == b'\\' && i + 1 < tokens.len() && is_ascii_punct(tokens[i + 1]) {
            out.push(tokens[i + 1]);
            i += 2;
            continue;
        }
        out.push(tokens[i]);
        i += 1;
    }
    // 反斜杠之后还要解 HTML 实体（对应 html.UnescapeBytes 的整体行为）
    let text = String::from_utf8_lossy(&out).to_string();
    html_escape::decode_html_entities(&text).to_string().into_bytes()
}

/// 对应 html.EncodeDestination：把链接目标里不能直接出现的字符做百分号编码。
pub(super) fn encode_destination(tokens: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for byte in tokens {
        match *byte {
            b' ' | b'"' | b'<' | b'>' | b'\x60' | b'\\' | b'[' | b']' => {
                out.push(b'%');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0f));
            }
            byte if byte >= 0x80 => {
                out.push(b'%');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0f));
            }
            _ => out.push(*byte),
        }
    }
    out
}

/// 百分号编码用的十六进制字符。
fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + (value - 10),
    }
}

/// 对应 parseInlineLinkDest：tokens 从开括号 ( 开始。
/// 返回 (passed 的字节数, 目标)。
pub(super) fn parse_inline_link_dest(tokens: &[u8]) -> Option<(usize, Vec<u8>)> {
    if tokens.len() < 2 {
        return None;
    }
    if tokens[1] == b'<' {
        // 尖括号形式：遇到换行即失败，闭合的 > 必须没被转义
        let mut destination: Vec<u8> = Vec::new();
        let mut i = 2usize;
        let mut match_end = false;
        while i < tokens.len() {
            let token = tokens[i];
            if token == b'\n' {
                return None;
            }
            destination.push(token);
            if token == b'>' && !is_backslash_escaped(tokens, i) {
                destination.pop();
                match_end = true;
                break;
            }
            i += 1;
        }
        if !match_end || tokens.len() <= i + 1 {
            return None;
        }
        let mut passed_len = i + 1;
        if tokens[i + 1] == b'>' || tokens[i + 1] == b')' {
            passed_len += 1;
        }
        let destination = encode_destination(&unescape_backslashes(&destination));
        return Some((passed_len, destination));
    }

    let mut destination: Vec<u8> = Vec::new();
    let mut open_parens = 0i32;
    let mut dest_started = false;
    let mut i = 0usize;
    let mut passed_len = 0usize;
    while i < tokens.len() {
        let token = tokens[i];
        passed_len += 1;
        destination.push(token);
        if !dest_started && !is_whitespace(token) && i > 0 {
            dest_started = true;
            destination.remove(0);
            destination = trim_whitespace(&destination);
        }
        if dest_started && (is_whitespace(token) || is_control(token)) {
            destination.pop();
            passed_len -= 1;
            open_parens -= 1;
            break;
        }
        if token == b'(' && !is_backslash_escaped(tokens, i) {
            open_parens += 1;
        }
        if token == b')' && !is_backslash_escaped(tokens, i) {
            open_parens -= 1;
            if open_parens < 1 {
                if destination.first() == Some(&b'(') {
                    destination.remove(0);
                }
                destination.pop();
                break;
            }
        }
        i += 1;
    }
    if tokens.len() > i {
        let token = tokens[i];
        if token != b')' && token != b' ' && token != b'\n' {
            return None;
        }
    }
    if open_parens != 0 {
        return None;
    }
    let destination = encode_destination(&unescape_backslashes(&destination));
    Some((passed_len, destination))
}

/// 对应 parseLinkDest1：尖括号包裹的目标。
pub(super) fn scan_link_dest_angle(tokens: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    let rest = &tokens[from..];
    if rest.len() < 2 || rest[0] != b'<' {
        return None;
    }
    let mut destination: Vec<u8> = Vec::new();
    let mut closed = false;
    let mut i = 0usize;
    while i < rest.len() {
        let token = rest[i];
        if i > 0 {
            destination.push(token);
            if token == b'<' && !is_backslash_escaped(rest, i) {
                return None;
            }
            if token == b'>' && !is_backslash_escaped(rest, i) {
                closed = true;
                destination.pop();
                break;
            }
        }
        i += 1;
    }
    if !closed {
        return None;
    }
    Some((destination, from + i + 1))
}

/// 对应 parseLinkDest2：裸目标，允许成对圆括号与反斜杠转义。
///
/// 返回值里的位置停在结束目标的那一个字节上（闭合圆括号、空白或行尾），
/// 由调用方决定是内联链接收尾还是继续解析标题。
pub(super) fn scan_link_dest_plain(tokens: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    let rest = &tokens[from..];
    let mut destination: Vec<u8> = Vec::new();
    let mut open_parens = 0i32;
    let mut i = 0usize;
    while i < rest.len() {
        let token = rest[i];
        if is_whitespace(token) || is_control(token) {
            break;
        }
        // 先收字节再判定：配对的右括号属于目标本身（对应 parseLinkDest2）
        destination.push(token);
        if token == b'(' && !is_backslash_escaped(rest, i) {
            open_parens += 1;
        }
        if token == b')' && !is_backslash_escaped(rest, i) {
            open_parens -= 1;
            if open_parens < 1 {
                i += 1;
                break;
            }
        }
        i += 1;
    }
    if i < rest.len() && !is_whitespace(rest[i]) {
        return None;
    }
    // 裸目标不能为空（引用定义的 [foo]: 后面什么都不跟不是定义）
    if destination.is_empty() {
        return None;
    }
    Some((destination, from + i))
}

/// 对应 parseLinkDest：先试尖括号形式，再试裸目标。
pub(super) fn scan_link_dest(tokens: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    if let Some(result) = scan_link_dest_angle(tokens, from) {
        return Some(result);
    }
    scan_link_dest_plain(tokens, from)
}

/// 对应 parseLinkTitleMatch：引号包裹的标题，忽略被转义的闭合符。
fn link_title_match(tokens: &[u8], opener: u8, closer: u8) -> Option<(Vec<u8>, usize)> {
    if tokens.len() < 2 || tokens[0] != opener {
        return None;
    }
    let mut title: Vec<u8> = Vec::new();
    let mut closed = false;
    let mut i = 1usize;
    while i < tokens.len() {
        let token = tokens[i];
        if token == closer && !is_backslash_escaped(tokens, i) {
            closed = true;
            break;
        }
        title.push(token);
        i += 1;
    }
    if !closed {
        return None;
    }
    Some((title, i + 1))
}

/// 对应 parseLinkTitle：双引号、单引号或圆括号。
pub(super) fn scan_link_title(tokens: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    if from >= tokens.len() {
        return None;
    }
    // 引用式标题（[ 开头）不在这里处理
    if tokens[from] == b'[' {
        return None;
    }
    let rest = &tokens[from..];
    for (opener, closer) in [(b'"', b'"'), (b'\'', b'\''), (b'(', b')')] {
        if let Some((title, consumed)) = link_title_match(rest, opener, closer) {
            // 标题里的反斜杠转义与 HTML 实体都要还原
            return Some((unescape_backslashes(&title), from + consumed));
        }
    }
    None
}

/// 对应 lex.IsBackslashEscapePunct：只有 ASCII 标点才可能被反斜杠转义。
pub(super) fn is_backslash_escaped(tokens: &[u8], pos: usize) -> bool {
    if pos >= tokens.len() || !is_ascii_punct(tokens[pos]) {
        return false;
    }
    let mut backslashes = 0usize;
    let mut i = pos;
    while i > 0 {
        i -= 1;
        if tokens[i] != b'\\' {
            break;
        }
        backslashes += 1;
    }
    backslashes % 2 != 0
}

/// 对应 lex.IsControl。
pub(super) fn is_control(token: u8) -> bool {
    token < 0x20 || token == 0x7f
}

/// 标准 base64，供 LinkRefLabel 使用。
pub(super) fn base64(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}
