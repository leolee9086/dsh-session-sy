package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"time"

	"github.com/88250/lute"
)

type fixture struct {
	Example  int    `json:"example"`
	Section  string `json:"section"`
	Markdown string `json:"markdown"`
}

// 用和 mdprobe 完全相同的引擎配置，测 markdown -> JSON 的吞吐。
// 对比对象是 Rust 的 session_sy::md::parse（解析 + 渲染，同等工作量）。
func main() {
	path := os.Args[1]
	rounds, _ := strconv.Atoi(os.Args[2])
	data, err := os.ReadFile(path)
	if nil != err {
		panic(err)
	}
	var cases []fixture
	if err = json.Unmarshal(data, &cases); nil != err {
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

	// 预热一轮，排除首次调用的初始化开销
	for _, c := range cases {
		l.RenderJSON(c.Markdown)
	}

	var total time.Duration
	// 累加输出字节数：用来确认两边产出的 JSON 体量一致，排除"少写内容所以更快"的可能
	var totalBytes int
	for r := 0; r < rounds; r++ {
		start := time.Now()
		for _, c := range cases {
			totalBytes += len(l.RenderJSON(c.Markdown))
		}
		total += time.Since(start)
	}
	avg := float64(total.Nanoseconds()) / float64(rounds) / 1e6
	fmt.Printf("go   cases=%d rounds=%d avg=%.2fms out=%dKB\n", len(cases), rounds, avg, totalBytes/1024)
}
