//! session-sy：把 DSH 会话日志转成思源 `.sy` 文档，并能转回来。
//!
//! 这是唯一实现。判据只有两个外部系统：
//!   1. 思源自己的解析器（tools/sycheck，调 dataparser.ParseJSON）能读 `.sy`；
//!   2. 转回来的会话与 DSH 写出的原始 JSONL 逐字段深等。
//!
//! 用法：
//!   session-sy render <session.jsonl.zstd> <out.sy> [--spin] [--assets DIR]
//!   session-sy restore <doc.sy> [<session.jsonl.zstd>] [--out <path>]
//!   session-sy conformance <fixtures.json> [section]
//!   session-sy parse <markdown>

use std::fs;
use std::process::ExitCode;

use serde_json::Value;

use session_sy::{md, render, session};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("conformance") => cmd_conformance(&args[1..]),
        Some("parse") => cmd_parse(&args[1..]),
        Some("render") => cmd_render(&args[1..]),
        Some("restore") => cmd_restore(&args[1..]),
        _ => {
            usage();
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!("用法:");
    eprintln!("  session-sy render <session.jsonl.zstd> <out.sy> [--spin] [--assets DIR]");
    eprintln!("  session-sy restore <doc.sy> [<session.jsonl.zstd>] [--out <path>]");
    eprintln!("  session-sy conformance <fixtures.json> [section]");
    eprintln!("  session-sy parse <markdown>");
}

/// 找第一处差异，便于定位；相同返回空串。
fn first_difference(left: &Value, right: &Value, path: &str) -> String {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys() {
                if !b.contains_key(key) {
                    return format!("{path}.{key}: 左侧有、右侧无");
                }
            }
            for key in b.keys() {
                if !a.contains_key(key) {
                    return format!("{path}.{key}: 右侧有、左侧无");
                }
            }
            for (key, value) in a {
                let found = first_difference(value, &b[key], &format!("{path}.{key}"));
                if !found.is_empty() {
                    return found;
                }
            }
            String::new()
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return format!("{path}: 长度不同 {} vs {}", a.len(), b.len());
            }
            for (index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                let found = first_difference(x, y, &format!("{path}[{index}]"));
                if !found.is_empty() {
                    return found;
                }
            }
            String::new()
        }
        _ => {
            if left == right {
                String::new()
            } else {
                format!("{path}: {} vs {}", shorten(left), shorten(right))
            }
        }
    }
}

/// 报差异时截断长值，免得日志被一整段正文撑爆。
fn shorten(value: &Value) -> String {
    let text = value.to_string();
    if text.len() <= 120 {
        return text;
    }
    let head: String = text.chars().take(60).collect();
    format!("{head}…（共 {} 字节）", text.len())
}




/// 从 spin 版 .sy 还原事件；给了原始会话就逐条比对。
fn cmd_restore(args: &[String]) -> Result<String, String> {
    let doc_path = args.first().ok_or("restore 需要 .sy 文件")?;
    let text = fs::read_to_string(doc_path).map_err(|error| error.to_string())?;
    let document: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let (header, events) = render::restore(&document)?;

    // --out <path>：把事件写成 DSH 认的 jsonl（每行一个事件）
    if let Some(position) = args.iter().position(|item| item == "--out") {
        let out_path = args.get(position + 1).ok_or("--out 需要路径")?;
        let mut text = String::new();
        // 第一行是会话头，和 DSH 的日志布局一致
        text.push_str(&serde_json::to_string(&header).map_err(|error| error.to_string())?);
        text.push('\n');
        for event in &events {
            text.push_str(&serde_json::to_string(event).map_err(|error| error.to_string())?);
            text.push('\n');
        }
        fs::write(out_path, text).map_err(|error| error.to_string())?;
        return Ok(format!("写出 {out_path}（{} 个事件）", events.len()));
    }

    let Some(session_path) = args.get(1) else {
        return Ok(format!("还原出 {} 个事件", events.len()));
    };
    let session = session::read(session_path)?;
    if session.header != header {
        return Err("会话头不一致".into());
    }
    if session.events.len() != events.len() {
        return Err(format!(
            "事件数不一致：原始 {}，还原 {}",
            session.events.len(),
            events.len()
        ));
    }
    for (index, (left, right)) in session.events.iter().zip(events.iter()).enumerate() {
        if left != right {
            return Err(format!("第 {index} 个事件不一致"));
        }
    }
    Ok(format!("{} 个事件逐条一致", events.len()))
}

