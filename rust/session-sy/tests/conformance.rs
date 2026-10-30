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

/// 直出路径必须和 Value 路径产出同一棵树。
///
/// `parse_to_json` 直接拼 JSON 文本，不经过 serde 的序列化器，
/// 所以字符串转义（引号、反斜杠、控制符、非 ASCII）全靠自己的实现。
/// 这里把它的输出解析回 Value 再比对，覆盖到转义的每一条规则。
#[test]
fn direct_json_matches_tree() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/commonmark-fixtures.json");
    let text = std::fs::read_to_string(path).expect("读取 fixtures 失败");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("fixtures 不是 JSON 数组");

    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let markdown = case.get("markdown").and_then(Value::as_str).unwrap_or("");
        let tree = session_sy::md::parse(markdown);
        let direct = session_sy::md::parse_to_json(markdown);
        let reparsed: Value = match serde_json::from_str(&direct) {
            Ok(value) => value,
            Err(error) => {
                let example = case.get("example").and_then(Value::as_u64).unwrap_or(0);
                failures.push(format!("#{example} 直出不是合法 JSON: {error}\n  {direct}"));
                continue;
            }
        };
        if reparsed != tree {
            let example = case.get("example").and_then(Value::as_u64).unwrap_or(0);
            failures.push(format!(
                "#{example} 直出与树不一致\n  直出: {direct}\n  树: {tree}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} / {} 条直出不一致：\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

