package main

// renderprobe 是渲染器侧的基准生成器：用真正的 Go Lute 算出每种渲染方法的期望输出。
//
// 为什么不能只用 lute.min.js 当判据：Vditor 4.0.1 内置的是 lute.min.js 1.7.6，
// 而思源内核和本模块缓存都是 1.7.9。两个版本在渲染细节上真的不一样
// （例如表格对齐 1.7.6 输出 align="left"，1.7.9 改成 style="text-align: left"）。
// 拿 1.7.6 的二进制去判 1.7.9 的移植，会凭空造出一批假差异，
// 所以渲染器等价测试一律以本工具为准；lute.min.js 只用来单独核对 Vditor 兼容性。
//
// 用法：renderprobe <用例.json> <输出.json> [配置名]
// 配置名：
//   vditor   —— 逐项对齐 Vditor 的 src/ts/markdown/setLute.ts（默认）
//   siyuan   —— 逐项对齐思源 kernel/util/lute.go 的 NewLute()
//   default  —— lute.New() 的原始默认值
//
// 输出每条用例的 md2html / vditor / ir / sv / json 五个字段。

import (
	"encoding/json"
	"os"

	"github.com/88250/lute"
)

type testcase struct {
	Example  int    `json:"example"`
	Section  string `json:"section"`
	File     string `json:"file"`
	Markdown string `json:"markdown"`
}

type fixture struct {
	Example  int    `json:"example"`
	Section  string `json:"section"`
	File     string `json:"file"`
	Markdown string `json:"markdown"`
	Md2html  string `json:"md2html"`
	Vditor   string `json:"vditor"`
	Ir       string `json:"ir"`
	Sv       string `json:"sv"`
	JSON     string `json:"json"`
}

// newVditorLute 逐项对照 Vditor 的 setLute.ts。
//
// 这里只设 setLute 里真的会设的项；setLute 里没有 SetVditorWYSIWYG，
// 因为那个开关会改变解析器行为（链接目标保留反斜杠），而 Vditor 从不设它。
func newVditorLute() *lute.Lute {
	l := lute.New()
	l.SetEmojiSite("https://cdn.jsdelivr.net/npm/vditor/dist/images/emoji")
	l.SetHeadingAnchor(true)
	l.SetInlineMathAllowDigitAfterOpenMarker(true)
	l.SetAutoSpace(true)
	l.SetCallout(true)
	l.SetInlineMath(true)
	l.SetToC(true)
	l.SetFootnotes(true)
	l.SetFixTermTypo(true)
	l.SetVditorCodeBlockPreview(true)
	l.SetVditorMathBlockPreview(true)
	l.SetSanitize(true)
	l.SetChineseParagraphBeginningSpace(false)
	l.SetRenderListStyle(false)
	l.SetGFMAutoLink(true)
	l.SetImageLazyLoading("")
	l.SetSup(true)
	l.SetSub(true)
	return l
}

// newSiYuanLute 逐项对照思源 kernel/util/lute.go 的 NewLute()。
func newSiYuanLute() *lute.Lute {
	l := lute.New()
	l.ParseOptions.KeepEscaped = true
	l.SetTextMark(true)
	l.SetProtyleWYSIWYG(true)
	l.SetBlockRef(true)
	l.SetFileAnnotationRef(true)
	l.SetKramdownIAL(true)
	l.SetTag(true)
	l.SetSuperBlock(true)
	l.SetCustomBlock(true)
	l.SetImgPathAllowSpace(true)
	l.SetGitConflict(true)
	l.SetInlineAsterisk(true)
	l.SetInlineUnderscore(true)
	l.SetSup(true)
	l.SetSub(true)
	l.SetInlineMath(true)
	l.SetGFMStrikethrough(true)
	l.SetFullWidthStrikethrough(false)
	l.SetMark(true)
	l.SetInlineMathAllowDigitAfterOpenMarker(true)
	l.SetGFMStrikethrough1(false)
	l.SetFootnotes(false)
	l.SetToC(false)
	l.SetIndentCodeBlock(false)
	l.SetParagraphBeginningSpace(true)
	l.SetAutoSpace(false)
	l.SetHeadingID(false)
	l.SetSetext(false)
	l.SetYamlFrontMatter(false)
	l.SetLinkRef(false)
	l.SetCodeSyntaxHighlight(false)
	l.SetSanitize(true)
	l.SetUnorderedListMarker("-")
	l.SetCallout(true)
	l.SetTabs(true)
	l.SetDataTask(true)
	l.SetArbitraryTaskListItemMarker(true)
	l.SetExportNormalizeTaskListMarker(false)
	l.SetEnsureListItemParagraph(true)
	return l
}

func main() {
	if len(os.Args) < 3 {
		panic("用法: renderprobe <用例.json> <输出.json> [vditor|siyuan|default]")
	}
	bytes, err := os.ReadFile(os.Args[1])
	if nil != err {
		panic(err)
	}
	var cases []testcase
	if err = json.Unmarshal(bytes, &cases); nil != err {
		panic(err)
	}
	config := "vditor"
	if 3 < len(os.Args) {
		config = os.Args[3]
	}
	var l *lute.Lute
	switch config {
	case "vditor":
		l = newVditorLute()
	case "siyuan":
		l = newSiYuanLute()
	case "default":
		l = lute.New()
	case "noial":
		// 显式关掉块级 IAL，用来验证那个开关到底管不管用
		l = lute.New()
		l.ParseOptions.KramdownBlockIAL = false
		l.ParseOptions.KramdownSpanIAL = false
	case "ial":
		l = lute.New()
		l.ParseOptions.KramdownBlockIAL = true
	default:
		panic("未知配置名: " + config)
	}

	out := make([]fixture, 0, len(cases))
	// 把关键开关打出来，免得靠读代码猜默认值
	println("KramdownBlockIAL:", l.ParseOptions.KramdownBlockIAL,
		"KramdownSpanIAL:", l.ParseOptions.KramdownSpanIAL,
		"CodeSyntaxHighlight:", l.RenderOptions.CodeSyntaxHighlight)
	for _, c := range cases {
		f := fixture{Example: c.Example, Section: c.Section, File: c.File, Markdown: c.Markdown}
		f.Md2html = l.Md2HTML(c.Markdown)
		f.Vditor = l.Md2VditorDOM(c.Markdown)
		f.Ir = l.Md2VditorIRDOM(c.Markdown)
		f.Sv = l.Md2VditorSVDOM(c.Markdown)
		f.JSON = l.RenderJSON(c.Markdown)
		out = append(out, f)
	}
	b, err := json.MarshalIndent(out, "", " ")
	if nil != err {
		panic(err)
	}
	if err = os.WriteFile(os.Args[2], b, 0644); nil != err {
		panic(err)
	}
	println("config:", config, "cases:", len(out))
}
