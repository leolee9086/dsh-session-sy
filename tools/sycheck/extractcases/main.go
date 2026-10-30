// 从 Lute 的 test/*.go 里提取所有 parseTest 用例的输入 markdown，
// 用 Lute 自己生成两套配置下的 JSON 期望树，输出成 fixtures。
//
// 为什么不用正则：测试里有大量反引号字符串和多行字面量，正则必然出错，
// 用 go/parser 拿到真实的字符串值最稳。
package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strconv"

	"github.com/88250/lute"
)

const testDir = `C:\Users\al765\go\pkg\mod\github.com\88250\lute@v1.7.9-0.20260926021905-c8cee1b0c658\test`

const outPath = `D:\dev\session-sy\.tmp-import\lute-cases.json`

type fixture struct {
	Example  int    `json:"example"`
	Section  string `json:"section"`
	File     string `json:"file"`
	Markdown string `json:"markdown"`
	Default  string `json:"default"`
	Extended string `json:"extended"`
}

func main() {
	fset := token.NewFileSet()
	type entry struct{ name, file, markdown string }
	var entries []entry
	seen := map[string]bool{}

	files, err := filepath.Glob(filepath.Join(testDir, "*.go"))
	if err != nil {
		panic(err)
	}
	for _, file := range files {
		f, err := parser.ParseFile(fset, file, nil, 0)
		if err != nil {
			fmt.Fprintln(os.Stderr, "parse skip:", filepath.Base(file), err)
			continue
		}
		ast.Inspect(f, func(n ast.Node) bool {
			lit, ok := n.(*ast.CompositeLit)
			if !ok || len(lit.Elts) < 3 {
				return true
			}
			vals := make([]string, 0, 3)
			for _, elt := range lit.Elts[:3] {
				bl, ok := elt.(*ast.BasicLit)
				if !ok || bl.Kind != token.STRING {
					return true
				}
				s, err := strconv.Unquote(bl.Value)
				if err != nil {
					return true
				}
				vals = append(vals, s)
			}
			key := vals[0] + "\x00" + vals[1]
			if seen[key] {
				return true
			}
			seen[key] = true
			entries = append(entries, entry{name: vals[0], file: filepath.Base(file), markdown: vals[1]})
			return true
		})
	}
	fmt.Println("提取用例:", len(entries))

	defaultEngine := lute.New()
	extendedEngine := lute.New()
	extendedEngine.SetSup(true)
	extendedEngine.SetSub(true)
	extendedEngine.SetMark(true)
	extendedEngine.SetBlockRef(true)
	extendedEngine.SetTag(true)
	extendedEngine.SetFileAnnotationRef(true)

	out := make([]fixture, 0, len(entries))
	for i, e := range entries {
		out = append(out, fixture{
			Example:  i,
			Section:  e.name,
			File:     e.file,
			Markdown: e.markdown,
			Default:  defaultEngine.RenderJSON(e.markdown),
			Extended: extendedEngine.RenderJSON(e.markdown),
		})
	}

	data, err := json.Marshal(out)
	if err != nil {
		panic(err)
	}
	if err := os.WriteFile(outPath, data, 0o644); err != nil {
		panic(err)
	}
	fmt.Println("写出:", outPath, len(data), "字节")
}
