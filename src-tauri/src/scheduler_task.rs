//! 应用内调度循环：启动补签 + 每日定点 + 周期检查 + 每日保活
//!
//! 每 30 秒 tick 一次，重读设置；全局互斥通过 signin_lock（try_lock 顺延）。

use crate::commands::{run_refresh_round_inner, run_signin_round, SigninSummary};
use crate::state::{state, AppState, DailyRetry};
use std::sync::Mutex as StdMutex;
use tauri::{AppHandle, Emitter};
use trae_signin_core::scheduler::parse_daily_time;

const TICK_SECS: u64 = 30;
const STARTUP_CATCHUP_DELAY_SECS: u64 = 15;
/// 整轮无进展（上游限流/网络）时的延后重试阶梯（秒）。
/// 用尽后放弃今日定点轮，交给「周期检查」兜底——既不会把一天烧在第一次拒绝上，
/// 也不会因为不记日期守卫而退化成每 30s 打一次上游。
const DAILY_RETRY_LADDER_SECS: &[u64] = &[300, 900, 1800, 3600];

/// setup 时启动调度
pub fn spawn_scheduler(app: AppHandle) {
    // 1) 启动补签：15 秒后执行一轮"未签则签"
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(STARTUP_CATCHUP_DELAY_SECS)).await;
        let st = state(&app2);
        if *st.startup_done.lock().unwrap() {
            return;
        }
        *st.startup_done.lock().unwrap() = true;
        log::info!("启动补签开始");
        let _ = run_signin_round(&app2, None, false).await;
    });

    // 2) 常驻调度循环
    let app3 = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = schedule_tick(&app3).await {
                log::error!("调度 tick 失败: {e}");
            }
        }
    });
}

/// 本轮是否真正尝试执行（仅"被锁顺延"算未尝试；Err 也算尝试过，按正常节奏重试）
fn round_attempted(r: &Result<SigninSummary, String>) -> bool {
    !matches!(r, Ok(s) if s.skipped)
}

fn date_done(slot: &StdMutex<Option<chrono::NaiveDate>>, today: chrono::NaiveDate) -> bool {
    *slot.lock().unwrap() == Some(today)
}

fn mark_date(slot: &StdMutex<Option<chrono::NaiveDate>>, today: chrono::NaiveDate) {
    *slot.lock().unwrap() = Some(today);
}

/// 今日定点轮是否可执行：无待重试计划、或计划时刻已到。
/// **不清空计划**——`bump_daily_retry` 要靠它记住已经延后到第几档；
/// 在这里清空会让阶梯每次都退回首档（2026-09-16 app.log 实测：连续三次都是「300s 后重试」）。
fn daily_retry_ready(st: &AppState, today: chrono::NaiveDate) -> bool {
    let mut g = st.sched_daily_retry.lock().unwrap();
    match *g {
        Some(r) if r.date == today => std::time::Instant::now() >= r.next,
        // 跨天：昨天的计划与今天的阶梯无关
        Some(_) => {
            *g = None;
            true
        }
        None => true,
    }
}

/// 今日定点轮结束（拿到终局结果，或重试阶梯用尽）
fn finish_daily(st: &AppState, today: chrono::NaiveDate) {
    *st.sched_daily_retry.lock().unwrap() = None;
    mark_date(&st.sched_last_daily, today);
}

/// 本轮无进展：延后到阶梯的下一档；档位用尽则放弃今日定点轮
fn bump_daily_retry(st: &AppState, today: chrono::NaiveDate, blocked: usize, total: usize) {
    let pending: Option<DailyRetry> = *st.sched_daily_retry.lock().unwrap();
    let used = pending.filter(|r| r.date == today).map(|r| r.attempts).unwrap_or(0);
    match DAILY_RETRY_LADDER_SECS.get(used) {
        Some(secs) => {
            *st.sched_daily_retry.lock().unwrap() = Some(DailyRetry {
                date: today,
                attempts: used + 1,
                next: std::time::Instant::now() + std::time::Duration::from_secs(*secs),
            });
            log::warn!(
                "每日定点轮 {blocked}/{total} 账号无进展（上游瞬时失败），{secs}s 后重试"
            );
        }
        None => {
            finish_daily(st, today);
            log::warn!("每日定点重试阶梯用尽，今日交由周期检查兜底");
        }
    }
}

