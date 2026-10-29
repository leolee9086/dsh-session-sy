//! CommonMark 等价测试：同一段 markdown，Rust 产出的 AST 必须与 Lute 的 RenderJSON 完全相等。
//!
//! fixtures 由 tools/sycheck/mdprobe 用 Lute 自己的 RenderJSON 生成，
//! 判据是外部实现，不是自己跟自己对账。

use serde_json::Value;

#[test]
fn commonmark_conformance() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/commonmark-fixtures.json");
    let text = std::fs::read_to_string(path).expect("读取 fixtures 失败");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("fixtures 不是 JSON 数组");
    assert!(!cases.is_empty(), "fixtures 是空的");

    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let markdown = case.get("markdown").and_then(Value::as_str).unwrap_or("");
        let expected: Value = case
            .get("tree")
            .and_then(Value::as_str)
            .and_then(|tree| serde_json::from_str(tree).ok())
            .unwrap_or(Value::Null);
        let actual = session_sy::md::parse(markdown);
        if actual == expected {
            continue;
        }
        let example = case.get("example").and_then(Value::as_u64).unwrap_or(0);
        let section = case.get("section").and_then(Value::as_str).unwrap_or("");
        failures.push(format!("#{example} [{section}] {markdown:?}"));
    }

    assert!(
        failures.is_empty(),
        "{} / {} 条不匹配：\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
