//! 解析配置，逐字段对齐 Lute 的 `parse.Options`。
//!
//! 同一个引擎在不同开关下会产出不同的 AST，所以"和 Lute 等价"必须说清楚是
//! 在哪套配置下等价。这里提供三套预设，对应 Lute 的三个真实使用场景：
//!
//! - [`Options::commonmark`]：只留 CommonMark 规范，GFM 和思源扩展全关。
//!   早期那 652 条 fixtures 就是这套配置生成的。
//! - [`Options::siyuan`]：SiYuan 内核 `kernel/util/lute.go` 的 `NewLute()`。
//! - [`Options::default`]：Lute 的 `parse.NewOptions()` 默认值，也就是 Vditor 的基础。

/// 解析选项。字段名与 Lute 的 `Options` 一一对应，注释说明该开关在 Lute 里的作用。
#[derive(Debug, Clone)]
pub struct Options {
    /// GFM 表格。
    pub gfm_table: bool,
    /// GFM 任务列表项 `- [ ]`。
    pub gfm_task_list_item: bool,
    /// GFM 删除线 `~~x~~`。
    pub gfm_strikethrough: bool,
    /// 单标记符删除线 `~x~`。
    pub gfm_strikethrough1: bool,
    /// 全角波浪号删除线。
    pub full_width_strikethrough: bool,
    /// GFM 自动链接。
    pub gfm_auto_link: bool,
    /// 脚注。
    pub footnotes: bool,
    /// 自定义标题 ID `# foo {id}`。
    pub heading_id: bool,
    /// 目录 `[toc]`。
    pub toc: bool,
    /// Emoji 别名替换。
    pub emoji: bool,
    /// Vditor 所见即所得模式。
    pub vditor_wysiwyg: bool,
    /// Vditor 即时渲染模式。
    pub vditor_ir: bool,
    /// Vditor 分屏预览模式。
    pub vditor_sv: bool,
    /// Protyle 所见即所得模式。
    pub protyle_wysiwyg: bool,
    /// Protyle 模式下启用自动链接。
    pub protyle_wysiwyg_auto_link: bool,
    /// 行级公式 `$x$`。
    pub inline_math: bool,
    /// 内联公式允许起始 `$` 后紧跟数字。
    pub inline_math_allow_digit_after_open_marker: bool,
    /// Setext 标题 `foo\n===`。
    pub setext: bool,
    /// YAML Front Matter。
    pub yaml_front_matter: bool,
    /// 内容块引用 `((id "文本"))`。
    pub block_ref: bool,
    /// 文件注解引用。
    pub file_annotation_ref: bool,
    /// `==标记==`。
    pub mark: bool,
    /// kramdown 块级内联属性列表。
    pub kramdown_block_ial: bool,
    /// kramdown 行级内联属性列表。
    pub kramdown_span_ial: bool,
    /// `#标签#`。
    pub tag: bool,
    /// 图片路径允许带空格。
    pub img_path_allow_space: bool,
    /// 超级块 `{{{ }}}`。
    pub super_block: bool,
    /// 自定义块。
    pub custom_block: bool,
    /// 上标 `^x^`。
    pub sup: bool,
    /// 下标 `~x~`。
    pub sub: bool,
    /// 行级 `*` 语法。
    pub inline_asterisk: bool,
    /// 行级 `_` 语法。
    pub inline_underscore: bool,
    /// Git 冲突标记。
    pub git_conflict: bool,
    /// 链接引用定义。
    pub link_ref: bool,
    /// 缩进代码块。
    pub indent_code_block: bool,
    /// 段首空格保留（影响段落 tokens 的裁剪方式）。
    pub paragraph_beginning_space: bool,
    /// `![foo](data:image...)` 形式的图片。
    pub data_image: bool,
    /// 通用行级节点（TextMark），高亮/kbd/下划线等走这里。
    pub text_mark: bool,
    /// 若干 HTML 标签转成 TextMark。
    pub html_tag2_text_mark: bool,
    /// 自旋解析，仅内部流程使用。
    pub spin: bool,
    /// 提示块 `> [!NOTE]`。
    pub callout: bool,
    /// 页签容器。
    pub tabs: bool,
    /// 保留转义内容，不做反转义。
    pub keep_escaped: bool,
    /// 任务列表接受任意标记符（`[/]`、`[>]` 等）。
    pub arbitrary_task_list_item_marker: bool,
    /// 空列表项下建子列表前补一个空段落。
    pub ensure_list_item_paragraph: bool,
    /// 禁止在富文本片段内部递归展开单元格富文本。
    pub disable_table_cell_rich: bool,
}

