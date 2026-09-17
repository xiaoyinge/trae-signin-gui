//! Tauri 命令 + 签到轮/保活轮/补签轮核心流程

use crate::login_service::ImportResult;
use crate::notify::{bark_push, system_notify};
use crate::state::{state, AppState};
use crate::tray::update_tray;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use trae_signin_core::history::HistoryEntry;
use trae_signin_core::settings::Settings;
use trae_signin_core::upstream::Upstream;
use trae_signin_core::CheckinStatus;

// ─────────────────────────── 数据视图 ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub uid: String,
    pub nickname: String,
    pub credits: Option<i64>,
    pub status: CheckinStatus,
    pub expires_at: Option<i64>,
    pub need_relogin: bool,
    pub refresh_failed: bool,
    pub last_run_ts: Option<i64>,
}

fn account_views(state: &AppState) -> Vec<AccountView> {
    let Some(dir) = state.current_data_dir() else {
        return Vec::new();
    };
    let creds = trae_signin_core::auth::list_credentials(&dir).unwrap_or_default();
    creds
        .into_iter()
        .map(|c| {
            let today = state.today.lock().unwrap().get(&c.uid).cloned();
            let t = today.unwrap_or_default();
            AccountView {
                uid: c.uid,
                nickname: c.nickname,
                credits: t.credits,
                status: t.status,
                expires_at: t.expires_at.or(Some(c.expires_at)),
                need_relogin: t.need_relogin,
                refresh_failed: t.refresh_failed,
                last_run_ts: t.last_run_ts,
            }
        })
        .collect()
}

// ─────────────────────────── 首次启动 / 数据目录 ───────────────────────────

#[derive(Debug, Serialize)]
pub struct Bootstrap {
    pub needs_setup: bool,
    pub suggested_dir: String,
    pub data_dir: Option<String>,
}

#[tauri::command]
pub fn bootstrap(app: AppHandle) -> Bootstrap {
    let state = state(&app);
    let needs_setup = *state.needs_setup.lock().unwrap();
    let data_dir = state
        .current_data_dir()
        .map(|p| p.to_string_lossy().into_owned());
    let suggested_dir = crate::state::exe_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    Bootstrap { needs_setup, suggested_dir, data_dir }
}

#[tauri::command]
pub async fn choose_data_dir(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择数据目录")
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });
    rx.await.map_err(|e| format!("对话框失败: {e}"))
}

