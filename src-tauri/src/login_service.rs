//! 登录服务：端口探测 → 生成链接 → 本地回调服务 → 换 token → 落盘

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tauri::async_runtime::JoinHandle;
use trae_signin_core::auth::Credential;
use trae_signin_core::login::{
    build_login_url, gen_device_id, gen_machine_id, gen_trace_id, parse_callback_query,
    LoginParams,
};
use trae_signin_core::upstream::Upstream;

pub const PORT_RANGE: std::ops::RangeInclusive<u16> = 18080..=18089;
pub const LOGIN_TIMEOUT_SECS: u64 = 5 * 60;

/// 进行中的登录会话
pub struct LoginSession {
    pub task: JoinHandle<()>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoginProgress {
    pub stage: String, // preparing | waiting | exchanging | saving | done | failed | cancelled
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
}

/// 探测可用端口（18080 起递增，≤18089；全占报错）
pub async fn probe_port() -> Result<u16, String> {
    for port in PORT_RANGE {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => {
                drop(l);
                return Ok(port);
            }
            Err(_) => continue,
        }
    }
    Err(format!(
        "回调端口 {}-{} 均被占用，请释放端口后重试",
        PORT_RANGE.start(),
        PORT_RANGE.end()
    ))
}

const SUCCESS_HTML: &str = r#"<!doctype html><html lang="zh-CN"><meta charset="utf-8">
<title>登录成功</title>
<body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0f172a;color:#e2e8f0">
<div style="text-align:center"><h1>登录成功</h1><p>请返回 TRAE 签到应用</p></div>
</body></html>"#;

const NOT_FOUND_HTML: &str = "404";

/// 启动一次登录会话
pub async fn start_login(app: AppHandle) -> Result<u16, String> {
    let port = probe_port().await?;
    let params = LoginParams {
        port,
        machine_id: gen_machine_id(),
        device_id: gen_device_id(),
        login_trace_id: gen_trace_id(),
    };
    let url = build_login_url(&params);
    stash_ids(params.machine_id.clone(), params.device_id.clone());
    log::info!("登录链接: {url}");

    let app2 = app.clone();
    // 「检查已有会话 → 打浏览器 → spawn → 注册句柄」必须整体在同一把锁内完成：
    // spawn 是同步调用（立即返回 JoinHandle），锁内无 await 点，
    // 并发 start_login（如双击添加账号）无法都通过检查——否则双击会多开浏览器页。
    {
        let state = crate::state::state(&app);
        let mut g = state.login.lock().unwrap();
        if g.is_some() {
            return Err("已有登录会话进行中".into());
        }
        // 打浏览器（失败不致命，用户可手动打开链接——把链接写进日志与事件）
        if let Err(e) = tauri_plugin_opener::open_url(url, None::<&str>) {
            log::warn!("打开浏览器失败: {e}");
        }
        let task = tauri::async_runtime::spawn(async move {
            let result = run_callback_server(&app2, port).await;
            // 清理会话
            let state = crate::state::state(&app2);
            {
                let mut g = state.login.lock().unwrap();
                *g = None;
            }
            match result {
                Ok(uid) => {
                    let _ = app2.emit("login://done", LoginProgress {
                        stage: "done".into(),
                        message: "登录成功".into(),
                        uid: Some(uid.0),
                        nickname: Some(uid.1),
                    });
                }
                Err(e) => {
                    let _ = app2.emit("login://failed", LoginProgress {
                        stage: if e == "已取消" { "cancelled" } else { "failed" }.into(),
                        message: e,
                        uid: None,
                        nickname: None,
                    });
                }
            }
        });
        *g = Some(LoginSession { task });
    }
    let _ = app.emit("login://progress", LoginProgress {
        stage: "waiting".into(),
        message: format!("已打开浏览器，等待授权回调（端口 {port}，5 分钟内有效）…"),
        uid: None,
        nickname: None,
    });
    Ok(port)
}

/// 取消当前登录会话
pub fn cancel_login(app: &AppHandle) {
    let session = {
        let state = crate::state::state(app);
        let mut g = state.login.lock().unwrap();
        g.take()
    };
    if let Some(s) = session {
        s.task.abort();
        let _ = app.emit("login://failed", LoginProgress {
            stage: "cancelled".into(),
            message: "已取消".into(),
            uid: None,
            nickname: None,
        });
    }
}