impl Default for Options {
    /// 对齐 Lute 的 `parse.NewOptions()`。
    fn default() -> Self {
        Options {
            gfm_table: true,
            gfm_task_list_item: true,
            gfm_strikethrough: true,
            gfm_strikethrough1: true,
            full_width_strikethrough: false,
            gfm_auto_link: true,
            footnotes: true,
            heading_id: true,
            toc: false,
            emoji: true,
            vditor_wysiwyg: false,
            vditor_ir: false,
            vditor_sv: false,
            protyle_wysiwyg: false,
            protyle_wysiwyg_auto_link: false,
            inline_math: true,
            inline_math_allow_digit_after_open_marker: false,
            setext: true,
            yaml_front_matter: true,
            block_ref: false,
            file_annotation_ref: false,
            mark: false,
            kramdown_block_ial: false,
            kramdown_span_ial: false,
            tag: false,
            img_path_allow_space: false,
            super_block: false,
            custom_block: false,
            sup: false,
            sub: false,
            inline_asterisk: true,
            inline_underscore: true,
            git_conflict: false,
            link_ref: true,
            indent_code_block: true,
            paragraph_beginning_space: false,
            data_image: true,
            text_mark: false,
            html_tag2_text_mark: false,
            spin: false,
            callout: false,
            tabs: false,
            keep_escaped: false,
            arbitrary_task_list_item_marker: false,
            ensure_list_item_paragraph: false,
            disable_table_cell_rich: false,
        }
    }
}

impl Options {
    /// 早期那套"GFM 全关"配置，652 条 CommonMark fixtures 就是它生成的。
    ///
    /// 对应 `mdprobe` 里逐个关掉 GFM、HeadingID、Emoji、YamlFrontMatter 的那份设置。
    pub fn commonmark() -> Self {
        Options {
            gfm_table: false,
            gfm_task_list_item: false,
            gfm_auto_link: false,
            gfm_strikethrough: false,
            heading_id: false,
            emoji: false,
            yaml_front_matter: false,
            ..Options::default()
        }
    }

    /// SiYuan 内核 `kernel/util/lute.go` 的 `NewLute()`。
    pub fn siyuan() -> Self {
        Options {
            keep_escaped: true,
            text_mark: true,
            protyle_wysiwyg: true,
            block_ref: true,
            file_annotation_ref: true,
            kramdown_block_ial: true,
            kramdown_span_ial: true,
            tag: true,
            super_block: true,
            custom_block: true,
            img_path_allow_space: true,
            git_conflict: true,
            sup: true,
            sub: true,
            inline_math: true,
            gfm_strikethrough: true,
            full_width_strikethrough: false,
            mark: true,
            inline_math_allow_digit_after_open_marker: true,
            gfm_strikethrough1: false,
            footnotes: false,
            toc: false,
            indent_code_block: false,
            paragraph_beginning_space: true,
            // 下面这些在 NewLute() 里被显式关掉，与默认值相反
            setext: false,
            yaml_front_matter: false,
            heading_id: false,
            link_ref: false,
            callout: true,
            tabs: true,
            arbitrary_task_list_item_marker: true,
            ensure_list_item_paragraph: true,
            ..Options::default()
        }
    }
}
