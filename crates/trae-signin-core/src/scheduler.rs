//! 调度时间计算（每日定点，含跨天；周期检查间隔）
//!
//! 调度本体（tokio task 循环）在 src-tauri 侧；本模块只做纯时间计算，便于单测。

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, Timelike};

/// 解析 "HH:MM" 为 NaiveTime；非法输入返回 None
pub fn parse_daily_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    NaiveTime::from_hms_opt(h, m, 0)
}

/// 计算下一次每日定点时刻：
/// - 今天该时刻未到 → 今天
/// - 已过 → 明天（跨天）
pub fn next_daily_run(daily: NaiveTime, now: DateTime<Local>) -> DateTime<Local> {
    let today = now
        .date_naive()
        .and_time(daily)
        .and_local_timezone(*now.offset())
        .single()
        .map(|t| t.with_timezone(&Local));
    match today {
        Some(t) if t > now => t,
        _ => (now.date_naive() + Duration::days(1))
            .and_time(daily)
            .and_local_timezone(*now.offset())
            .single()
            .map(|t| t.with_timezone(&Local))
            .unwrap_or_else(|| now + Duration::days(1)),
    }
}

/// 下一次周期检查时刻：now + interval（秒）
pub fn next_periodic_run(interval_minutes: u32, now: DateTime<Local>) -> DateTime<Local> {
    now + Duration::minutes(i64::from(interval_minutes.max(1)))
}

/// 判断给定时刻是否"今天"（本地时区）
pub fn is_today(ts: i64, now: DateTime<Local>) -> bool {
    let dt = DateTime::from_timestamp(ts, 0)
        .map(|u| u.with_timezone(&Local));
    match dt {
        Some(d) => d.year() == now.year() && d.ordinal() == now.ordinal(),
        None => false,
    }
}

/// 格式化 HH:MM
pub fn format_hhmm(t: NaiveTime) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parse_daily_time_cases() {
        assert_eq!(parse_daily_time("08:00"), NaiveTime::from_hms_opt(8, 0, 0));
        assert_eq!(parse_daily_time("23:59"), NaiveTime::from_hms_opt(23, 59, 0));
        assert_eq!(parse_daily_time("0:5"), NaiveTime::from_hms_opt(0, 5, 0));
        assert_eq!(parse_daily_time("24:00"), None);
        assert_eq!(parse_daily_time("08:60"), None);
        assert_eq!(parse_daily_time("abc"), None);
        assert_eq!(parse_daily_time(""), None);
    }

    #[test]
    fn next_daily_run_same_day() {
        let now = Local.with_ymd_and_hms(2026, 9, 14, 7, 0, 0).unwrap();
        let daily = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let next = next_daily_run(daily, now);
        assert_eq!(next.hour(), 8);
        assert_eq!(next.day(), 14);
    }

    #[test]
    fn next_daily_run_cross_day() {
        let now = Local.with_ymd_and_hms(2026, 9, 14, 8, 0, 1).unwrap();
        let daily = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let next = next_daily_run(daily, now);
        assert_eq!(next.day(), 15); // 已过 1 秒 → 明天
    }

    #[test]
    fn next_daily_run_cross_month() {
        let now = Local.with_ymd_and_hms(2026, 9, 30, 9, 0, 0).unwrap();
        let daily = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let next = next_daily_run(daily, now);
        assert_eq!((next.month(), next.day()), (10, 1));
    }

    #[test]
    fn next_daily_run_cross_year() {
        let now = Local.with_ymd_and_hms(2026, 12, 31, 23, 59, 0).unwrap();
        let daily = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        let next = next_daily_run(daily, now);
        assert_eq!((next.year(), next.month(), next.day()), (2027, 1, 1));
    }

    #[test]
    fn next_daily_run_exact_now_returns_tomorrow() {
        let now = Local.with_ymd_and_hms(2026, 9, 14, 8, 0, 0).unwrap();
        let daily = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let next = next_daily_run(daily, now);
        assert_eq!(next.day(), 15); // 恰好等于 now 视为已过
    }

    #[test]
    fn format_hhmm_pads() {
        assert_eq!(format_hhmm(NaiveTime::from_hms_opt(8, 5, 0).unwrap()), "08:05");
    }

    #[test]
    fn periodic_interval_min_1() {
        let now = Local.with_ymd_and_hms(2026, 9, 14, 8, 0, 0).unwrap();
        let next = next_periodic_run(0, now);
        assert_eq!(next, now + Duration::minutes(1));
    }
}
