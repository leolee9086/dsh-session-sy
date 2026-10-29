//! 块级解析用的纯函数：空白裁剪、围栏判定、base64 标记。
//!
//! 这里不碰 Parser 状态，都是可以直接测的输入输出。

use super::ListData;

/// 两个列表记号是否属于同一个列表（对应 listsMatch）。
pub(super) fn lists_match(list: &ListData, item: &ListData) -> bool {
    list.typ == item.typ
        && ((list.delimiter == 0 && item.delimiter == 0) || list.delimiter == item.delimiter)
        && list.bullet_char == item.bullet_char
}

pub(super) fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// 去掉 ATX 标题尾部的闭合井号（含它前面的空白）。
pub(super) fn strip_closing_crosshatch(content: &[u8]) -> Vec<u8> {
    let mut end = content.len();
    while end > 0 && matches!(content[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let mut hash_end = end;
    while hash_end > 0 && content[hash_end - 1] == b'#' {
        hash_end -= 1;
    }
    if hash_end == end {
        return content[..end].to_vec();
    }
    if hash_end == 0 {
        return Vec::new();
    }
    if matches!(content[hash_end - 1], b' ' | b'\t') {
        let mut trimmed = hash_end - 1;
        while trimmed > 0 && matches!(content[trimmed - 1], b' ' | b'\t') {
            trimmed -= 1;
        }
        return content[..trimmed].to_vec();
    }
    content[..end].to_vec()
}

/// 对应 Lute 的 ReplaceNewlineSpace：只裁掉尾部多余的换行/空格。
pub(super) fn trim_trailing_newline_spaces(tokens: &[u8]) -> Vec<u8> {
    let mut end = tokens.len();
    while end >= 2 {
        let last = tokens[end - 1];
        if last != b'\n' && last != b' ' {
            break;
        }
        if tokens[end - 2] == b'\n' {
            end -= 1;
        } else {
            break;
        }
    }
    tokens[..end].to_vec()
}

pub(super) fn trim_right_whitespace(tokens: &[u8]) -> &[u8] {
    let mut end = tokens.len();
    while end > 0 && matches!(tokens[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    &tokens[..end]
}

pub(super) fn trim_whitespace(tokens: &[u8]) -> Vec<u8> {
    let mut start = 0usize;
    let mut end = tokens.len();
    while start < end && matches!(tokens[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    while end > start && matches!(tokens[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    tokens[start..end].to_vec()
}

pub(super) fn trim_trailing_blank(tokens: &[u8]) -> Vec<u8> {
    let mut end = tokens.len();
    while end > 0 && matches!(tokens[end - 1], b'\n' | b'\r' | b' ' | b'\t') {
        end -= 1;
    }
    tokens[..end].to_vec()
}

/// 闭合围栏是否合法：足够长的同类围栏 + 后面只有空白。
pub(super) fn fence_len_rest(rest: &[u8], ch: u8, len: usize) -> bool {
    let mut run = 0usize;
    while run < rest.len() && rest[run] == ch {
        run += 1;
    }
    run >= len && rest[run..].iter().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

/// 解开 Lute 存在字段里的 base64 标记。
pub(super) fn decode_marker(marker: &str) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = marker.as_bytes();
    let mut out = String::new();
    let mut buffer = 0u32;
    let mut bits = 0usize;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        let Some(value) = TABLE.iter().position(|item| *item == b) else {
            continue;
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8 as char);
        }
    }
    out
}

/// 标准 base64（带填充）：Lute 把标记按 base64 存进字段。
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
