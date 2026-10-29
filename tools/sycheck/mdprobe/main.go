package main

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

// 把 CommonMark 规范用例跑成 Lute 的 AST，作为 Rust 复刻的等价测试基准。
func main() {
	bytes, err := os.ReadFile(os.Args[1])
	if nil != err {
		panic(err)
	}
	var cases []testcase
	if err = json.Unmarshal(bytes, &cases); nil != err {
		panic(err)
	}
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
	println("cases:", len(out))
}
