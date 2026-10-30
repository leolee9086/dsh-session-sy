//! GFM 表格解析，逐函数对照 Lute 的 `parse/table.go`。
//!
//! 这里只做"从段落 tokens 里识别表格并拆出单元格"，生成节点树由调用方负责。
//! 单元格内容仍以 tokens 形式返回，交给行内解析阶段处理（对应 Lute 的
//! `NodeTableCell` 走 `walkParseInline`）。

use super::util::trim_whitespace;

/// 解析出来的表格数据，尚未变成节点。
pub(super) struct Table {
    /// 每列对齐方式：0 默认、1 左、2 中、3 右。
    pub aligns: Vec<i64>,
    /// 表头行的单元格 tokens。
    pub head_cells: Vec<Vec<u8>>,
    /// 数据行，每行若干单元格 tokens。
    pub rows: Vec<Vec<Vec<u8>>>,
}

/// 对应 `lex.IsBlank`：全是空格（空切片也算）。
fn is_blank(tokens: &[u8]) -> bool {
    tokens.iter().all(|&b| b == b' ')
}

/// 对应 `lex.IsASCIIPunct`。
fn is_ascii_punct(token: u8) -> bool {
    (0x21..=0x2f).contains(&token)
        || (0x3a..=0x40).contains(&token)
        || (0x5b..=0x60).contains(&token)
        || (0x7b..=0x7e).contains(&token)
}

/// 对应 `lex.IsBackslashEscapePunct`：pos 处的字符是否被奇数个反斜杠转义。
fn is_backslash_escape_punct(tokens: &[u8], pos: usize) -> bool {
    if !is_ascii_punct(tokens[pos]) {
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

/// 对应 `lex.SplitWithoutBackslashEscape`：按分隔符切分，被转义的不算。
///
/// 注意末尾的空片段不加入（和 Go 版一致），中间的空片段会保留。
fn split_without_backslash_escape<'a>(tokens: &'a [u8], sep: u8) -> Vec<&'a [u8]> {
    let mut ret = Vec::new();
    let mut start = 0usize;
    for i in 0..tokens.len() {
        if tokens[i] != sep || is_backslash_escape_punct(tokens, i) {
            continue;
        }
        ret.push(&tokens[start..i]);
        start = i + 1;
    }
    if start < tokens.len() {
        ret.push(&tokens[start..]);
    }
    ret
}

/// 对应 `lex.Split`：按分隔符切分，末尾空片段不加入。
fn split<'a>(tokens: &'a [u8], sep: u8) -> Vec<&'a [u8]> {
    let mut ret = Vec::new();
    let mut start = 0usize;
    for i in 0..tokens.len() {
        if tokens[i] != sep {
            continue;
        }
        ret.push(&tokens[start..i]);
        start = i + 1;
    }
    if start < tokens.len() {
        ret.push(&tokens[start..]);
    }
    ret
}

/// 对应 table.go 的 `inInline`：判断位置 i 是否落在行内数学或代码标记内部。
fn in_inline(tokens: &[u8], i: usize, marker: u8) -> bool {
    if i + 1 >= tokens.len() || i < 1 {
        return false;
    }
    let head = &tokens[..i];
    let start = head.iter().position(|&b| b == marker);
    let count = head.iter().filter(|&&b| b == marker).count();
    if count % 2 == 0 {
        return false;
    }
    let end = tokens[i + 1..].iter().position(|&b| b == marker);
    start.is_some() && end.is_some()
}

/// 把 `\\|` 换成 `\\&#124;`（只在行内数学/代码里出现时才换），返回新内容。
fn protect_escaped_pipe(line: &[u8]) -> Option<Vec<u8>> {
    let idx = line.windows(2).position(|w| w == b"\\|")?;
    if idx == 0 {
        return None;
    }
    if !in_inline(line, idx, b'$') && !in_inline(line, idx, b'`') {
        return None;
    }
    let mut out = Vec::with_capacity(line.len() + 4);
    let mut i = 0usize;
    while i < line.len() {
        if i + 1 < line.len() && line[i] == b'\\' && line[i + 1] == b'|' {
            out.extend_from_slice(b"\\&#124;");
            i += 2;
            continue;
        }
        out.push(line[i]);
        i += 1;
    }
    Some(out)
}

