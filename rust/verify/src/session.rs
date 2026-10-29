//! 读 DSH 会话日志：多帧 zstd，解压后逐行 JSONL。
//!
//! 第 1 行是头（type: "session"），其余每行一个 SessionEvent。事件保持原样，
//! 拆字段就会漏 —— surfaceOp、sourceEventSeqs、ignorable、格式迁移留下的 seq0/time0
//! 都在事件里，而漏掉是不可见的。

use std::io::Read;

use serde_json::Value;

/// zstd 帧魔数 0xFD2FB528 的小端字节序。
const FRAME_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// 一份会话：头 + 按序事件，全部保留原始 JSON。
pub struct Session {
    pub header: Value,
    pub events: Vec<Value>,
}

impl Session {
    /// 会话 id。
    pub fn id(&self) -> &str {
        self.header.get("id").and_then(Value::as_str).unwrap_or_default()
    }

    /// 头的 createdAt，用于派生稳定的块 id；缺失时用 0。
    pub fn created_at(&self) -> i64 {
        self.header.get("createdAt").and_then(Value::as_i64).unwrap_or(0)
    }
}

/// 读一个会话文件。
pub fn read(path: &str) -> Result<Session, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("读取 {path} 失败: {error}"))?;
    let text = String::from_utf8(decode(&bytes)?).map_err(|error| format!("{path} 不是 UTF-8: {error}"))?;
    parse(&text)
}

/// 解压多帧 zstd。
///
/// DSH 按帧写日志，帧之间首尾相接。逐帧给解码器精确的字节范围最稳：
/// 万一魔数在载荷里偶然出现（约 2^-32 的概率），截断的帧会解码失败，
/// 于是把尾部逐字节退回去，直到这一帧成立。
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut starts = Vec::new();
    if bytes.len() >= 4 {
        for index in 0..=bytes.len() - 4 {
            if bytes[index..index + 4] == FRAME_MAGIC {
                starts.push(index);
            }
        }
    }
    if starts.is_empty() {
        return Err("没有找到 zstd 帧".into());
    }

    let mut out = Vec::new();
    for (position, start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(bytes.len());
        let mut attempt = end;
        let mut decoded: Option<Vec<u8>> = None;
        while attempt > *start {
            let slice = &bytes[*start..attempt];
            match ruzstd::StreamingDecoder::new(slice) {
                Ok(mut decoder) => {
                    let mut buffer = Vec::new();
                    match decoder.read_to_end(&mut buffer) {
                        Ok(_) => {
                            decoded = Some(buffer);
                            break;
                        }
                        Err(_) => attempt -= 1,
                    }
                }
                Err(_) => attempt -= 1,
            }
        }
        let buffer = decoded.ok_or_else(|| format!("第 {position} 帧（字节 {start}）解压失败"))?;
        out.extend_from_slice(&buffer);
    }
    Ok(out)
}

/// 逐行解析 JSONL：第一行是头，其余是事件。
pub fn parse(text: &str) -> Result<Session, String> {
    let mut header: Option<Value> = None;
    let mut events = Vec::new();
    for (number, line) in text.split('\n').enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("第 {} 行不是 JSON: {error}", number + 1))?;
        if header.is_none() {
            header = Some(value);
        } else {
            events.push(value);
        }
    }
    Ok(Session { header: header.ok_or("会话文件为空")?, events })
}
