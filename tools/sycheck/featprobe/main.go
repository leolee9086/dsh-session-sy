package main

// 一次性探测：把命令行给的 markdown 用 Lute 默认配置渲染成 JSON。
// 第二个参数为 noauto 时关掉 GFM 自动链接，用来判断某段文本是不是被自动链接吃掉的。

import (
	"fmt"
	"os"

	"github.com/88250/lute"
)

func main() {
	l := lute.New()
	if len(os.Args) > 2 && os.Args[2] == "noauto" {
		l.SetGFMAutoLink(false)
	}
	if len(os.Args) > 1 {
		fmt.Println(l.RenderJSON(os.Args[1]))
		return
	}
	fmt.Println("用法: featprobe <markdown> [noauto]")
}
