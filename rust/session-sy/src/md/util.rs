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

/// 只读版本：渲染时只需要看内容，不必为每个段落复制一份。
pub(super) fn trim_trailing_blank_ref(tokens: &[u8]) -> &[u8] {
    let mut end = tokens.len();
    while end > 0 && matches!(tokens[end - 1], b'\n' | b'\r' | b' ' | b'\t') {
        end -= 1;
    }
    &tokens[..end]
}

pub(super) fn trim_trailing_blank(tokens: &[u8]) -> Vec<u8> {
    trim_trailing_blank_ref(tokens).to_vec()
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

/// 快速写一个十进制整数，绕开 `write!` 的格式化 machinery。
///
/// 直出路径上数字非常多（列表分隔符、缩进、序号、标题级别），
/// `core::fmt` 每次都要走 trait 对象和格式化参数，比手写慢一个量级。
pub(super) fn write_i64(out: &mut String, value: i64) {
    let mut buffer = [0u8; 20];
    let mut i = buffer.len();
    let negative = value < 0;
    // 用 u64 处理，避免 i64::MIN 取反溢出
    let mut rest = if negative { (value as i128).unsigned_abs() as u64 } else { value as u64 };
    loop {
        i -= 1;
        buffer[i] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if negative {
        i -= 1;
        buffer[i] = b'-';
    }
    // 只写了 ASCII 数字和负号，一定是合法 UTF-8
    out.push_str(std::str::from_utf8(&buffer[i..]).unwrap_or("0"));
}

/// 按 serde_json 的规则把字符串写进 out（含两端引号）。
///
/// 规则与 serde_json::to_string 一致：只转义 " \ 和 0x00-0x1F 控制符，
/// 非 ASCII 原样输出，DEL(0x7F) 不转义。切分点都落在单字节 ASCII 上，不会切坏 UTF-8。
pub(super) fn write_json_string(out: &mut String, value: &str) {
    /// 这个字节是否需要转义。非 ASCII（>=0x80）一律不需要，原样输出。
    #[inline]
    fn needs_escape(b: u8) -> bool {
        b < 0x20 || b == b'"' || b == b'\\'
    }

    let bytes = value.as_bytes();
    // 快速路径：绝大多数文本（尤其中文正文）一个需要转义的字节都没有。
    // any() 会被向量化，比逐字节走一遍 match 快得多。
    if !bytes.iter().any(|&b| needs_escape(b)) {
        out.push('"');
        out.push_str(value);
        out.push('"');
        return;
    }

    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        let simple = match b {
            b'"' => Some("\\\""),
            b'\\' => Some("\\\\"),
            0x08 => Some("\\b"),
            0x09 => Some("\\t"),
            0x0a => Some("\\n"),
            0x0c => Some("\\f"),
            0x0d => Some("\\r"),
            0x00..=0x1f => None,
            _ => continue,
        };
        if start < i {
            out.push_str(&value[start..i]);
        }
        match simple {
            Some(escaped) => out.push_str(escaped),
            None => {
                out.push_str("\\u00");
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
        start = i + 1;
    }
    if start < bytes.len() {
        out.push_str(&value[start..]);
    }
    out.push('"');
}

