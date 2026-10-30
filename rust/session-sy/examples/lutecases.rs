// 用 Lute 自己 test/*.go 里提取出来的用例，检查我们的 JSON 等价度。
//
// 输入文件由 tools/sycheck/extractcases 生成，每条带着 default 与 extended
// 两套配置下 Lute 的 RenderJSON 结果，以及它来自哪个测试文件。

use serde_json::Value;
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "D:\\dev\\session-sy\\.tmp-import\\lute-cases.json".to_string());
    let text = std::fs::read_to_string(&path).expect("读不到用例文件");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("不是 JSON 数组");

    let default_options = session_sy::md::Options::default();
    let extended_options = session_sy::md::Options::extended();

    // 按来源文件统计失败数，方便判断下一块该补哪个语法
    let mut default_by_file: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut extended_by_file: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut samples: Vec<(usize, String, String, String)> = Vec::new();

    for case in &cases {
        let index = case.get("example").and_then(Value::as_u64).unwrap_or(0) as usize;
        let name = case.get("section").and_then(Value::as_str).unwrap_or("");
        let file = case.get("file").and_then(Value::as_str).unwrap_or("?").to_string();
        let markdown = case.get("markdown").and_then(Value::as_str).unwrap_or("");

        for (key, options, bucket) in [
            ("default", &default_options, &mut default_by_file),
            ("extended", &extended_options, &mut extended_by_file),
        ] {
            let Some(expected_text) = case.get(key).and_then(Value::as_str) else {
                continue;
            };
            let Ok(expected) = serde_json::from_str::<Value>(expected_text) else {
                continue;
            };
            let entry = bucket.entry(file.clone()).or_insert((0usize, 0usize));
            entry.1 += 1;
            let actual = session_sy::md::parse_with(markdown, options);
            if actual == expected {
                entry.0 += 1;
            } else if key == "default" && samples.len() < 60 {
                samples.push((index, name.to_string(), file.clone(), markdown.to_string()));
            }
        }
    }

    let total = cases.len();
    let pass = |bucket: &BTreeMap<String, (usize, usize)>| {
        bucket.values().map(|(ok, _)| *ok).sum::<usize>()
    };
    println!("用例总数 {total}");
    println!("default:  {}/{}", pass(&default_by_file), total);
    println!("extended: {}/{}", pass(&extended_by_file), total);

    let mut ranked: Vec<(usize, String, usize, usize)> = default_by_file
        .iter()
        .map(|(file, (ok, all))| (all - ok, file.clone(), *ok, *all))
        .filter(|(bad, _, _, _)| *bad > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    println!("\n=== default 失败按来源文件（前 25）===");
    for (bad, file, ok, all) in ranked.iter().take(25) {
        println!("  {bad:4} 失败  {file}  {ok}/{all}");
    }

    println!("\n=== default 失败样例（前 60）===");
    for (index, name, file, markdown) in &samples {
        let preview: String = markdown.chars().take(80).collect();
        println!("#{index} [{file}] {name} {preview:?}");
    }
}