/// 把会话渲染成 .sy：默认 clean，加 --spin 则携带原文。
fn cmd_render(args: &[String]) -> Result<String, String> {
    let session_path = args.first().ok_or("render 需要会话文件")?;
    let out_path = args.get(1).ok_or("render 需要输出路径")?;
    let profile = if args.iter().any(|item| item == "--spin") {
        render::Profile::Spin
    } else {
        render::Profile::Clean
    };
    // --assets <dir>：思源的 data/assets 目录，图片会复制到这里
    let assets = args
        .iter()
        .position(|item| item == "--assets")
        .and_then(|position| args.get(position + 1))
        .map(std::path::PathBuf::from);
    let session = session::read(session_path)?;
    let document = render::document(&session, profile, assets.as_deref());
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    fs::write(out_path, text).map_err(|error| error.to_string())?;
    Ok(format!(
        "写出 {out_path}（{} 个块）",
        document
            .get("Children")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0)
    ))
}

/// 从命令行里取 `--config` 指定的引擎配置，缺省是 commonmark。
fn parse_config(args: &[String]) -> Result<md::Options, String> {
    if let Some(position) = args.iter().position(|item| item == "--config") {
        if let Some(name) = args.get(position + 1) {
            return match name.as_str() {
                "siyuan" => Ok(md::Options::siyuan()),
                "extended" => Ok(md::Options::extended()),
                "default" => Ok(md::Options::default()),
                "commonmark" => Ok(md::Options::commonmark()),
                other => Err(format!("未知配置: {other}")),
            };
        }
    }
    Ok(md::Options::commonmark())
}

/// 把一段 markdown 解析成 Lute 风格的 AST，用于对照参考实现。
///
/// `--config commonmark|default|siyuan` 选择引擎配置，默认 commonmark。
fn cmd_parse(args: &[String]) -> Result<String, String> {
    let text = args.first().ok_or("parse 需要一段 markdown")?;
    let options = parse_config(args)?;
    let mut source = text.clone();
    if !source.ends_with('\n') {
        source.push('\n');
    }
    let tree = md::parse_with(&source, &options);
    serde_json::to_string_pretty(&tree).map_err(|error| error.to_string())
}

/// 跑 CommonMark 规范的等价测试：同一段 markdown，Rust 的 AST 必须等于 Lute 的 AST。
fn cmd_conformance(args: &[String]) -> Result<String, String> {
    let path = args.first().ok_or("conformance 需要 fixture 路径")?;
    let options = parse_config(args)?;
    let only_section = args
        .get(1)
        .filter(|item| !item.starts_with("--"))
        .map(String::as_str);
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let cases: Vec<Value> = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut pass = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        let markdown = case.get("markdown").and_then(Value::as_str).unwrap_or("");
        let expected: Value = case
            .get("tree")
            .and_then(Value::as_str)
            .and_then(|tree| serde_json::from_str(tree).ok())
            .unwrap_or(Value::Null);
        let actual = md::parse_with(markdown, &options);
        if actual == expected {
            pass += 1;
            continue;
        }
        let section = case.get("section").and_then(Value::as_str).unwrap_or("");
        if only_section.is_some_and(|wanted| wanted != section) {
            continue;
        }
        if failures.len() < 8 {
            failures.push(format!(
                "#{} {} markdown={:?}\n  差异 {}",
                case.get("example").and_then(Value::as_u64).unwrap_or(0),
                section,
                markdown,
                first_difference(&expected, &actual, ""),
            ));
        }
    }
    // 按章节统计，方便看缺口集中在哪些规则上。
    let mut sections: std::collections::BTreeMap<String, (usize, usize)> = std::collections::BTreeMap::new();
    for case in &cases {
        let markdown = case.get("markdown").and_then(Value::as_str).unwrap_or("");
        let expected: Value = case
            .get("tree")
            .and_then(Value::as_str)
            .and_then(|tree| serde_json::from_str(tree).ok())
            .unwrap_or(Value::Null);
        let actual = md::parse_with(markdown, &options);
        let section = case.get("section").and_then(Value::as_str).unwrap_or("?").to_owned();
        let entry = sections.entry(section).or_insert((0usize, 0usize));
        entry.1 += 1;
        if actual == expected {
            entry.0 += 1;
        }
    }
    let mut summary: Vec<(usize, String)> = Vec::new();
    for (section, (ok, total)) in &sections {
        if ok < total {
            summary.push((total - ok, format!("{section}: {ok}/{total}")));
        }
    }
    summary.sort_by(|a, b| b.0.cmp(&a.0));
    let mut report = format!("通过 {pass}/{}", cases.len());
    for (_, line) in summary.iter().take(18) {
        report.push_str("\n  ");
        report.push_str(line);
    }
    for failure in &failures {
        report.push('\n');
        report.push_str(failure);
    }
    Ok(report)
}

