//! 应用状态：数据目录指针（exe 同级 data-dir.txt）、设置缓存、今日签到缓存

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use tauri::Manager;
use trae_signin_core::settings::{load_settings, Settings};
use trae_signin_core::CheckinStatus;

pub const POINTER_FILE: &str = "data-dir.txt";

/// exe 同级目录（便携指针所在）
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

/// 读数据目录指针；缺失/空 → None（视为首次启动）
pub fn read_pointer() -> Option<PathBuf> {
    let dir = exe_dir()?;
    let text = std::fs::read_to_string(dir.join(POINTER_FILE)).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// 写数据目录指针
pub fn write_pointer(path: &std::path::Path) -> Result<(), String> {
    let dir = exe_dir().ok_or("无法定位 exe 目录")?;
    std::fs::write(dir.join(POINTER_FILE), path.to_string_lossy().as_bytes())
        .map_err(|e| format!("写入指针失败: {e}"))
}

/// 账号今日运行态（内存缓存；签到/刷新后更新）
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TodayState {
    pub status: CheckinStatus,
    pub credits: Option<f64>,
    /// token 到期（秒）
    pub expires_at: Option<i64>,
    /// 401 / refreshToken 失效 → 需重新登录
    pub need_relogin: bool,
    /// 网络/超时类刷新失败（可重试）
    pub refresh_failed: bool,
    /// 最近一次操作时间（秒）
    pub last_run_ts: Option<i64>,
}

/// 每日定点轮的上游限流重试计划。
/// 存在即表示「今天的定点轮还没拿到终局结果」，与 `sched_last_daily` 分离，
/// 免得把「无进展」当成「今日已完成」，也免得每 30s tick 打一次上游。
#[derive(Debug, Clone, Copy)]
pub struct DailyRetry {
    pub date: chrono::NaiveDate,
    /// 已经延后过几次
    pub attempts: usize,
    pub next: std::time::Instant,
}

/// 共享应用状态
pub struct AppState {
    pub settings: StdMutex<Settings>,
    /// 是否已完成首次设置
    pub needs_setup: StdMutex<bool>,
    /// 当前数据目录
    pub data_dir: StdMutex<Option<PathBuf>>,
    /// uid → 今日状态缓存
    pub today: StdMutex<HashMap<String, TodayState>>,
    /// 签到全局互斥（串行执行轮）
    pub signin_lock: tokio::sync::Mutex<()>,
    /// 进行中的登录会话（abort 句柄 + 端口）
    pub login: StdMutex<Option<crate::login_service::LoginSession>>,
    /// 调度：上次执行日期记录
    pub sched_last_daily: StdMutex<Option<chrono::NaiveDate>>,
    pub sched_last_keepalive: StdMutex<Option<chrono::NaiveDate>>,
    pub sched_last_periodic: StdMutex<Option<std::time::Instant>>,
    /// 调度：每日定点轮的延后重试计划
    pub sched_daily_retry: StdMutex<Option<DailyRetry>>,
    /// 启动补签是否已执行
    pub startup_done: StdMutex<bool>,
    /// 今日缓存对应的日期（跨天清空 `today` 的依据，见 `rollover_today_cache`）
    pub cache_date: StdMutex<Option<chrono::NaiveDate>>,
}

impl AppState {
    pub fn new() -> Self {
        let pointer = read_pointer();
        let settings = pointer
            .as_ref()
            .and_then(|dir| load_settings(dir).ok())
            .unwrap_or_default();
        let needs_setup = pointer.is_none();
        Self {
            settings: StdMutex::new(settings),
            needs_setup: StdMutex::new(needs_setup),
            data_dir: StdMutex::new(pointer),
            today: StdMutex::new(HashMap::new()),
            signin_lock: tokio::sync::Mutex::new(()),
            login: StdMutex::new(None),
            sched_last_daily: StdMutex::new(None),
            sched_last_keepalive: StdMutex::new(None),
            sched_last_periodic: StdMutex::new(None),
            sched_daily_retry: StdMutex::new(None),
            startup_done: StdMutex::new(false),
            cache_date: StdMutex::new(None),
        }
    }

    pub fn current_settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn current_data_dir(&self) -> Option<PathBuf> {
        self.data_dir.lock().unwrap().clone()
    }

    /// 从磁盘重载设置
    pub fn reload_settings(&self) -> Settings {
        let s = self
            .current_data_dir()
            .and_then(|dir| load_settings(&dir).ok())
            .unwrap_or_default();
        *self.settings.lock().unwrap() = s.clone();
        s
    }

    /// 保存设置到磁盘 + 更新缓存
    pub fn store_settings(&self, s: Settings) -> Result<(), String> {
        let dir = self
            .current_data_dir()
            .ok_or("数据目录未初始化")?;
        trae_signin_core::settings::save_settings(&dir, &s).map_err(|e| e.to_string())?;
        *self.settings.lock().unwrap() = s;
        Ok(())
    }

    /// 更新某账号今日缓存
    pub fn update_today(&self, uid: &str, st: TodayState) {
        self.today.lock().unwrap().insert(uid.to_string(), st);
    }

    /// 汇总今日状态：(已签数, 总数)。
    /// 总数 = 全部账号**剔除已禁用**（PLAN §10 #8：disabled 本就不该被签，
    /// 计入"已签"会与账号卡 ⛔ 语义冲突；不剔除则新导入账号漏出分母 → 不亮红点）。
    /// 未出现在今日缓存里的账号视为未签（新导入未处理 → 亮红点）。
    pub fn today_summary(&self) -> (usize, usize) {
        let Some(dir) = self.current_data_dir() else {
            return (0, 0);
        };
        let creds = trae_signin_core::auth::list_credentials(&dir).unwrap_or_default();
        if creds.is_empty() {
            return (0, 0);
        }
        let map = self.today.lock().unwrap();
        let mut total = 0usize;
        let mut signed = 0usize;
        for c in &creds {
            match map.get(&c.uid).map(|t| t.status) {
                Some(CheckinStatus::Disabled) => {}
                Some(CheckinStatus::Ok | CheckinStatus::Already) => {
                    total += 1;
                    signed += 1;
                }
                _ => total += 1,
            }
        }
        (signed, total)
    }
}

/// 应用是否以 --hidden 启动（自启静默进托盘）
pub fn launched_hidden() -> bool {
    std::env::args().any(|a| a == "--hidden" || a == "-hidden")
}

/// 跨天滚动：昨日残留的今日缓存（Ok/Already/Disabled）若不清空，跨天后所有
/// skip_signed 轮（定点/周期/手动全部签到）都会把昨天当成今天而跳过——
/// 应用常驻跨天是主场景，等于自动签到静默失效一整天，托盘还亮绿点。
/// 启动首轮（cache_date=None）只记日期不清缓存：setup 时 `restore_today_from_history`
/// 已按 is_today 过滤，缓存里本来就只有今日记录。
/// **调用方必须已持有 signin_lock**（与轮内 update_today 串行）。
pub fn rollover_today_cache(state: &AppState) {
    let today = chrono::Local::now().date_naive();
    let mut d = state.cache_date.lock().unwrap();
    if *d == Some(today) {
        return;
    }
    if d.is_some() {
        state.today.lock().unwrap().clear();
    }
    *d = Some(today);
}

/// 从历史恢复今日缓存（启动时调用：避免重启后丢失"今日已签"判断）
pub fn restore_today_from_history(state: &AppState) {
    let Some(dir) = state.current_data_dir() else {
        return;
    };
    let Ok(entries) = trae_signin_core::history::read_recent(&dir, 500) else {
        return;
    };
    // 今日记录按时间升序重放，最后一条生效。
    // read_recent 返回时间**降序**（最新在前），必须反转后重放：
    // 否则最早一条（如失败）会覆盖最新一条（如成功），重启后状态回退。
    // 用 core 的 is_today 过滤（M9/L6 修复：旧实现构造"今日零点"时间戳，
    // 本地午夜不存在的时区下 `.single()` 为 None → today_start=0 → 回放整个历史文件当"今日"）。
    let now_local = chrono::Local::now();
    let mut latest: HashMap<String, &trae_signin_core::history::HistoryEntry> = HashMap::new();
    for e in entries
        .iter()
        .rev()
        .filter(|e| trae_signin_core::scheduler::is_today(e.ts, now_local))
    {
        latest.insert(e.uid.clone(), e);
    }
    for (uid, e) in latest {
        state.update_today(
            &uid,
            TodayState {
                status: e.status,
                credits: e.credits,
                expires_at: None,
                need_relogin: false,
                refresh_failed: false,
                last_run_ts: Some(e.ts),
            },
        );
    }
}

/// 便捷：获取 app state
pub fn state(app: &tauri::AppHandle) -> &AppState {
    app.state::<AppState>().inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use trae_signin_core::history::{append_entry, HistoryEntry};

    fn test_state_with_dir(dir: &std::path::Path) -> AppState {
        let st = AppState::new();
        *st.data_dir.lock().unwrap() = Some(dir.to_path_buf());
        st
    }

    fn write_history(dir: &std::path::Path, ts: i64, status: CheckinStatus) {
        append_entry(
            dir,
            &HistoryEntry {
                ts,
                uid: "u-1".into(),
                nickname: "测试".into(),
                status,
                credits: None,
                message: "m".into(),
            },
        )
        .unwrap();
    }

    /// 同一账号今日有多条记录（先失败后成功）时，必须恢复**最新**一条。
    /// read_recent 返回时间降序，重放必须反转；旧实现降序遍历直接 insert，
    /// 最早的 Failed 覆盖了最新的 Ok，重启后托盘误亮红点、定时轮重复 claim。
    #[test]
    fn restore_replays_latest_entry_per_uid() {
        let dir = std::env::temp_dir().join(format!("tstate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = test_state_with_dir(&dir);
        let now = chrono::Local::now().timestamp();
        write_history(&dir, now - 100, CheckinStatus::Failed);
        write_history(&dir, now - 10, CheckinStatus::Ok);
        restore_today_from_history(&st);
        assert_eq!(
            st.today.lock().unwrap().get("u-1").map(|t| t.status),
            Some(CheckinStatus::Ok),
            "恢复的必须是最新一条（Ok），而不是最早的 Failed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 跨天滚动：昨日残留的今日缓存必须清空（自动签到被昨日状态跳过一整天的根源）。
    #[test]
    fn rollover_clears_cache_on_date_change() {
        let st = AppState::new();
        let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
        // 模拟昨日状态：cache_date 指向昨天 + 缓存里有"已签"
        *st.cache_date.lock().unwrap() = Some(yesterday);
        st.update_today(
            "u-1",
            TodayState {
                status: CheckinStatus::Ok,
                ..Default::default()
            },
        );
        super::rollover_today_cache(&st);
        assert!(st.today.lock().unwrap().get("u-1").is_none(), "跨天后昨日缓存必须清空");
        assert_eq!(
            *st.cache_date.lock().unwrap(),
            Some(chrono::Local::now().date_naive())
        );
    }

    /// 同一天内重复滚动不得清缓存（否则周期检查刚写的结果会被下一轮抹掉）。
    #[test]
    fn rollover_is_noop_within_same_day() {
        let st = AppState::new();
        st.update_today(
            "u-1",
            TodayState {
                status: CheckinStatus::Ok,
                ..Default::default()
            },
        );
        super::rollover_today_cache(&st);
        super::rollover_today_cache(&st);
        assert_eq!(
            st.today.lock().unwrap().get("u-1").map(|t| t.status),
            Some(CheckinStatus::Ok),
            "同一天内滚动是无操作"
        );
    }
}
