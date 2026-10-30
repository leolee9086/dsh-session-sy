package main

// 把 CommonMark 规范用例跑成 Lute 的 AST，作为 Rust 复刻的等价测试基准。
//
// 用法：mdprobe <输入用例.json> <输出 fixtures.json> [配置名]
// 配置名：
//   commonmark —— 早期那套"GFM 全关"配置（默认，保持向后兼容）
//   siyuan     —— 逐项对齐 SiYuan 内核 kernel/util/lute.go 的 NewLute()
//
// 同一份 markdown 在不同配置下会产出不同的 AST，所以 fixtures 必须标清楚是哪个配置生成的。

import (
	"encoding/json"
	"os"

	"github.com/88250/lute"
)

type testcase struct {
	Section  string `json:"section"`
	Markdown string `json:"markdown"`
	Example  int    `json:"example"`
}

type fixture struct {
	Example  int    `json:"example"`
	Section  string `json:"section"`
	Markdown string `json:"markdown"`
	Tree     string `json:"tree"`
}

// newCommonMarkLute 是早期那套配置：只留 CommonMark，GFM 和思源扩展全关。
func newCommonMarkLute() *lute.Lute {
	l := lute.New()
	l.ParseOptions.GFMTaskListItem = false
	l.ParseOptions.GFMTable = false
	l.ParseOptions.GFMAutoLink = false
	l.ParseOptions.GFMStrikethrough = false
	l.RenderOptions.SoftBreak2HardBreak = false
	l.RenderOptions.CodeSyntaxHighlight = false
	l.ParseOptions.HeadingID = false
	l.RenderOptions.HeadingID = false
	l.RenderOptions.AutoSpace = false
	l.RenderOptions.FixTermTypo = false
	l.ParseOptions.Emoji = false
	l.ParseOptions.YamlFrontMatter = false
	return l
}

// newSiYuanLute 逐项对照 SiYuan 内核 kernel/util/lute.go 的 NewLute()。
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
	bytes, err := os.ReadFile(os.Args[1])
	if nil != err {
		panic(err)
	}
	var cases []testcase
	if err = json.Unmarshal(bytes, &cases); nil != err {
		panic(err)
	}
	config := "commonmark"
	if 3 < len(os.Args) {
		config = os.Args[3]
	}
	var l *lute.Lute
	switch config {
	case "siyuan":
		l = newSiYuanLute()
	case "commonmark":
		l = newCommonMarkLute()
	case "default":
		// Lute 的 parse.NewOptions() 默认值，也就是 Vditor 的基础配置
		l = lute.New()
	default:
		panic("未知配置名: " + config)
	}

	out := make([]fixture, 0, len(cases))
	for _, c := range cases {
		out = append(out, fixture{Example: c.Example, Section: c.Section, Markdown: c.Markdown, Tree: l.RenderJSON(c.Markdown)})
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
