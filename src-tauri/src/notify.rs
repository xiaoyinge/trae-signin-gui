//! 通知：Windows 系统通知 + Bark 推送

use trae_signin_core::login::urlencode;
use tauri::AppHandle;
use trae_signin_core::settings::Settings;

/// 发送 Windows 系统通知（开关关闭时静默跳过）
pub fn system_notify(app: &AppHandle, settings: &Settings, title: &str, body: &str) {
    if !settings.notify_system {
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

/// Bark 推送；url 形如 https://api.day.app/{key}；返回 Err(信息)
pub async fn bark_push(base_url: &str, title: &str, body: &str) -> Result<(), String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Bark URL 为空".into());
    }
    let url = format!("{base}/{}", urlencode(title));
    let url = if body.is_empty() {
        url
    } else {
        format!("{url}/{}", urlencode(body))
    };
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(&url).send().await.map_err(|e| format!("Bark 请求失败: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Bark HTTP {}", resp.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bark_empty_url_errs() {
        assert!(bark_push("", "t", "b").await.is_err());
    }

    #[test]
    fn bark_url_shape() {
        // 仅验证拼接逻辑（urlencode 复用 login 模块）
        let base = "https://api.day.app/abc123/";
        let url = format!(
            "{}/{}",
            base.trim_end_matches('/'),
            urlencode("TRAE 签到测试推送")
        );
        assert_eq!(url, "https://api.day.app/abc123/TRAE%20%E7%AD%BE%E5%88%B0%E6%B5%8B%E8%AF%95%E6%8E%A8%E9%80%81");
    }
}
