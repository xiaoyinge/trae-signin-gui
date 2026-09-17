//! settings.json 读写（存数据目录，原子写）
//!
//! 字段缺省时取默认值（宽容解析）。数据目录本身的定位见 PLAN 第 5 节：
//! exe 同级 `data-dir.txt` 指针（src-tauri 侧实现）。

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// 数据目录（冗余记录，便于展示与校验）
    pub data_dir: String,
    /// 每日定点签到时刻 "HH:MM"
    pub daily_time: String,
    /// 周期检查开关
    pub periodic_check_enabled: bool,
    /// 周期检查间隔（分钟）
    pub periodic_check_minutes: u32,
    /// 每日 token 保活开关
    pub keep_alive_daily: bool,
    /// Windows 系统通知
    pub notify_system: bool,
    /// Bark 推送 URL（空 = 关闭）
    pub bark_url: String,
    /// 关闭窗口隐藏到托盘（false = 退出）
    pub close_to_tray: bool,
    /// 开机自启
    pub autostart: bool,
    /// 主题：dark | light | system
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            data_dir: String::new(),
            daily_time: "08:00".into(),
            periodic_check_enabled: true,
            periodic_check_minutes: 30,
            keep_alive_daily: true,
            notify_system: true,
            bark_url: String::new(),
            close_to_tray: true,
            autostart: false,
            theme: "dark".into(),
        }
    }
}

impl Settings {
    /// 校验并归一：非法 daily_time 回退 08:00；非法 theme 回退 dark；间隔下限 1
    pub fn sanitized(mut self) -> Self {
        if crate::scheduler::parse_daily_time(&self.daily_time).is_none() {
            self.daily_time = "08:00".into();
        }
        if !matches!(self.theme.as_str(), "dark" | "light" | "system") {
            self.theme = "dark".into();
        }
        if self.periodic_check_minutes == 0 {
            self.periodic_check_minutes = 30;
        }
        self
    }
}

/// 加载 settings.json（不存在或损坏 → 默认值）
pub fn load_settings(data_dir: &Path) -> Result<Settings, crate::CoreError> {
    let path = data_dir.join(SETTINGS_FILE);
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = std::fs::read_to_string(&path)?;
    match serde_json::from_str::<Settings>(&text) {
        Ok(s) => Ok(s.sanitized()),
        Err(_) => Ok(Settings::default()),
    }
}

/// 保存 settings.json（原子写）
pub fn save_settings(data_dir: &Path, s: &Settings) -> Result<(), crate::CoreError> {
    std::fs::create_dir_all(data_dir)?;
    crate::auth::atomic_write_json(&data_dir.join(SETTINGS_FILE), &s.clone().sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_plan() {
        let s = Settings::default();
        assert_eq!(s.daily_time, "08:00");
        assert!(s.periodic_check_enabled);
        assert_eq!(s.periodic_check_minutes, 30);
        assert!(s.keep_alive_daily);
        assert!(s.notify_system);
        assert!(s.bark_url.is_empty());
        assert!(s.close_to_tray);
        assert!(!s.autostart);
        assert_eq!(s.theme, "dark");
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("tss-{}", uuid::Uuid::new_v4()));
        let s = Settings {
            daily_time: "09:30".into(),
            bark_url: "https://api.day.app/key".into(),
            autostart: true,
            theme: "light".into(),
            ..Default::default()
        };
        save_settings(&dir, &s).unwrap();
        let loaded = load_settings(&dir).unwrap();
        assert_eq!(loaded, s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_gives_default() {
        let dir = std::env::temp_dir().join(format!("tss-{}", uuid::Uuid::new_v4()));
        assert_eq!(load_settings(&dir).unwrap(), Settings::default());
    }

    #[test]
    fn corrupted_file_gives_default() {
        let dir = std::env::temp_dir().join(format!("tss-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), "{corrupted").unwrap();
        assert_eq!(load_settings(&dir).unwrap(), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_fields_filled() {
        let dir = std::env::temp_dir().join(format!("tss-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"daily_time":"07:15"}"#).unwrap();
        let s = load_settings(&dir).unwrap();
        assert_eq!(s.daily_time, "07:15");
        assert_eq!(s.periodic_check_minutes, 30); // 缺省补全
        assert_eq!(s.theme, "dark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_invalid_values() {
        let s = Settings {
            daily_time: "25:99".into(),
            theme: "blue".into(),
            periodic_check_minutes: 0,
            ..Default::default()
        };
        let s = s.sanitized();
        assert_eq!(s.daily_time, "08:00");
        assert_eq!(s.theme, "dark");
        assert_eq!(s.periodic_check_minutes, 30);
    }
}
