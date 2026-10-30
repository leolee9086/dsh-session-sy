//! 从会话日志里提取真实 markdown 文本，作为解析器的一致性语料。
//!
//! 用法：extract_corpus <输出.json> <会话日志>...
//!
//! 只收 text 块（会话里真正当成 markdown 渲染的部分），过滤掉过短和过长的，
//! 免得语料被一两段巨型输出或一堆空片段淹没。

use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: extract_corpus <输出.json> <会话日志>...");
        return;
    }
    let out_path = &args[1];
    let mut corpus: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for path in &args[2..] {
        let session = match session_sy::session::read(path) {
            Ok(session) => session,
            Err(error) => {
                eprintln!("跳过 {path}: {error}");
                continue;
            }
        };
        let session_id = session.header.get("id").and_then(Value::as_str).unwrap_or("?");
        let mut collected = 0usize;
        for event in &session.events {
            let Some(data) = event.get("data") else { continue };
            let seq = event.get("seq").and_then(Value::as_i64).unwrap_or(-1);
            for (index, block) in content_blocks(data).into_iter().enumerate() {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                let Some(text) = block.get("text").and_then(Value::as_str) else { continue };
                if text.len() < 8 || text.len() > 8192 {
                    continue;
                }
                if !seen.insert(text.to_owned()) {
                    continue;
                }
                corpus.push(json!({
                    "markdown": text,
                    "source": format!("{session_id}#{seq}#{index}"),
                }));
                collected += 1;
            }
        }
        eprintln!("{path}: 收集 {collected} 条");
    }

    let text = serde_json::to_string(&corpus).expect("语料序列化失败");
    std::fs::write(out_path, text).expect("写出语料失败");
    println!("共 {} 条语料 -> {out_path}", corpus.len());
}

/// 与会话渲染层保持一致的 content 取法：优先 data.content，其次 data.message.content。
fn content_blocks(data: &Value) -> Vec<Value> {
    if let Some(Value::Array(items)) = data.get("content") {
        return items.clone();
    }
    if let Some(Value::Array(items)) = data
        .get("message")
        .and_then(|message| message.get("content"))
    {
        return items.clone();
    }
    Vec::new()
}
