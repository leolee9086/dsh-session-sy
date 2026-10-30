//! Markdown 解析吞吐基准，对比 Go 版 Lute 的 RenderJSON。
//!
//! 用法：cargo run --release --example bench -- <fixtures.json> <rounds> [value]
//!
//! 默认走 `parse_to_json`（直出 JSON 文本），这才是和 Lute 的 RenderJSON 等价的工作量：
//! 两者都产出 JSON 文本。传第三个参数 value 时走 `parse`（构造 Value 树），
//! 用来观察两条路径差多少。

use serde_json::Value;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let rounds: usize = args[2].parse().expect("rounds 必须是数字");
    let mode = args.get(3).map(String::as_str).unwrap_or("json");

    let text = std::fs::read_to_string(path).expect("读不到 fixtures");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("fixtures 不是数组");

    let marks: Vec<&str> = cases
        .iter()
        .filter_map(|case| case.get("markdown").and_then(Value::as_str))
        .collect();

    // 预热一轮，排除首次调用的初始化开销
    for md in &marks {
        black_box(session_sy::md::parse_to_json(md));
    }

    let mut total = std::time::Duration::new(0, 0);
    let mut bytes = 0usize;
    for _ in 0..rounds {
        let start = Instant::now();
        for md in &marks {
            if mode == "value" {
                black_box(session_sy::md::parse(md));
            } else {
                // black_box 保证产物真的被算出来，而不是被优化掉
                let json = session_sy::md::parse_to_json(md);
                bytes += json.len();
                black_box(json);
            }
        }
        total += start.elapsed();
    }

    let avg = total.as_secs_f64() * 1000.0 / rounds as f64;
    println!(
        "rust cases={} rounds={} mode={} avg={:.2}ms out={}KB",
        marks.len(),
        rounds,
        mode,
        avg,
        black_box(bytes) / 1024
    );
}
