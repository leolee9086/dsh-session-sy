//! 块 id 生成。
//!
//! 这里只负责 id 派生（14 位时间戳 + 7 位 base36）。节点的具体构造分给
//! md（解析）和 render（装配）各自负责，不在这里放通用帮手——两处各写一份
//! 比维护一份"谁都能用"的构造器更不容易漂移。

/// 单调递增的块 id 生成器：14 位时间戳 + 7 位 base36。
pub struct IdGen {
    created_at: i64,
    serial: u64,
}

impl IdGen {
    /// 以会话创建时间作为 id 前缀。
    pub fn new(created_at: i64) -> Self {
        IdGen { created_at, serial: 0 }
    }

    /// 取下一个 id。
    pub fn next(&mut self) -> String {
        let id = block_id(self.created_at, self.serial);
        self.serial += 1;
        id
    }
}

/// 由时间戳和序号派生思源块 id（形如 20260911163216-0000000）。
pub fn block_id(created_at: i64, serial: u64) -> String {
    let seconds = if created_at >= 0 { created_at / 1000 } else { 0 };
    let days = seconds / 86_400;
    let rem = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let stamp = format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}",
        year,
        month,
        day,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    );
    format!("{stamp}-{}", base36(serial))
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buffer = [b'0'; 7];
    let mut index = 7usize;
    loop {
        index -= 1;
        buffer[index] = DIGITS[(value % 36) as usize];
        value /= 36;
        if value == 0 || index == 0 {
            break;
        }
    }
    String::from_utf8(buffer.to_vec()).unwrap_or_else(|_| "0000000".to_owned())
}

/// 从 1970-01-01 起的天数还原成公历年月日。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
