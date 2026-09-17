//! trae-signin-core — 纯逻辑 crate（无 Tauri 依赖，可单测）
//!
//! 模块：
//! - [`auth`]      凭证文件解析（嵌套+扁平）与原子写回
//! - [`upstream`]  上游 API 客户端（换 token / 用户信息 / 签到 / 积分）
//! - [`login`]     登录链接生成与回调解析
//! - [`scheduler`] 调度时间计算（每日定点、含跨天）
//! - [`history`]   签到历史 JSONL 读写
//! - [`settings`]  settings.json 读写

pub mod auth;
pub mod history;
pub mod login;
pub mod scheduler;
pub mod settings;
pub mod upstream;

/// 签到结果四态（对齐上游 CLI 判定顺序）
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum CheckinStatus {
    /// 本次签到成功
    Ok,
    /// 今日已签到（status 或错误信息判定）
    Already,
    /// 账号签到功能被禁用
    Disabled,
    /// 失败
    Failed,
    /// 未知（未查询到状态）
    #[default]
    Unknown,
}

impl CheckinStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckinStatus::Ok => "ok",
            CheckinStatus::Already => "already",
            CheckinStatus::Disabled => "disabled",
            CheckinStatus::Failed => "failed",
            CheckinStatus::Unknown => "unknown",
        }
    }

    pub fn parse_status(s: &str) -> Self {
        match s {
            "ok" => CheckinStatus::Ok,
            "already" => CheckinStatus::Already,
            "disabled" => CheckinStatus::Disabled,
            "failed" => CheckinStatus::Failed,
            _ => CheckinStatus::Unknown,
        }
    }
}

/// token 刷新失败的可重试性（复核结论 #8：区分网络错误与 token 失效）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshErrorKind {
    /// 网络/超时等临时错误，可重试
    Retryable,
    /// 401 / refreshToken 无效，需重新登录
    AuthExpired,
}

/// 统一错误类型
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("HTTP 错误: {0}")]
    Http(#[from] reqwest::Error),
    #[error("认证失效（需重新登录）: {0}")]
    AuthExpired(String),
    #[error("可重试错误（网络/超时）: {0}")]
    Retryable(String),
    #[error("{0}")]
    Other(String),
}

impl CoreError {
    /// 把 reqwest 错误分为可重试 / 认证失效两类
    pub fn classify_refresh(err: &reqwest::Error) -> (RefreshErrorKind, CoreError) {
        let status = err.status();
        if matches!(status, Some(s) if s.as_u16() == 401 || s.as_u16() == 403) {
            (RefreshErrorKind::AuthExpired, CoreError::AuthExpired(err.to_string()))
        } else {
            (RefreshErrorKind::Retryable, CoreError::Retryable(err.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkin_status_roundtrip() {
        for s in [
            CheckinStatus::Ok,
            CheckinStatus::Already,
            CheckinStatus::Disabled,
            CheckinStatus::Failed,
            CheckinStatus::Unknown,
        ] {
            assert_eq!(CheckinStatus::parse_status(s.as_str()), s);
        }
    }

    #[test]
    fn checkin_status_serde() {
        assert_eq!(
            serde_json::to_string(&CheckinStatus::Already).unwrap(),
            "\"already\""
        );
        let v: CheckinStatus = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(v, CheckinStatus::Failed);
        // serde 别名与 parse_status 一致
        assert_eq!(CheckinStatus::parse_status("already"), CheckinStatus::Already);
    }
}
