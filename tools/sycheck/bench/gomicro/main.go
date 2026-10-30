package main

import (
	"encoding/json"
	"fmt"
	"runtime"
	"time"
)

var sink interface{}

func bench(label string, f func() interface{}) {
	// 预热
	f()
	N := 200000
	start := time.Now()
	for i := 0; i < N; i++ {
		sink = f()
	}
	ns := float64(time.Since(start).Nanoseconds()) / float64(N)
	fmt.Printf("%-42s %8.1f ns/op\n", label, ns)
	runtime.KeepAlive(sink)
}

func main() {
	bench("map 1 key", func() interface{} {
		return map[string]interface{}{"Type": "NodeParagraph"}
	})
	bench("map 2 keys", func() interface{} {
		return map[string]interface{}{"Type": "NodeParagraph", "Children": []interface{}{}}
	})
	bench("full paragraph node (text child)", func() interface{} {
		return map[string]interface{}{
			"Type": "NodeParagraph",
			"Children": []interface{}{
				map[string]interface{}{"Type": "NodeText", "Data": "hello world foo bar baz"},
			},
		}
	})
	node := map[string]interface{}{
		"Type": "NodeParagraph",
		"Children": []interface{}{
			map[string]interface{}{"Type": "NodeText", "Data": "hello world foo bar baz"},
		},
	}
	bench("json.Marshal(full paragraph node)", func() interface{} {
		b, _ := json.Marshal(node)
		return b
	})
}