async fn schedule_tick(app: &AppHandle) -> Result<(), String> {
    let st = state(app);
    let settings = st.current_settings();
    let now = chrono::Local::now();
    let today = now.date_naive();

    let daily_due = parse_daily_time(&settings.daily_time)
        .is_some_and(|daily| now.time() >= daily);

    // 每日定点签到：真正跑过才记日期守卫，被锁顺延则留给下一次 tick（30s 后）重试
    if daily_due
        && !date_done(&st.sched_last_daily, today)
        && daily_retry_ready(st, today)
    {
        log::info!("每日定点签到触发（{}）", settings.daily_time);
        let r = run_signin_round(app, None, false).await;
        if round_attempted(&r) {
            match &r {
                // 上游把整轮挡在门外：今日槽位不算用完，按阶梯延后重试
                Ok(s) if s.retryable_failed > 0 => {
                    bump_daily_retry(st, today, s.retryable_failed, s.total)
                }
                _ => finish_daily(st, today),
            }
        }
        if let Err(e) = r {
            log::warn!("每日定点签到轮失败: {e}");
        }
    }

    // 每日保活（定点时刻之后执行一次，无条件刷新全部 token）
    if daily_due
        && settings.keep_alive_daily
        && !date_done(&st.sched_last_keepalive, today)
        && refresh_keepalive(app).await
    {
        mark_date(&st.sched_last_keepalive, today);
    }

    // 周期检查（未签则签）
    if settings.periodic_check_enabled {
        let interval = std::time::Duration::from_secs(
            u64::from(settings.periodic_check_minutes.max(1)) * 60,
        );
        let due = match *st.sched_last_periodic.lock().unwrap() {
            Some(t) => t.elapsed() >= interval,
            None => true,
        };
        if due {
            log::info!("周期检查触发（{} 分钟）", settings.periodic_check_minutes);
            let r = run_signin_round(app, None, false).await;
            // 被锁顺延时不重置计时，下一次 tick 继续尝试
            if round_attempted(&r) {
                *st.sched_last_periodic.lock().unwrap() = Some(std::time::Instant::now());
            }
            if let Err(e) = r {
                log::warn!("周期检查轮失败: {e}");
            }
        }
    }
    Ok(())
}

