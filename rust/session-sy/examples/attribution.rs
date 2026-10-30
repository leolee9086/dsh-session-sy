//! 失败归因：对每条不通过的用例，统计「Lute 树里有、我们树里没有」的节点类型。
//!
//! 用法：attribution <fixtures.json> [config]
//!
//! 输出按缺失量排序的节点类型清单——照着清单去读对应的 parse/*.go 文件，
//! 而不是靠肉眼猜哪条语法没实现。

use serde_json::Value;
use std::collections::HashMap;

fn collect(value: &Value, out: &mut HashMap<String, usize>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ty)) = map.get("Type") {
                *out.entry(ty.clone()).or_insert(0) += 1;
            }
            for child in map.values() {
                collect(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect(child, out);
            }
        }
        _ => {}
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: attribution <fixtures.json> [commonmark|default|siyuan]");
        return;
    }
    let path = &args[1];
    let config = args.get(2).map(String::as_str).unwrap_or("default");
    let options = match config {
        "siyuan" => session_sy::md::Options::siyuan(),
        "commonmark" => session_sy::md::Options::commonmark(),
        _ => session_sy::md::Options::default(),
    };

    let text = std::fs::read_to_string(path).expect("读不到 fixtures");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("fixtures 不是数组");

    let mut missing: HashMap<String, usize> = HashMap::new();
    let mut extra: HashMap<String, usize> = HashMap::new();
    let mut failed = 0usize;
    let mut failed_with_structure = 0usize;

    for case in &cases {
        let markdown = case.get("markdown").and_then(Value::as_str).unwrap_or("");
        let expected: Value = case
            .get("tree")
            .and_then(Value::as_str)
            .and_then(|tree| serde_json::from_str(tree).ok())
            .unwrap_or(Value::Null);
        let actual = session_sy::md::parse_with(markdown, &options);
        if actual == expected {
            continue;
        }
        failed += 1;

        let mut exp_types: HashMap<String, usize> = HashMap::new();
        let mut act_types: HashMap<String, usize> = HashMap::new();
        collect(&expected, &mut exp_types);
        collect(&actual, &mut act_types);
        if exp_types != act_types {
            failed_with_structure += 1;
        }

        for (ty, count) in &exp_types {
            let have = act_types.get(ty).unwrap_or(&0);
            if count > have {
                *missing.entry(ty.clone()).or_insert(0) += count - have;
            }
        }
        for (ty, count) in &act_types {
            let want = exp_types.get(ty).unwrap_or(&0);
            if count > want {
                *extra.entry(ty.clone()).or_insert(0) += count - want;
            }
        }
    }

    println!("配置 {config}: 失败 {failed}/{} 条，其中节点类型集合不同的 {failed_with_structure} 条", cases.len());

    let mut missing_list: Vec<(String, usize)> = missing.into_iter().collect();
    missing_list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\n== Lute 有、我们缺的节点（按缺失数排序）==");
    for (ty, count) in missing_list.iter().take(30) {
        println!("{count:>7}  {ty}");
    }

    let mut extra_list: Vec<(String, usize)> = extra.into_iter().collect();
    extra_list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\n== 我们多出来、Lute 没有的节点 ==");
    for (ty, count) in extra_list.iter().take(15) {
        println!("{count:>7}  {ty}");
    }
}
