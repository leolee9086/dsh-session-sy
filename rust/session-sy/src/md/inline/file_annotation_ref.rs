//! 文件注解引用，对应 Lute 的 parse/file_annotation_ref.go。
//!
//! 形如 `<<assets/x.pdf/20210911230820-lhiaysx "锚文本">>`：两个小于号开头、
//! 一个 assets/ 下的资源路径、可选的双引号锚文本、两个大于号结尾。

use super::block_ref::is_node_id_pattern;
use super::{is_whitespace, scan_link_title, Parser};

/// 对应 parseFileAnnotationRefID：返回 (passed 长度, remains 起点, id)。
///
/// `passed` 可能比 remains 起点长——ID 后面直接跟 `>>` 时那两个字节也算进了 passed。
fn parse_file_annotation_ref_id(tokens: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let length = tokens.len();
    if length == 0 {
        return None;
    }
    let mut i = 0usize;
    while i < length {
        let token = tokens[i];
        // 插入符是编辑器光标，跳过
        if token == 0xe2 && tokens.get(i + 1) == Some(&0x80) && tokens.get(i + 2) == Some(&0xb8) {
            i += 3;
            continue;
        }
        // 遇到嵌套起始符立即拒绝，避免连续无效输入反复扫描剩余文本
        if token == b'<' {
            return None;
        }
        if tokens[i..].starts_with(b" \"") {
            break;
        }
        if token == b'>' {
            break;
        }
        i += 1;
    }
    let remains_at = i;
    let id_part = &tokens[..i];
    if id_part
        .iter()
        .any(|b| matches!(b, b'<' | b'>' | b'"' | b'\r' | b'\n' | b'\\'))
    {
        return None;
    }
    let Some(after_assets) = id_part.strip_prefix(b"assets/") else {
        return None;
    };
    // 查询参数和片段属于资源定位信息，不参与路径与标注 ID 校验
    let core = match after_assets
        .iter()
        .position(|b| *b == b'?' || *b == b'#')
    {
        Some(at) => &after_assets[..at],
        None => after_assets,
    };
    let separator = core.iter().rposition(|b| *b == b'/')?;
    if separator == 0 {
        return None;
    }
    let file_part = &core[..separator];
    if !file_part.to_ascii_lowercase().ends_with(b".pdf") {
        return None;
    }
    // 对应 Lute 的 `4 >= len(filePart) - LastIndexByte(filePart, '/') - 1`：
    // 没有斜杠时 LastIndexByte 返回 -1，所以这里也要按 -1 算，不能按 0。
    let last_slash = file_part
        .iter()
        .rposition(|b| *b == b'/')
        .map_or(-1i64, |at| at as i64);
    if 4 >= file_part.len() as i64 - last_slash - 1 {
        return None;
    }
    for part in file_part.split(|b| *b == b'/') {
        if part.is_empty() || part == b"." || part == b".." {
            return None;
        }
    }
    if !is_node_id_pattern(&core[separator + 1..]) {
        return None;
    }

    let id = tokens[..i].to_vec();
    if length - remains_at < 2 {
        return None;
    }
    let mut passed = id.len();
    let mut at = remains_at;
    // 插入符也计入 passed
    if tokens[at..].starts_with(&[0xe2, 0x80, 0xb8]) {
        passed += 3;
        at += 3;
    }
    if tokens.get(at) == Some(&b'>') && tokens.get(at + 1) == Some(&b'>') {
        passed += 2;
        return Some((passed, remains_at, id));
    }
    if !is_whitespace(tokens[at]) {
        return None;
    }
    Some((passed, remains_at, id))
}

impl<'a> Parser<'a> {
    /// 对应 parseFileAnnotationRef：成功时把节点挂好并返回 true。
    pub(super) fn parse_file_annotation_ref(&mut self) -> bool {
        if !self.options.file_annotation_ref {
            return false;
        }
        if self.pos + 1 >= self.tokens.len()
            || self.tokens[self.pos] != b'<'
            || self.tokens[self.pos + 1] != b'<'
        {
            return false;
        }

        let save_pos = self.pos;
        let mut pos = self.pos + 2;
        let mut id: Vec<u8> = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        let mut matched = false;

        loop {
            // 跳过空白
            while pos < self.tokens.len() && is_whitespace(self.tokens[pos]) {
                pos += 1;
            }
            let Some((passed, remains_at, id_value)) =
                parse_file_annotation_ref_id(&self.tokens[pos..])
            else {
                break;
            };
            if passed == 0 {
                break;
            }
            id = id_value;
            // 先把 passed 整体跳过，再看它末尾是不是 >>
            let passed_at = pos;
            pos += passed;
            if self.tokens.get(pos - 1) == Some(&b'>') && self.tokens.get(pos - 2) == Some(&b'>') {
                matched = true;
                break;
            }
            let remains_at_abs = passed_at + remains_at;
            if remains_at_abs >= self.tokens.len() || !is_whitespace(self.tokens[remains_at_abs]) {
                break;
            }
            // 跟空白：可能是 >> 直接闭合，也可能先来一段标题
            let mut after_space = remains_at_abs;
            while after_space < self.tokens.len() && is_whitespace(self.tokens[after_space]) {
                after_space += 1;
            }
            if self.tokens.get(after_space) == Some(&b'>')
                && self.tokens.get(after_space + 1) == Some(&b'>')
            {
                pos = after_space + 2;
                matched = true;
                break;
            }
            let Some((title_value, title_end)) = scan_link_title(&self.tokens, after_space) else {
                break;
            };
            text = title_value;
            let mut tail = title_end;
            while tail < self.tokens.len() && is_whitespace(self.tokens[tail]) {
                tail += 1;
            }
            if self.tokens.get(tail) == Some(&b'>') && self.tokens.get(tail + 1) == Some(&b'>') {
                pos = tail + 2;
                matched = true;
            }
            break;
        }

        if !matched {
            self.pos = save_pos;
            return false;
        }
        self.pos = pos;

        let node = self.new_node("NodeFileAnnotationRef");
        let open1 = self.new_node("NodeLess");
        let open2 = self.new_node("NodeLess");
        let id_node = self.new_node("NodeFileAnnotationRefID");
        self.nodes[id_node].data = id;
        let mut children = vec![open1, open2, id_node];
        if !text.is_empty() {
            let space = self.new_node("NodeFileAnnotationRefSpace");
            let text_node = self.new_node("NodeFileAnnotationRefText");
            self.nodes[text_node].data = text;
            children.push(space);
            children.push(text_node);
        }
        let close1 = self.new_node("NodeGreater");
        let close2 = self.new_node("NodeGreater");
        children.push(close1);
        children.push(close2);
        self.nodes[node].children = children;
        self.append(node);
        true
    }
}
