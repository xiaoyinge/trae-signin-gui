//! 极简文件日志后端：写 `{数据目录}/app.log`。
//!
//! 为什么必须有它：此前全 workspace 没有任何 `log` 后端，所有 `log::info!/warn!/error!`
//! 静默丢弃。上游以 HTTP 200 + 业务码拒绝签到时，用户只能看到最终文案，
//! 原始响应体永远拿不到——`code` 的真实语义至今无法核实即为此因。
//! 这里只解决「看得见」，不引第三方 logger。
//!
//! 取舍：
//! - 每条记录开一次文件追加写。日志量是「每轮几行」级别，换掉句柄状态与失效重开问题。
//! - 单条消息截断，避免整份响应体无限增长；超上限轮转一份 `app.log.1`。
//! - 写失败一律静默：日志后端里再打日志会递归。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const FILE_NAME: &str = "app.log";
/// 超过此大小轮转为 app.log.1（覆盖上一份）
const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 单条消息落盘的最大字符数
const MAX_MSG_CHARS: usize = 600;

/// 当前日志目录；None = 还无处可写
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 安装后端。`dir` 为初始数据目录；未初始化时调用方给 exe 同级目录兜底。
pub fn init(dir: Option<PathBuf>) {
    if let Ok(mut g) = DIR.lock() {
        *g = dir;
    }
    if log::set_logger(&FileLogger).is_err() {
        return; // 重复安装：保留第一个后端
    }
    log::set_max_level(log::LevelFilter::Info);
}

/// 换数据目录后让日志跟着走
pub fn set_dir(dir: PathBuf) {
    if let Ok(mut g) = DIR.lock() {
        *g = Some(dir);
    }
}

struct FileLogger;

impl log::Log for FileLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let Some(dir) = DIR.lock().ok().and_then(|g| g.clone()) else {
            return;
        };
        let msg: String = record.args().to_string().chars().take(MAX_MSG_CHARS).collect();
        let line = format!(
            "{} {:>5} {}: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            record.target(),
            msg
        );
        let path = dir.join(FILE_NAME);
        let _ = rotate_if_huge(&path);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

fn rotate_if_huge(path: &std::path::Path) -> std::io::Result<()> {
    if path.metadata().map(|m| m.len()).unwrap_or(0) < MAX_BYTES {
        return Ok(());
    }
    let bak = path.with_extension("log.1");
    if bak.exists() {
        let _ = fs::remove_file(&bak);
    }
    fs::rename(path, bak)
}