#[tauri::command]
pub async fn init_data_dir(app: AppHandle, path: String) -> Result<Settings, String> {
    let dir = std::path::PathBuf::from(&path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    // 校验可写
    let probe = dir.join(".write-probe");
    std::fs::write(&probe, b"ok").map_err(|e| format!("目录不可写: {e}"))?;
    let _ = std::fs::remove_file(&probe);

    crate::state::write_pointer(&dir)?;
    let st = state(&app);
    // 目标目录已有 settings.json（重装/换机器后指针丢失，引导时选回原数据目录）：
    // 保留用户既有配置，只补 data_dir；不能拿内存默认值覆盖。
    let mut settings = if dir.join(trae_signin_core::settings::SETTINGS_FILE).exists() {
        trae_signin_core::settings::load_settings(&dir)
            .unwrap_or_else(|_| st.current_settings())
    } else {
        st.current_settings()
    };
    settings.data_dir = path.clone();
    trae_signin_core::settings::save_settings(&dir, &settings).map_err(|e| e.to_string())?;
    crate::logger::set_dir(dir.clone());
    *st.data_dir.lock().unwrap() = Some(dir);
    *st.settings.lock().unwrap() = settings.clone();
    *st.needs_setup.lock().unwrap() = false;
    // 恢复今日缓存（换目录/重装场景）
    crate::state::restore_today_from_history(st);
    update_tray(&app);
    Ok(settings)
}

#[tauri::command]
pub async fn change_data_dir(app: AppHandle, path: String) -> Result<Settings, String> {
    let new_dir = std::path::PathBuf::from(&path);
    std::fs::create_dir_all(&new_dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let st = state(&app);
    // 与签到/刷新/保活轮互斥：轮持有旧目录 PathBuf 会持续往旧目录回写凭证与历史，
    // 边迁边写会让复制出去的快照丢更新。拿不到锁说明有轮进行中，明确拒绝。
    let _signin_guard = match st.signin_lock.try_lock() {
        Ok(g) => g,
        Err(_) => return Err("签到进行中，请稍后再试".into()),
    };
    let old_dir = st
        .current_data_dir()
        .ok_or("当前数据目录未初始化")?;

    // 自动迁移 auths/ 与 history.jsonl（若目标不存在对应文件）
    let migrate = |src: std::path::PathBuf, dst: std::path::PathBuf| -> Result<(), String> {
        if !src.exists() {
            return Ok(());
        }
        if dst.exists() {
            // 目标已有同名数据：保留目标（不覆盖），提示由前端展示
            return Ok(());
        }
        std::fs::copy(&src, &dst).map(|_| ()).map_err(|e| format!("迁移 {src:?} 失败: {e}"))
    };
    // 整个 auths 目录：目录级复制
    let src_auths = trae_signin_core::auth::auths_dir(&old_dir);
    let dst_auths = trae_signin_core::auth::auths_dir(&new_dir);
    if src_auths.exists() && !dst_auths.exists() {
        copy_dir_recursive(&src_auths, &dst_auths)
            .map_err(|e| format!("迁移凭证目录失败: {e}"))?;
    }
    migrate(
        trae_signin_core::history::history_path(&old_dir),
        trae_signin_core::history::history_path(&new_dir),
    )?;
    // settings.json 迁移后重写（含新 data_dir）
    let mut settings = st.current_settings();
    settings.data_dir = path.clone();
    trae_signin_core::settings::save_settings(&new_dir, &settings).map_err(|e| e.to_string())?;
    crate::state::write_pointer(&new_dir)?;
    crate::logger::set_dir(new_dir.clone());
    *st.data_dir.lock().unwrap() = Some(new_dir);
    *st.settings.lock().unwrap() = settings.clone();
    st.today.lock().unwrap().clear();
    crate::state::restore_today_from_history(st);
    update_tray(&app);
    Ok(settings)
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn open_data_dir(app: AppHandle) -> Result<(), String> {
    let dir = state(&app)
        .current_data_dir()
        .ok_or("数据目录未初始化")?;
    tauri_plugin_opener::open_path(dir, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_external(_app: AppHandle, url: String) -> Result<(), String> {
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ─────────────────────────── 设置 ───────────────────────────

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let st = state(&app);
    st.reload_settings();
    Ok(st.current_settings())
}

#[tauri::command]
pub async fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let st = state(&app);
    st.store_settings(settings.sanitized())?;
    sync_autostart(&app, &st.current_settings()).await;
    update_tray(&app);
    let _ = app.emit("settings://changed", st.current_settings());
    Ok(())
}

/// 同步开机自启（autostart 插件，自启带 --hidden 静默进托盘）
async fn sync_autostart(app: &AppHandle, settings: &Settings) {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    let enabled = launcher.is_enabled().unwrap_or(false);
    match (settings.autostart, enabled) {
        (true, false) => {
            let _ = launcher.enable();
        }
        (false, true) => {
            let _ = launcher.disable();
        }
        _ => {}
    }
}

#[tauri::command]
pub fn check_update(_app: AppHandle) -> serde_json::Value {
    serde_json::json!({
        "configured": false,
        "message": "更新源未配置（更新器为骨架占位，暂未接入 GitHub Releases）"
    })
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

// ─────────────────────────── 账号 ───────────────────────────

#[tauri::command]
pub fn list_accounts(app: AppHandle) -> Vec<AccountView> {
    account_views(state(&app))
}

#[tauri::command]
pub fn import_credential(app: AppHandle, json: String) -> Result<ImportResult, String> {
    let st = state(&app);
    let r = crate::login_service::import_credential_text(st, &json)?;
    update_tray(&app);
    let _ = app.emit("accounts://changed", ());
    Ok(r)
}

#[tauri::command]
pub async fn delete_account(app: AppHandle, uid: String) -> Result<bool, String> {
    let st = state(&app);
    // 与签到/刷新/保活轮互斥：凭证回写（token 刷新、deviceId 迁移）只发生在持锁的轮内，
    // 删除也拿同一把锁才能避免"已删的凭证文件被进行中的轮回写复活"。
    let _guard = match st.signin_lock.try_lock() {
        Ok(g) => g,
        Err(_) => return Err("签到进行中，请稍后再试".into()),
    };
    let dir = st.current_data_dir().ok_or("数据目录未初始化")?;
    let deleted = trae_signin_core::auth::delete_credential(&dir, &uid)
        .map_err(|e| e.to_string())?;
    st.today.lock().unwrap().remove(&uid);
    update_tray(&app);
    let _ = app.emit("accounts://changed", ());
    Ok(deleted)
}

// ─────────────────────────── 登录 ───────────────────────────

#[tauri::command]
pub async fn start_login(app: AppHandle) -> Result<u16, String> {
    crate::login_service::start_login(app).await
}

#[tauri::command]
pub fn cancel_login(app: AppHandle) {
    crate::login_service::cancel_login(&app)
}

// ─────────────────────────── 签到 / 刷新 ───────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct SigninProgress {
    pub uid: String,
    pub nickname: String,
    pub stage: String, // waiting | refreshing | checking | claiming | querying | done
    pub status: Option<CheckinStatus>,
    pub message: String,
    pub credits: Option<i64>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SigninSummary {
    pub total: usize,
    pub ok: usize,
    pub already: usize,
    pub disabled: usize,
    pub failed: usize,
    /// 本轮无进展、稍后重试可能成功的账号数（上游限流/网络类）
    pub retryable_failed: usize,
    /// true = 未执行（另一轮占用互斥），调度方应顺延重试而不是记为已完成
    pub skipped: bool,
}

fn empty_summary(skipped: bool) -> SigninSummary {
    SigninSummary {
        total: 0,
        ok: 0,
        already: 0,
        disabled: 0,
        failed: 0,
        retryable_failed: 0,
        skipped,
    }
}

/// 执行一轮签到（串行、实时进度）。
/// uids=None 全部账号；manual=true 拿不到锁报"签到进行中"，false（定时触发）静默跳过。
pub async fn run_signin_round(
    app: &AppHandle,
    uids: Option<Vec<String>>,
    manual: bool,
) -> Result<SigninSummary, String> {
    let st = state(app);
    let _guard = if manual {
        match st.signin_lock.try_lock() {
            Ok(l) => l,
            Err(_) => return Err("签到进行中，请稍后再试".into()),
        }
    } else {
        match st.signin_lock.try_lock() {
            Ok(l) => l,
            Err(_) => {
                log::info!("上一轮签到未结束，本次定时触发顺延");
                return Ok(empty_summary(true));
            }
        }
    };

    let dir = st.current_data_dir().ok_or("数据目录未初始化")?;
    let mut creds = trae_signin_core::auth::list_credentials(&dir)
        .map_err(|e| format!("读取凭证失败: {e}"))?;
    if let Some(ids) = &uids {
        creds.retain(|c| ids.contains(&c.uid));
    }
    let total = creds.len();
    let mut summary = SigninSummary {
        total,
        ok: 0,
        already: 0,
        disabled: 0,
        failed: 0,
        retryable_failed: 0,
        skipped: false,
    };
    if total == 0 {
        return Ok(summary);
    }

    let settings = st.current_settings();
    let upstream = Upstream::new();
    let mut bark_lines: Vec<String> = Vec::new();

    for cred in creds.iter_mut() {
        let uid = cred.uid.clone();
        let nickname = if cred.nickname.is_empty() { uid.clone() } else { cred.nickname.clone() };
        let emit = |stage: &str, message: &str, status: Option<CheckinStatus>, credits: Option<i64>| {
            let _ = app.emit(
                "signin://progress",
                SigninProgress {
                    uid: uid.clone(),
                    nickname: nickname.clone(),
                    stage: stage.into(),
                    status,
                    message: message.into(),
                    credits,
                },
            );
        };

        emit("checking", "查询签到状态…", None, None);
        let result = upstream.signin_account(cred, &dir).await;
        let no_progress = result.retryable || result.refresh_failed;
        let outcome = result.outcome;
        let credits = result.credits;

        // 更新缓存
        let now_ts = chrono::Utc::now().timestamp();
        st.update_today(
            &uid,
            crate::state::TodayState {
                status: outcome.status,
                credits,
                expires_at: Some(cred.expires_at),
                need_relogin: result.need_relogin,
                refresh_failed: result.refresh_failed,
                last_run_ts: Some(now_ts),
            },
        );

        match outcome.status {
            CheckinStatus::Ok => summary.ok += 1,
            CheckinStatus::Already => summary.already += 1,
            CheckinStatus::Disabled => summary.disabled += 1,
            _ => summary.failed += 1,
        }
        if no_progress {
            summary.retryable_failed += 1;
        }

        // 历史
        let entry = HistoryEntry {
            ts: now_ts,
            uid: uid.clone(),
            nickname: nickname.clone(),
            status: outcome.status,
            credits,
            message: outcome.message.clone(),
        };
        if let Err(e) = trae_signin_core::history::append_entry(&dir, &entry) {
            log::error!("写历史失败: {e}");
        }

        let status_text = match outcome.status {
            CheckinStatus::Ok => "✅",
            CheckinStatus::Already => "☑️",
            CheckinStatus::Disabled => "⛔",
            CheckinStatus::Failed => "❌",
            CheckinStatus::Unknown => "❓",
        };
        bark_lines.push(format!(
            "{status_text} {nickname} {}{}",
            outcome.message,
            credits.map(|c| format!(" 积分{c}")).unwrap_or_default()
        ));

        emit("done", &outcome.message, Some(outcome.status), credits);
        update_tray(app);
        let _ = app.emit("accounts://changed", ());
    }

    let _ = app.emit("signin://done", &summary);

    // 通知
    let title = format!(
        "TRAE 签到完成 总计{} 成功{} 已签{} 禁用{} 失败{}",
        total, summary.ok, summary.already, summary.disabled, summary.failed
    );
    let body = bark_lines.join("\n");
    system_notify(app, &settings, &title, &body);
    if !settings.bark_url.trim().is_empty() {
        let url = settings.bark_url.clone();
        let title2 = title.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = bark_push(&url, &title2, &body).await {
                log::warn!("Bark 推送失败: {e}");
            }
        });
    }

    Ok(summary)
}

/// 单账号签到（手动重签）
#[tauri::command]
pub async fn signin_one(app: AppHandle, uid: String) -> Result<SigninSummary, String> {
    run_signin_round(&app, Some(vec![uid]), true).await
}

/// 全部签到（手动）
#[tauri::command]
pub async fn signin_all(app: AppHandle) -> Result<SigninSummary, String> {
    run_signin_round(&app, None, true).await
}

/// 全部刷新：查询状态 + 积分（不签到）。与签到轮共用互斥（刷新会回写 token）。
pub async fn run_refresh_round(
    app: &AppHandle,
    uids: Option<Vec<String>>,
    manual: bool,
) -> Result<(), String> {
    let st = state(app);
    let _guard = if manual {
        match st.signin_lock.try_lock() {
            Ok(l) => l,
            Err(_) => return Err("签到进行中，请稍后再试".into()),
        }
    } else {
        match st.signin_lock.try_lock() {
            Ok(l) => l,
            Err(_) => {
                log::info!("签到轮进行中，刷新顺延");
                return Ok(());
            }
        }
    };
    run_refresh_round_inner(app, uids).await
}

/// 刷新主体。**调用方必须已持有 signin_lock**（tokio Mutex 不可重入）。
pub async fn run_refresh_round_inner(
    app: &AppHandle,
    uids: Option<Vec<String>>,
) -> Result<(), String> {
    let st = state(app);
    let dir = st.current_data_dir().ok_or("数据目录未初始化")?;
    let mut creds = trae_signin_core::auth::list_credentials(&dir)
        .map_err(|e| format!("读取凭证失败: {e}"))?;
    if let Some(ids) = &uids {
        creds.retain(|c| ids.contains(&c.uid));
    }
    let upstream = Upstream::new();
    for cred in creds.iter_mut() {
        let uid = cred.uid.clone();
        let mut t = st.today.lock().unwrap().get(&uid).cloned().unwrap_or_default();
        // 惰性刷新 token（需要时）并回写
        match upstream.ensure_fresh_token(cred, &dir).await {
            Ok(()) => {
                t.need_relogin = false;
                t.refresh_failed = false;
            }
            Err(trae_signin_core::CoreError::AuthExpired(m)) => {
                t.need_relogin = true;
                t.refresh_failed = false;
                log::warn!("{uid} 刷新失败(需重登): {m}");
            }
            Err(e) => {
                t.refresh_failed = true;
                log::warn!("{uid} 刷新失败(可重试): {e}");
            }
        }
        // 查状态 + 积分（失败不阻塞其他账号）
        if let Ok((checked_in, enable)) = upstream.checkin_status(cred).await {
            t.status = if checked_in {
                CheckinStatus::Already
            } else if !enable {
                CheckinStatus::Disabled
            } else {
                CheckinStatus::Unknown
            };
        }
        t.credits = upstream.query_credits(cred).await.ok();
        t.expires_at = Some(cred.expires_at);
        t.last_run_ts = Some(chrono::Utc::now().timestamp());
        st.update_today(&uid, t);
        update_tray(app);
        let _ = app.emit("accounts://changed", ());
    }
    Ok(())
}

#[tauri::command]
pub async fn refresh_all(app: AppHandle) -> Result<(), String> {
    run_refresh_round(&app, None, true).await
}

#[tauri::command]
pub async fn refresh_account(app: AppHandle, uid: String) -> Result<(), String> {
    run_refresh_round(&app, Some(vec![uid]), true).await
}

// ─────────────────────────── 历史 ───────────────────────────

#[tauri::command]
pub fn get_history(app: AppHandle, limit: Option<usize>) -> Result<Vec<HistoryEntry>, String> {
    let st = state(&app);
    let dir = st.current_data_dir().ok_or("数据目录未初始化")?;
    trae_signin_core::history::read_recent(&dir, limit.unwrap_or(200)).map_err(|e| e.to_string())
}

// ─────────────────────────── Bark ───────────────────────────

#[tauri::command]
pub async fn test_bark(url: String) -> Result<(), String> {
    bark_push(&url, "TRAE 签到测试推送", "如果你看到这条消息，说明 Bark 配置正确。").await
}