/// 回调服务主流程：监听 → 等待 /authorize → 换 token → 落盘。成功返回 (uid, nickname)
async fn run_callback_server(
    app: &AppHandle,
    port: u16,
) -> Result<(String, String), String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("端口 {port} 绑定失败: {e}"))?;

    // 等待回调（5 分钟超时）
    let query = tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS)) =>
            return Err("登录超时（5 分钟）".into()),
        r = wait_authorize(&listener) => r?,
    };

    let _ = app.emit("login://progress", LoginProgress {
        stage: "exchanging".into(),
        message: "已捕获回调，正在换取凭证…".into(),
        uid: None,
        nickname: None,
    });

    let data = parse_callback_query(&query).map_err(|e| format!("回调解析失败: {e}"))?;

    let upstream = Upstream::new();
    // 优先 refreshToken → ExchangeToken；无则用 userJwt.Token 兜底
    let (access_token, refresh_token, expires_at) = if !data.refresh_token.is_empty() {
        let t = upstream
            .exchange_token(&data.refresh_token)
            .await
            .map_err(|e| format!("换取 token 失败: {e}"))?;
        (t.access_token, t.refresh_token, t.expires_at)
    } else {
        (
            data.fallback_access_token.clone(),
            String::new(),
            data.expires_at.unwrap_or_else(|| chrono::Utc::now().timestamp() + 7 * 86400),
        )
    };

    let info = upstream.get_user_info(&access_token).await.map_err(|e| {
        format!("获取用户信息失败: {e}")
    })?;
    // userInfo 回调参数兜底
    let uid = if info.uid.is_empty() { data.uid.clone() } else { info.uid };
    let nickname = if !info.nickname.is_empty() { info.nickname } else { data.screen_name.clone() };
    if uid.is_empty() {
        return Err("未能获取 UserID".into());
    }

    let (machine_id, device_id) = take_ids();
    let cred = Credential {
        access_token,
        refresh_token,
        expires_at,
        domain: "trae.cn".into(),
        api_host: trae_signin_core::upstream::OAUTH_HOST.into(),
        machine_id,
        device_id,
        uid: uid.clone(),
        enterprise_id: String::new(),
        nickname: nickname.clone(),
    };

    let _ = app.emit("login://progress", LoginProgress {
        stage: "saving".into(),
        message: "保存凭证…".into(),
        uid: Some(uid.clone()),
        nickname: Some(nickname.clone()),
    });

    let state = crate::state::state(app);
    let dir = state
        .current_data_dir()
        .ok_or("数据目录未初始化，请先完成首次设置")?;
    trae_signin_core::auth::save_credential(&dir, &cred)
        .map_err(|e| format!("保存凭证失败: {e}"))?;

    Ok((uid, nickname))
}

// machine/device id 在 start_login 生成、回调落盘时取回：用全局暂存（跨 await 安全）
pub static LAST_IDS_GLOBAL: std::sync::Mutex<Option<(String, String)>> =
    std::sync::Mutex::new(None);

/// 记录本次登录的 machine/device id
pub fn stash_ids(machine_id: String, device_id: String) {
    *LAST_IDS_GLOBAL.lock().unwrap() = Some((machine_id, device_id));
}

pub fn take_ids() -> (String, String) {
    LAST_IDS_GLOBAL
        .lock()
        .unwrap()
        .take()
        .unwrap_or_else(|| (gen_machine_id(), gen_device_id()))
}

/// 等待 /authorize 回调，返回 query string
async fn wait_authorize(listener: &tokio::net::TcpListener) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("accept 失败: {e}"))?;
        let mut buf = vec![0u8; 8192];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("读取请求失败: {e}"))?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let request_line = req.lines().next().unwrap_or("");
        let path = request_line.split_whitespace().nth(1).unwrap_or("");
        // path 形如 /authorize?a=1&b=2
        let (route, query) = match path.split_once('?') {
            Some((r, q)) => (r, q.to_string()),
            None => (path, String::new()),
        };
        if route == "/authorize" {
            let html = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                SUCCESS_HTML.len(),
                SUCCESS_HTML
            );
            let _ = stream.write_all(html.as_bytes()).await;
            let _ = stream.flush().await;
            return Ok(query);
        }
        // 其他请求（如 favicon）→ 404，继续等待
        let resp = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            NOT_FOUND_HTML.len(),
            NOT_FOUND_HTML
        );
        let _ = stream.write_all(resp.as_bytes()).await;
    }
}

// ── 导入凭证 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub uid: String,
    pub nickname: String,
}

/// 粘贴 JSON 导入（校验 + 落盘）
pub fn import_credential_text(
    state: &AppState,
    text: &str,
) -> Result<ImportResult, String> {
    let dir = state.current_data_dir().ok_or("数据目录未初始化")?;
    let cred = trae_signin_core::auth::parse_credential(text)
        .map_err(|e| format!("凭证格式错误: {e}"))?;
    trae_signin_core::auth::save_credential(&dir, &cred).map_err(|e| e.to_string())?;
    Ok(ImportResult { uid: cred.uid, nickname: cred.nickname })
}

#[cfg(test)]
mod tests {
    use super::*;
    use trae_signin_core::login::urlencode;

    #[tokio::test]
    async fn probe_port_works() {
        let p = probe_port().await.unwrap();
        assert!((18080..=18089).contains(&p));
    }

    #[test]
    fn urlencode_ascii() {
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }
}