/// 对应 `tableDelimAlign`：单个分隔列的解析结果，-1 表示不是合法分隔列。
fn table_delim_align(col: &[u8]) -> i64 {
    if col.is_empty() {
        return -1;
    }
    let left = col[0] == b':';
    let right = col[col.len() - 1] == b':';
    let mut i = 1usize;
    while i + 1 < col.len() {
        if col[i] != b'-' {
            return -1;
        }
        i += 1;
    }
    if left && right {
        2
    } else if left {
        1
    } else if right {
        3
    } else {
        0
    }
}

/// 对应 `parseTableDelimRow`：解析分隔行，返回每列对齐方式。
pub(super) fn parse_table_delim_row(line: &[u8]) -> Option<Vec<i64>> {
    if line.is_empty() {
        return None;
    }
    for &b in line {
        if !matches!(b, b'|' | b'-' | b':' | b' ') {
            return None;
        }
    }
    let owned = protect_escaped_pipe(line);
    let line: &[u8] = match &owned {
        Some(value) => value,
        None => line,
    };
    let mut cols = split_without_backslash_escape(line, b'|');
    if cols.is_empty() {
        return None;
    }
    if is_blank(cols[0]) {
        cols.remove(0);
    }
    if let Some(last) = cols.last() {
        if is_blank(last) {
            cols.pop();
        }
    }
    let mut aligns = Vec::with_capacity(cols.len());
    for col in cols {
        let col = trim_whitespace(col);
        let col = replace_entity_pipe(&col);
        if col.is_empty() {
            return None;
        }
        let align = table_delim_align(&col);
        if align == -1 {
            return None;
        }
        aligns.push(align);
    }
    Some(aligns)
}

/// 把 `&#124;` 还原成 `|`。
fn replace_entity_pipe(col: &[u8]) -> Vec<u8> {
    const NEEDLE: &[u8] = b"&#124;";
    if col.len() < NEEDLE.len() || !col.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
        return col.to_vec();
    }
    let mut out = Vec::with_capacity(col.len());
    let mut i = 0usize;
    while i < col.len() {
        if i + NEEDLE.len() <= col.len() && &col[i..i + NEEDLE.len()] == NEEDLE {
            out.push(b'|');
            i += NEEDLE.len();
            continue;
        }
        out.push(col[i]);
        i += 1;
    }
    out
}

/// 对应 `parseTableRow`：拆出一行的单元格 tokens。
///
/// 表头行的列数超过分隔行定义的列数时返回 None（不满足表格格式）。
pub(super) fn parse_table_row(
    line: &[u8],
    aligns: &[i64],
    is_head: bool,
) -> Option<Vec<Vec<u8>>> {
    let owned = protect_escaped_pipe(line);
    let line: &[u8] = match &owned {
        Some(value) => value,
        None => line,
    };

    let mut cols = split_without_backslash_escape(line, b'|');
    if cols.is_empty() {
        return None;
    }
    if is_blank(cols[0]) {
        cols.remove(0);
    }
    if let Some(last) = cols.last() {
        if is_blank(last) {
            cols.pop();
        }
    }

    let cols_len = cols.len();
    let aligns_len = aligns.len();
    if is_head && cols_len > aligns_len {
        return None;
    }

    let mut cells = Vec::with_capacity(aligns_len);
    let mut i = 0usize;
    while i < cols_len && i < aligns_len {
        let col = trim_whitespace(cols[i]);
        cells.push(replace_entity_pipe(&col));
        i += 1;
    }
    // 剩余列补空单元格
    while i < aligns_len {
        cells.push(Vec::new());
        i += 1;
    }
    Some(cells)
}

/// 对应 `parseTable0`：从一段 tokens 的起始处尝试解析表格。
pub(super) fn parse_table0(tokens: &[u8]) -> Option<Table> {
    let head_end = tokens.iter().position(|&b| b == b'\n')?;
    let body = &tokens[head_end + 1..];
    let delim_end = body
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(body.len());
    let delim_row = trim_whitespace(&body[..delim_end]);
    let body: &[u8] = if delim_end < body.len() {
        &body[delim_end + 1..]
    } else {
        &[]
    };
    if delim_row.len() < 2 {
        return None;
    }

    let aligns = parse_table_delim_row(&delim_row)?;

    if body.is_empty()
        && aligns.len() == 1
        && aligns[0] == 0
        && !tokens.contains(&b'|')
    {
        // 只有两行、默认对齐、又没有管道符时（foo\n---），会和 Setext 标题规则冲突，
        // 走到这里说明 Setext 已经失败，不能再当表格解析。
        return None;
    }

    let head_line = trim_whitespace(&tokens[..head_end]);
    let head_cells = parse_table_row(&head_line, &aligns, true)?;

    let mut rows = Vec::new();
    for line in split(body, b'\n') {
        let line = trim_whitespace(line);
        let row = parse_table_row(&line, &aligns, false)?;
        rows.push(row);
    }

    Some(Table {
        aligns,
        head_cells,
        rows,
    })
}

