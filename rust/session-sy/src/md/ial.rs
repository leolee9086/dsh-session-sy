//! 段落里的 kramdown 块级 IAL 合并。
//!
//! 对应 Lute `inlines.go` 里 `walkParseInline` 调用 `parseKramdownIALInListItem` 的那一段：
//! **这一处不检查 `KramdownBlockIAL` 开关**，所以即使配置里关着，
//! 以 `{: ` 开头的段落也会被吃掉、属性挂到前一个兄弟块上。

use super::inline::html::tag_attr;

/// 对应 `parseKramdownIALInListItem`：tokens 必须以 `{:` 开头，且至少解析出一个属性。
pub(super) fn parse_block_ial(tokens: &[u8]) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    if !tokens.starts_with(b"{:") {
        return None;
    }
    let body = &tokens[2..];
    let end = body.iter().position(|&b| b == b'}')?;
    if end < 3 {
        return None;
    }
    // 对应 tokens[:bytes.Index(tokens, "}")]：} 之后的内容一律丢弃
    let mut rest = &body[..end];
    let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    loop {
        let (valid, remains, attr, name, value) = tag_attr(rest, 0);
        if !valid || attr.is_empty() {
            break;
        }
        pairs.push((name, value));
        rest = &rest[remains..];
    }
    if pairs.is_empty() {
        None
    } else {
        Some(pairs)
    }
}
