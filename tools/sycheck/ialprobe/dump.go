package main

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/88250/lute/ast"
	"github.com/88250/lute/parse"
	"github.com/siyuan-note/dataparser"
)

// Feed a hand-written .sy straight into dataparser and report what survives on inline nodes.
func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	opt := parse.NewOptions()
	opt.KramdownSpanIAL = true
	tree, needFix, err := dataparser.ParseJSON(data, opt)
	if err != nil {
		fmt.Println("PARSE ERROR:", err)
		return
	}
	fmt.Println("needFix:", needFix, "root:", tree.Root.ID)
	ast.Walk(tree.Root, func(n *ast.Node, entering bool) ast.WalkStatus {
		if !entering {
			return ast.WalkContinue
		}
		if len(n.Properties) == 0 && len(n.KramdownIAL) == 0 && n.Type != ast.NodeText {
			return ast.WalkContinue
		}
		props, _ := json.Marshal(n.Properties)
		ial, _ := json.Marshal(n.KramdownIAL)
		text := n.Data
		if len(text) > 40 {
			text = text[:40]
		}
		fmt.Printf("%-22s data=%-30q props=%s ial=%s\n", n.Type, text, props, ial)
		return ast.WalkContinue
	})
}
