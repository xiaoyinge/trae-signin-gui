//! 签到历史 JSONL 读写
//!
//! 文件：`{data_dir}/history.jsonl`，每行一条 JSON：
//! `{ ts, uid, nickname, status, credits, message }`
//! 只追加；读取时取最近 N 条（时间倒序）。

use crate::CheckinStatus;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    /// 秒级 Unix 时间戳
    pub ts: i64,
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
    pub status: CheckinStatus,
    #[serde(default)]
    pub credits: Option<i64>,
    #[serde(default)]
    pub message: String,
}

/// history.jsonl 路径
pub fn history_path(data_dir: &Path) -> PathBuf {
    data_dir.join("history.jsonl")
}

/// 追加一条记录（原子性：单行 write + '\n'）
pub fn append_entry(data_dir: &Path, entry: &HistoryEntry) -> Result<(), crate::CoreError> {
    std::fs::create_dir_all(data_dir)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path(data_dir))?;
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    Ok(())
}

/// 读取最近 limit 条（按 ts 降序返回）。解析失败的行跳过。
pub fn read_recent(data_dir: &Path, limit: usize) -> Result<Vec<HistoryEntry>, crate::CoreError> {
    let path = history_path(data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = File::open(&path)?;
    let reader = BufReader::new(f);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(e) = serde_json::from_str::<HistoryEntry>(trimmed) {
            entries.push(e);
        }
    }
    // 稳定排序：按 ts 降序
    entries.sort_by_key(|e| std::cmp::Reverse(e.ts));
    entries.truncate(limit);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: i64, uid: &str, status: CheckinStatus, credits: Option<i64>) -> HistoryEntry {
        HistoryEntry {
            ts,
            uid: uid.into(),
            nickname: format!("名{uid}"),
            status,
            credits,
            message: "测试".into(),
        }
    }

    #[test]
    fn append_read_recent_desc() {
        let dir = std::env::temp_dir().join(format!("tsh-{}", uuid::Uuid::new_v4()));
        append_entry(&dir, &entry(100, "u1", CheckinStatus::Ok, Some(10))).unwrap();
        append_entry(&dir, &entry(300, "u2", CheckinStatus::Already, None)).unwrap();
        append_entry(&dir, &entry(200, "u3", CheckinStatus::Failed, Some(5))).unwrap();
        append_entry(&dir, &entry(400, "u4", CheckinStatus::Disabled, None)).unwrap();

        let all = read_recent(&dir, 200).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].ts, 400);
        assert_eq!(all[3].ts, 100);

        let recent2 = read_recent(&dir, 2).unwrap();
        assert_eq!(recent2.len(), 2);
        assert_eq!(recent2[0].ts, 400);
        assert_eq!(recent2[1].ts, 300);
        assert_eq!(recent2[1].status, CheckinStatus::Already);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_broken_lines() {
        let dir = std::env::temp_dir().join(format!("tsh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = File::create(history_path(&dir)).unwrap();
        writeln!(f, "{{\"broken\":1").unwrap(); // 非法 JSON 行（故意截断）
        writeln!(f, "{}", serde_json::to_string(&entry(1, "u", CheckinStatus::Ok, None)).unwrap()).unwrap();
        writeln!(f).unwrap();
        drop(f);
        let v = read_recent(&dir, 10).unwrap();
        assert_eq!(v.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_when_missing() {
        let dir = std::env::temp_dir().join(format!("tsh-{}", uuid::Uuid::new_v4()));
        assert!(read_recent(&dir, 10).unwrap().is_empty());
    }

    #[test]
    fn serde_field_names() {
        let e = entry(1786858238, "u1", CheckinStatus::Ok, Some(5100));
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"ts\":1786858238"));
        assert!(s.contains("\"uid\":\"u1\""));
        assert!(s.contains("\"status\":\"ok\""));
        assert!(s.contains("\"credits\":5100"));
    }
}
