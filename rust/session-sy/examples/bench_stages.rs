//! 分阶段基准：直出路径里块解析占多少、JSON 渲染占多少。
//!
//! 用法：cargo run --release --example bench_stages -- <fixtures.json> <rounds> [times]
//!
//! times 是重复测量次数，取最小值——这台机器有背景负载，均值会被噪声抬高。

use serde_json::Value;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let rounds: usize = args[2].parse().expect("rounds 必须是数字");
    let times: usize = args.get(3).map(|v| v.parse().expect("times 必须是数字")).unwrap_or(1);

    let text = std::fs::read_to_string(path).expect("读不到 fixtures");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("fixtures 不是数组");
    let marks: Vec<&str> = cases
        .iter()
        .filter_map(|c| c.get("markdown").and_then(Value::as_str))
        .collect();

    for md in &marks {
        black_box(session_sy::md::parse_to_json(md));
    }

    let mut best_total = u64::MAX;
    let mut best_blocks = u64::MAX;
    let mut out_bytes = 0usize;
    for _ in 0..times {
        let mut total = 0u64;
        let mut blocks = 0u64;
        for _ in 0..rounds {
            for md in &marks {
                let t = Instant::now();
                let json = session_sy::md::parse_to_json(md);
                total += t.elapsed().as_nanos() as u64;
                out_bytes += json.len();
                black_box(json);
                // 只跑块解析那一段（诊断函数内部自己计时）
                blocks += session_sy::md::bench_blocks_nanos(md);
            }
        }
        best_total = best_total.min(total);
        best_blocks = best_blocks.min(blocks);
    }

    let n = rounds as f64;
    let total_ms = best_total as f64 / 1e6 / n;
    let blocks_ms = best_blocks as f64 / 1e6 / n;
    println!(
        "cases={} rounds={} best_of={} total={:.2}ms blocks={:.2}ms ({:.0}%) render_json={:.2}ms per_MB_out={:.1}ms",
        marks.len(),
        rounds,
        times,
        total_ms,
        blocks_ms,
        blocks_ms / total_ms * 100.0,
        total_ms - blocks_ms,
        (total_ms - blocks_ms) / (out_bytes as f64 / 1024.0 / 1024.0 / (rounds * times) as f64)
    );
}