/// 每日保活：无条件刷新全部账号 token；失败区分类型写入历史。
/// 返回 false 表示因签到轮占用互斥而未执行（调用方不应记为今日已完成）。
async fn refresh_keepalive(app: &AppHandle) -> bool {
    let st = state(app);
    // 保活会轮换并回写 refreshToken，必须与签到轮互斥
    let Ok(_guard) = st.signin_lock.try_lock() else {
        log::info!("签到轮进行中，保活顺延");
        return false;
    };
    let Some(dir) = st.current_data_dir() else {
        return true; // 未完成首次设置，无事可做（记为已完成，避免每 tick 重试）
    };
    let Ok(mut creds) = trae_signin_core::auth::list_credentials(&dir) else { return true };
    let upstream = trae_signin_core::upstream::Upstream::new();
    for cred in creds.iter_mut() {
        let uid = cred.uid.clone();
        let nickname = if cred.nickname.is_empty() { uid.clone() } else { cred.nickname.clone() };
        // 强制刷新（无视 2h 缓冲）：临时把 expires_at 拉到过去
        cred.expires_at = 0;
        let now_ts = chrono::Utc::now().timestamp();
        let (status, message, need_relogin, refresh_failed) =
            match upstream.ensure_fresh_token(cred, &dir).await {
                Ok(()) => (
                    trae_signin_core::CheckinStatus::Unknown,
                    "保活刷新成功".into(),
                    false,
                    false,
                ),
                Err(trae_signin_core::CoreError::AuthExpired(m)) => (
                    trae_signin_core::CheckinStatus::Failed,
                    format!("需重新登录: {m}"),
                    true,
                    false,
                ),
                Err(e) => (
                    trae_signin_core::CheckinStatus::Unknown,
                    format!("刷新失败（可重试）: {e}"),
                    false,
                    true,
                ),
            };
        // 更新缓存
        {
            let mut t = st.today.lock().unwrap().get(&uid).cloned().unwrap_or_default();
            t.expires_at = Some(cred.expires_at);
            t.need_relogin = need_relogin;
            t.refresh_failed = refresh_failed;
            t.last_run_ts = Some(now_ts);
            st.update_today(&uid, t);
        }
        // 失败写历史（成功不写，避免噪音）
        if need_relogin || refresh_failed {
            let entry = trae_signin_core::history::HistoryEntry {
                ts: now_ts,
                uid: uid.clone(),
                nickname,
                status,
                credits: None,
                message,
            };
            let _ = trae_signin_core::history::append_entry(&dir, &entry);
        }
        crate::tray::update_tray(app);
        let _ = app.emit("accounts://changed", ());
    }
    // 完成后做一次刷新视图（已持锁，走不再取锁的内部版本）
    let _ = run_refresh_round_inner(app, None).await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> chrono::NaiveDate {
        chrono::Local::now().date_naive()
    }

    /// 到点不得销毁计划：`bump_daily_retry` 靠它记档。
    /// 2026-09-16 app.log 实测旧实现连续三次都写「300s 后重试」——阶梯被清空即退回首档。
    #[test]
    fn daily_retry_ready_keeps_ladder_position_when_due() {
        let st = AppState::new();
        let d = today();
        *st.sched_daily_retry.lock().unwrap() = Some(DailyRetry {
            date: d,
            attempts: 2,
            next: std::time::Instant::now() - std::time::Duration::from_secs(1),
        });
        assert!(daily_retry_ready(&st, d));
        assert_eq!(
            st.sched_daily_retry.lock().unwrap().map(|r| r.attempts),
            Some(2),
            "计划时刻已到只能放行，不能清空阶梯位置"
        );
        bump_daily_retry(&st, d, 1, 1);
        assert_eq!(st.sched_daily_retry.lock().unwrap().map(|r| r.attempts), Some(3));
    }

    /// 未到点不放行；阶梯逐档推进；用尽后放弃今日定点轮并记日期守卫。
    #[test]
    fn daily_retry_ladder_advances_then_closes_day() {
        let st = AppState::new();
        let d = today();
        assert!(daily_retry_ready(&st, d), "无计划时应放行（首次定点轮）");

        for i in 0..DAILY_RETRY_LADDER_SECS.len() {
            bump_daily_retry(&st, d, 1, 1);
            let pending = *st.sched_daily_retry.lock().unwrap();
            let r = pending.expect("阶梯未用尽前应保留计划");
            assert_eq!(r.attempts, i + 1);
            assert!(!daily_retry_ready(&st, d), "延后时刻未到，不得放行");
        }
        // 档位用尽：再 bump 一次即放弃今日
        bump_daily_retry(&st, d, 1, 1);
        assert!(st.sched_daily_retry.lock().unwrap().is_none());
        assert_eq!(*st.sched_last_daily.lock().unwrap(), Some(d));
    }

    /// 昨天的计划不能挡住今天的第一次定点轮
    #[test]
    fn stale_daily_retry_from_previous_date_is_dropped() {
        let st = AppState::new();
        let d = today();
        *st.sched_daily_retry.lock().unwrap() = Some(DailyRetry {
            date: d - chrono::Duration::days(1),
            attempts: 3,
            next: std::time::Instant::now() + std::time::Duration::from_secs(3600),
        });
        assert!(daily_retry_ready(&st, d));
        assert!(st.sched_daily_retry.lock().unwrap().is_none());
    }
}
