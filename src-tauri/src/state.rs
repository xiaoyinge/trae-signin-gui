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
    pub credits: Option<i64>,
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

    /// 汇总今日状态：(已签数, 总数)
    pub fn today_summary(&self) -> (usize, usize) {
        let map = self.today.lock().unwrap();
        let total = map.len();
        let signed = map
            .values()
            .filter(|s| {
                matches!(
                    s.status,
                    CheckinStatus::Ok | CheckinStatus::Already | CheckinStatus::Disabled
                )
            })
            .count();
        (signed, total)
    }
}

/// 应用是否以 --hidden 启动（自启静默进托盘）
pub fn launched_hidden() -> bool {
    std::env::args().any(|a| a == "--hidden" || a == "-hidden")
}

/// 从历史恢复今日缓存（启动时调用：避免重启后丢失"今日已签"判断）
pub fn restore_today_from_history(state: &AppState) {
    let Some(dir) = state.current_data_dir() else {
        return;
    };
    let Ok(entries) = trae_signin_core::history::read_recent(&dir, 500) else {
        return;
    };
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .single()
        .map(|t| t.timestamp())
        .unwrap_or(0);
    // 今日记录按时间升序重放，最后一条生效
    let mut latest: HashMap<String, &trae_signin_core::history::HistoryEntry> = HashMap::new();
    for e in entries.iter().filter(|e| e.ts >= today_start) {
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
