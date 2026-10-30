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
///
/// 保留字符白名单照抄 Lute（源自 mdurl）：`!#$&'()*+,-./0123456789:;=?@A-Z_a-z~`。
/// `%` 单独处理：后面跟两位十六进制就大写化保留，否则编码成 `%25`。
pub(super) fn encode_destination(tokens: &[u8]) -> Vec<u8> {
    const SAFE: &[u8] =
        b"!#$&'()*+,-./0123456789:;=?@ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz~";
    let mut out: Vec<u8> = Vec::with_capacity(tokens.len() + 16);
    let mut i = 0usize;
    while i < tokens.len() {
        let byte = tokens[i];
        if byte >= 0x80 {
            // 非 ASCII 按 UTF-8 的每个字节编码
            let len = utf8_len(byte);
            let end = (i + len).min(tokens.len());
            for item in &tokens[i..end] {
                push_percent(&mut out, *item);
            }
            i = end;
            continue;
        }
        if byte == b'%' {
            if i + 2 < tokens.len()
                && tokens[i + 1].is_ascii_hexdigit()
                && tokens[i + 2].is_ascii_hexdigit()
            {
                out.push(b'%');
                out.push(tokens[i + 1].to_ascii_uppercase());
                out.push(tokens[i + 2].to_ascii_uppercase());
                i += 3;
                continue;
            }
            out.push(b'%');
            out.push(b'2');
            out.push(b'5');
            i += 1;
            continue;
        }
        if SAFE.contains(&byte) {
            out.push(byte);
        } else {
            push_percent(&mut out, byte);
        }
        i += 1;
    }
    out
}

/// UTF-8 首字节对应的序列长度。
fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first & 0xe0 == 0xc0 {
        2
    } else if first & 0xf0 == 0xe0 {
        3
    } else if first & 0xf8 == 0xf0 {
        4
    } else {
        1
    }
}

fn push_percent(out: &mut Vec<u8>, byte: u8) {
    out.push(b'%');
    out.push(hex_digit(byte >> 4));
    out.push(hex_digit(byte & 0x0f));
}

/// 百分号编码用的十六进制字符。
fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + (value - 10),
    }
}

/// 对应 parseInlineLinkDest：tokens 从开括号 ( 开始。
///
/// 返回 (passed 字节数, remains 起点, 目标)。
///
/// 第二个值是 Lute 里 `remains` 的起点，**它不一定等于 passed 的末尾**：
/// 目标以 `)` 闭合时 Lute 的 `passed` 含这个 `)`，而 `remains` 也从 `)` 处开始，
/// 两者重叠一个字节。调用方要拿它判断“后面跟的是什么”，不能拿 passed 末尾算。
pub(super) fn parse_inline_link_dest(tokens: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
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
        // 尖括号分支里 remains 从 passed 末尾开始，没有重叠
        return Some((passed_len, passed_len, destination));
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
    // remains 从 break 处开始：`)` 闭合时指回那个 `)`，遇到空白时指向空白本身
    Some((passed_len, i, destination))
}

/// 对应 html.DecodeDestination：把百分号编码还原成字节。
///
/// 非法序列原样保留；多字节 UTF-8 序列按 `%XX%XX...` 组装，残缺时写 U+FFFD。
pub(super) fn decode_destination(rawurl: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(rawurl.len());
    let mut i = 0usize;
    while i < rawurl.len() {
        if rawurl[i] == b'%'
            && i + 2 < rawurl.len()
            && rawurl[i + 1].is_ascii_hexdigit()
            && rawurl[i + 2].is_ascii_hexdigit()
        {
            let byte = (unhex(rawurl[i + 1]) << 4) | unhex(rawurl[i + 2]);
            if byte < 0x80 {
                out.push(byte);
                i += 3;
                continue;
            }
            let need = utf8_len(byte).saturating_sub(1);
            let mut sequence = vec![byte];
            let mut cursor = i + 3;
            let mut complete = true;
            for _ in 0..need {
                if cursor + 2 < rawurl.len()
                    && rawurl[cursor] == b'%'
                    && rawurl[cursor + 1].is_ascii_hexdigit()
                    && rawurl[cursor + 2].is_ascii_hexdigit()
                {
                    sequence.push((unhex(rawurl[cursor + 1]) << 4) | unhex(rawurl[cursor + 2]));
                    cursor += 3;
                } else {
                    complete = false;
                    break;
                }
            }
            if complete {
                out.extend_from_slice(&sequence);
                i = cursor;
            } else {
                out.extend_from_slice("\u{FFFD}".as_bytes());
                i += 3;
            }
            continue;
        }
        out.push(rawurl[i]);
        i += 1;
    }
    out
}

fn unhex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => byte - b'A' + 10,
    }
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
