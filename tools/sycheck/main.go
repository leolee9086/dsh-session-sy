// sycheck 用思源自己的解析器读 .sy，验证生成的文档思源内核能接受。
//
// kernel/filesys/tree.go 里就是这么做的：dataparser.ParseJSON(jsonData, luteEngine.ParseOptions)。
// 这个程序复刻那一步，不自己实现格式判断。
package main

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/88250/lute"
	"github.com/88250/lute/ast"
	"github.com/siyuan-note/dataparser"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "用法: sycheck <file.sy> [more.sy ...]")
		os.Exit(2)
	}

	engine := lute.New()
	failures := 0
	for _, path := range os.Args[1:] {
		data, err := os.ReadFile(path)
		if err != nil {
			fmt.Fprintf(os.Stderr, "%s: 读取失败: %v\n", path, err)
			failures++
			continue
		}
		tree, needFix, err := dataparser.ParseJSON(data, engine.ParseOptions)
		if err != nil {
			fmt.Fprintf(os.Stderr, "%s: 解析失败: %v\n", path, err)
			failures++
			continue
		}
		report, err := json.Marshal(map[string]any{
			"file": path, "id": tree.ID, "blocks": countBlocks(tree.Root), "needFix": needFix,
		})
		if err != nil {
			fmt.Fprintf(os.Stderr, "%s: 结果序列化失败: %v\n", path, err)
			failures++
			continue
		}
		fmt.Println(string(report))
	}
	if failures > 0 {
		os.Exit(1)
	}
}

// countBlocks 数带 ID 的块节点，用来确认树真的建起来了。
func countBlocks(node *ast.Node) int {
	if node == nil {
		return 0
	}
	total := 0
	if node.IsBlock() && node.ID != "" {
		total++
	}
	for child := node.FirstChild; child != nil; child = child.Next {
		total += countBlocks(child)
	}
	return total
}
