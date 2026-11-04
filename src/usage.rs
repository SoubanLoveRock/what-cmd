//! 使用记录：读取与聚合供查询时计算使用加分，[`append`] 供 `whatcmd record` 追加。
//!
//! 文件每行只含 `key` 与 `at`，不含查询文本。
//! 无法解析的行记 `warn` 后跳过，不因单行损坏而让查询失败。

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::warn;

const SECONDS_PER_DAY: f32 = 86_400.0;

/// `usage.jsonl` 的一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// 被选中的节点 key。
    pub key: String,
    /// RFC 3339 UTC 时间，形如 `2026-11-03T10:00:00Z`。
    pub at: String,
}

/// 单个 key 的聚合：选中次数与最近一次的时间（Unix 秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    count: u32,
    last_unix: i64,
}

/// 内存中的聚合视图，供查询读取。
#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    entries: HashMap<String, Entry>,
}

impl UsageSnapshot {
    /// 空快照，没有任何使用记录。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 返回 `(选中次数, 最近一次选中距 now_unix 的天数)`。从未选中时返回 `None`。
    /// 时间早于记录时按 0 天计。
    pub fn stat(&self, key: &str, now_unix: i64) -> Option<(u32, f32)> {
        let entry = self.entries.get(key)?;
        let age_secs = (now_unix - entry.last_unix).max(0);
        Some((entry.count, age_secs as f32 / SECONDS_PER_DAY))
    }

    fn record(&mut self, key: &str, at_unix: i64) {
        let entry = self.entries.entry(key.to_owned()).or_insert(Entry {
            count: 0,
            last_unix: at_unix,
        });
        entry.count += 1;
        entry.last_unix = entry.last_unix.max(at_unix);
    }
}

/// 读取 `usage.jsonl` 为快照。文件不存在返回空快照。
pub fn load_snapshot(path: &Path) -> Result<UsageSnapshot, UsageError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(UsageSnapshot::empty()),
        Err(err) => return Err(err.into()),
    };
    let mut snapshot = UsageSnapshot::empty();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<UsageRecord>(line) else {
            warn!(line = line_no + 1, "usage line is not valid JSON, skipped");
            continue;
        };
        let Some(at) = parse_rfc3339_utc(&record.at) else {
            warn!(line = line_no + 1, "usage timestamp invalid, skipped");
            continue;
        };
        snapshot.record(&record.key, at);
    }
    Ok(snapshot)
}

/// key 的长度上限（字节）。
const MAX_KEY_BYTES: usize = 256;

/// 追加一条记录，只写 key 与 `now`（RFC 3339 UTC）。父目录不存在时创建。
///
/// key 为空、过长或含控制字符时拒绝，不写文件。整行一次写入，`O_APPEND` 保证并发追加不交错。
pub fn append(path: &Path, key: &str, now: &str) -> Result<(), UsageError> {
    if key.trim().is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
        return Err(UsageError::BadKey);
    }
    if parse_rfc3339_utc(now).is_none() {
        return Err(UsageError::BadTime);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let record = UsageRecord {
        key: key.to_owned(),
        at: now.to_owned(),
    };
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// 解析严格的 `YYYY-MM-DDTHH:MM:SSZ`，返回 Unix 秒。格式不符返回 `None`。
///
/// 只支持这一种写法，不引入时间库。
pub fn parse_rfc3339_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let shape_ok = b.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z';
    if !shape_ok {
        return None;
    }
    let year = field(s, 0..4)?;
    let month = field(s, 5..7)?;
    let day = field(s, 8..10)?;
    let hour = field(s, 11..13)?;
    let minute = field(s, 14..16)?;
    let second = field(s, 17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// 取出 `range` 内的纯数字字段。
fn field(s: &str, range: Range<usize>) -> Option<i64> {
    let text = s.get(range)?;
    if !text.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// 公历日期到 1970-01-01 的天数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 使用记录错误。
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    /// 读取文件失败（不含文件不存在）。
    #[error("usage io: {0}")]
    Io(#[from] io::Error),
    /// 序列化记录失败。
    #[error("usage json: {0}")]
    Json(#[from] serde_json::Error),
    /// key 为空、过长或含控制字符。
    #[error("usage key is empty, too long, or contains control characters")]
    BadKey,
    /// 时间不是 `YYYY-MM-DDTHH:MM:SSZ`。
    #[error("usage timestamp is not RFC 3339 UTC")]
    BadTime,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        UsageError, UsageSnapshot, append, days_from_civil, load_snapshot, parse_rfc3339_utc,
    };
    use crate::testutil::tempdir;

    #[test]
    fn append_creates_dir_and_round_trips() {
        let dir = tempdir();
        let path = dir.path().join("cache").join("usage.jsonl");
        append(&path, "git pull", "2026-11-01T00:00:00Z").unwrap();
        append(&path, "git pull", "2026-11-02T00:00:00Z").unwrap();
        append(&path, "ls", "2026-11-02T00:00:00Z").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert_eq!(
            text.lines().next().unwrap(),
            "{\"key\":\"git pull\",\"at\":\"2026-11-01T00:00:00Z\"}"
        );
        let snap = load_snapshot(&path).unwrap();
        let now = parse_rfc3339_utc("2026-11-03T00:00:00Z").unwrap();
        assert_eq!(snap.stat("git pull", now).unwrap().0, 2);
        assert_eq!(snap.stat("ls", now).unwrap().0, 1);
    }

    #[test]
    fn append_rejects_bad_key_and_time() {
        let dir = tempdir();
        let path = dir.path().join("usage.jsonl");
        let now = "2026-11-01T00:00:00Z";
        assert!(matches!(append(&path, "", now), Err(UsageError::BadKey)));
        assert!(matches!(append(&path, "  ", now), Err(UsageError::BadKey)));
        assert!(matches!(
            append(&path, "a\nb", now),
            Err(UsageError::BadKey)
        ));
        let long = "x".repeat(257);
        assert!(matches!(append(&path, &long, now), Err(UsageError::BadKey)));
        assert!(matches!(
            append(&path, "ls", "now"),
            Err(UsageError::BadTime)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn parses_strict_utc_timestamps() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_utc("2000-03-01T00:00:00Z"),
            Some(11_017 * 86_400)
        );
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(parse_rfc3339_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_utc("2026-11-03 10:00:00Z"), None);
        assert_eq!(parse_rfc3339_utc("2026-11-03T10:00:00+08:00"), None);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempdir();
        let snap = load_snapshot(&dir.path().join("usage.jsonl")).unwrap();
        assert!(snap.stat("git pull", 0).is_none());
    }

    #[test]
    fn counts_valid_lines_and_skips_bad_ones() {
        let dir = tempdir();
        let path = dir.path().join("usage.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"key\":\"git pull\",\"at\":\"2026-11-01T00:00:00Z\"}\n",
                "not json\n",
                "{\"key\":\"git pull\",\"at\":\"2026-11-02T00:00:00Z\"}\n",
                "{\"key\":\"ls\",\"at\":\"bad\"}\n",
            ),
        )
        .unwrap();
        let snap = load_snapshot(&path).unwrap();
        let now = parse_rfc3339_utc("2026-11-03T00:00:00Z").unwrap();
        let (count, age) = snap.stat("git pull", now).unwrap();
        assert_eq!(count, 2);
        assert!((age - 1.0).abs() < 1e-6);
        assert!(snap.stat("ls", now).is_none());
    }

    #[test]
    fn empty_snapshot_has_no_stats() {
        assert!(UsageSnapshot::empty().stat("git", 0).is_none());
    }
}