use serde_json::json;

use super::Parser;

impl Parser {
    /// 对应 Lute 的 `parseTable`：从段落 tokens 里识别表格并就地转换。
    ///
    /// 返回 true 表示这一段已经被表格占用，调用方不要再把 Tip 复位。
    pub(super) fn try_parse_table(&mut self, block: usize) -> bool {
        let tokens = self.nodes[block].tokens.clone();
        let mut i = 0usize;
        while i < tokens.len() {
            if i == 0 || tokens[i] == b'\n' {
                let start = if i == 0 { 0 } else { i + 1 };
                if let Some(table) = parse_table0(&tokens[start..]) {
                    return self.apply_table(block, &tokens, start, i, table);
                }
            }
            i += 1;
        }
        false
    }

    /// 把解析出来的表格数据变成节点树。
    ///
    /// 有前置内容时（offset > 0）段落只保留前面的部分，表格另起一个兄弟节点插在后面，
    /// 与 Lute 的 `p.InsertAfter(table)` 一致；否则整段直接变成表节点。
    fn apply_table(
        &mut self,
        block: usize,
        tokens: &[u8],
        start: usize,
        offset: usize,
        table: Table,
    ) -> bool {
        let aligns = table.aligns.clone();

        let head = self.new_node("NodeTableHead");
        let head_row = self.new_node("NodeTableRow");
        self.append_child(head, head_row);
        for (index, cell_tokens) in table.head_cells.iter().enumerate() {
            let cell = self.new_node("NodeTableCell");
            self.nodes[cell].tokens = cell_tokens.clone();
            self.set_cell_align(cell, aligns[index]);
            self.append_child(head_row, cell);
        }

        let mut data_rows = Vec::with_capacity(table.rows.len());
        for row in &table.rows {
            let table_row = self.new_node("NodeTableRow");
            self.nodes[table_row]
                .attrs
                .push(("TableAligns", json!(aligns)));
            for (index, cell_tokens) in row.iter().enumerate() {
                let cell = self.new_node("NodeTableCell");
                self.nodes[cell].tokens = cell_tokens.clone();
                self.set_cell_align(cell, aligns[index]);
                self.append_child(table_row, cell);
            }
            data_rows.push(table_row);
        }

        let target = if offset == 0 {
            block
        } else {
            let table_node = self.new_node("NodeTable");
            let parent = self.nodes[block].parent.unwrap_or(0);
            if let Some(position) = self.nodes[parent]
                .children
                .iter()
                .position(|child| *child == block)
            {
                self.nodes[parent].children.insert(position + 1, table_node);
            } else {
                self.append_child(parent, table_node);
            }
            self.nodes[table_node].parent = Some(parent);
            table_node
        };

        self.nodes[target].node_type = "NodeTable";
        self.nodes[target].attrs.push(("TableAligns", json!(aligns)));
        self.nodes[target].children.clear();
        self.append_child(target, head);
        for row in data_rows {
            self.append_child(target, row);
        }

        // 整段变表时 Tokens 原样保留（渲染成 Data）；有前置内容时各自留各自的。
        // 例外是 setext 那条路，它走完会自己清掉（对应 Lute SetextHeadingStart 里的
        // container.Tokens = nil）。
        if offset != 0 {
            self.nodes[block].tokens = tokens[..offset].to_vec();
            self.nodes[target].tokens = tokens[start..].to_vec();
        }

        self.nodes[target].close = true;
        self.tip = Some(target);
        true
    }

    /// TableCellAlign 是 omitempty：0 表示默认对齐，不写进 JSON。
    fn set_cell_align(&mut self, cell: usize, align: i64) {
        if align != 0 {
            self.nodes[cell].attrs.push(("TableCellAlign", json!(align)));
        }
    }
}

