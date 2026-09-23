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

/// 探测可用端口并**保留 listener**（PLAN §10 #7 / M1 修复）：
/// 先 bind 后 drop 再重 bind 存在 TOCTOU——链接已按该端口生成，真 bind 失败时用户已授权完毕。
/// `PermissionDenied`（Hyper-V 保留端口段）立即报错，不得误报成"均被占用"。
pub async fn probe_listener() -> Result<(u16, tokio::net::TcpListener), String> {
    let mut last_err: Option<std::io::Error> = None;
    for port in PORT_RANGE {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => return Ok((port, l)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(format!(
                    "端口 {port} 被系统保留（Hyper-V 排除端口段），请在排除列表中移除 {}-{} 后重试",
                    PORT_RANGE.start(),
                    PORT_RANGE.end()
                ));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(format!(
        "回调端口 {}-{} 均被占用（{last_err:?}），请释放端口后重试",
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
    // listener 与端口同时取得：会话直接持有，杜绝重 bind 失败的 TOCTOU
    let (port, listener) = probe_listener().await?;
    let params = LoginParams {
        port,
        machine_id: gen_machine_id(),
        device_id: gen_device_id(),
        login_trace_id: gen_trace_id(),
    };
    let url = build_login_url(&params);
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
        // machine/device id 与 state 以参数形式穿进会话（PLAN §10 #7，删除全局单槽），
        // 两会话重叠时不再可能取到对方的 ids。
        let task = tauri::async_runtime::spawn(async move {
            let result = run_callback_server(&app2, listener, params).await;
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

/// 回调服务主流程：监听 → 等待 /authorize（校验 state/Host/GET）→ 换 token → 落盘。
/// 成功返回 (uid, nickname)。listener 与登录参数由 start_login 传入（会话自有，无全局状态）。
async fn run_callback_server(
    app: &AppHandle,
    listener: tokio::net::TcpListener,
    params: LoginParams,
) -> Result<(String, String), String> {
    // 等待回调（5 分钟超时）
    let query = tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS)) =>
            return Err("登录超时（5 分钟）".into()),
        r = wait_authorize(&listener, params.port, &params.login_trace_id) => r?,
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
        // 假过期兜底已删（PLAN §10 #7）：字段缺失一律上抛，绝不静默造 now+7d
        let exp = data
            .expires_at
            .ok_or("回调 userJwt 缺少 TokenExpireAt，无法确定 token 有效期")?;
        (data.fallback_access_token.clone(), String::new(), exp)
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

    let cred = Credential {
        access_token,
        refresh_token,
        expires_at,
        domain: "trae.cn".into(),
        api_host: trae_signin_core::upstream::OAUTH_HOST.into(),
        machine_id: params.machine_id,
        device_id: params.device_id,
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

/// 请求头读取上限（PLAN §10 #7）：userJwt+userInfo 百分号编码后可超 8 KiB，
/// 单次固定 8192 读会把 query 截断成残缺凭证；读到 `\r\n\r\n` 为止，超过即 413。
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// 等待 /authorize 回调，返回**已通过校验**的 query string。
///
/// 防护（PLAN §10 #7）：
/// - 仅接受 `GET`（其他方法 405）；
/// - 校验 `Host` 头必须是 `127.0.0.1:{port}`（防 DNS rebinding）；
/// - 严格比对 query 中的 `state` 与会话生成值（防本机任意进程/网页注入伪造凭证）；
/// - 请求头读满 64 KiB 仍无 `\r\n\r\n` → 413 并放弃该连接（会话继续等待）。
async fn wait_authorize(
    listener: &tokio::net::TcpListener,
    port: u16,
    expected_state: &str,
) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("accept 失败: {e}"))?;

        // 循环读到请求头结束（\r\n\r\n），带字节上限
        let mut buf: Vec<u8> = Vec::with_capacity(2048);
        let mut chunk = [0u8; 2048];
        let mut header_end: Option<usize> = None;
        loop {
            if buf.len() > MAX_REQUEST_BYTES {
                let _ = stream
                    .write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                // 排空对端剩余数据再关连接：带着未读数据关闭会触发 RST、
                // 把刚写出的 413 从对端缓冲里吞掉。500ms 上限防恶意客户端灌数据。
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    drain(&mut stream),
                )
                .await;
                log::warn!("回调请求超过 {MAX_REQUEST_BYTES} 字节，已拒绝（等待真实回调中）");
                break;
            }
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|e| format!("读取请求失败: {e}"))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_header_end(&buf) {
                header_end = Some(pos);
                break;
            }
        }
        let Some(end) = header_end else {
            // 对端在头结束前断开 / 超限被拒：继续等待下一个请求
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..end]).into_owned();

        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        if !method.eq_ignore_ascii_case("GET") {
            let _ = stream
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }

        // Host 校验（防 DNS rebinding）：必须 127.0.0.1:{port}
        let host_ok = lines
            .by_ref()
            .take_while(|l| !l.is_empty())
            .any(|l| {
                l.strip_prefix("Host:")
                    .map(|v| v.trim().eq_ignore_ascii_case(&format!("127.0.0.1:{port}")))
                    .unwrap_or(false)
            });
        if !host_ok {
            log::warn!("回调请求 Host 不符，已拒绝");
            let _ = stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }

        // target 形如 /authorize?a=1&b=2
        let (route, query) = match target.split_once('?') {
            Some((r, q)) => (r, q.to_string()),
            None => (target, String::new()),
        };
        if route != "/authorize" {
            // 其他请求（如 favicon）→ 404，继续等待
            let resp = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                NOT_FOUND_HTML.len(),
                NOT_FOUND_HTML
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            continue;
        }

        // state 严格比对：不匹配（伪造/重放/过期链接）→ 403 并继续等待真实回调
        let state_ok = trae_signin_core::login::parse_query(&query)
            .into_iter()
            .any(|(k, v)| k == "state" && v == expected_state);
        if !state_ok {
            log::warn!("回调 state 校验失败，疑似伪造回调，已拒绝（等待真实回调中）");
            let _ = stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }

        let html = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            SUCCESS_HTML.len(),
            SUCCESS_HTML
        );
        let _ = stream.write_all(html.as_bytes()).await;
        let _ = stream.flush().await;
        return Ok(query);
    }
}

/// 在缓冲区中定位 `\r\n\r\n`（请求头结束位置，返回其后首个字节的下标）
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// 读取并丢弃对端后续数据直到 EOF（配合 413 使用）
async fn drain(s: &mut tokio::net::TcpStream) {
    let mut sink = [0u8; 8192];
    loop {
        match s.read(&mut sink).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
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
    async fn probe_listener_keeps_listener_bound() {
        // 返回的 listener 必须仍处于监听状态（M1 修复：先 bind 后 drop 再重 bind 的 TOCTOU）
        let (port, listener) = probe_listener().await.unwrap();
        assert!((18080..=18089).contains(&port));
        drop(listener); // 归还端口，供后续测试使用
    }

    #[test]
    fn urlencode_ascii() {
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }

    /// 连接回调服务，发送原始请求，读取服务端响应（带超时；EOF 前收到的部分也算）
    async fn send_and_read(port: u16, raw: String) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(raw.as_bytes()).await.unwrap();
        // 发完即关写侧：服务端的排空逻辑依赖 EOF 结束
        let _ = s.shutdown().await;
        let mut resp = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), s.read_to_end(&mut resp)).await;
        String::from_utf8_lossy(&resp).into_owned()
    }

    fn http_get(query: &str, host: &str) -> String {
        format!("GET /authorize?{query} HTTP/1.1\r\nHost: {host}\r\n\r\n")
    }

    /// 并发驱动 waiter 与 client：非法请求场景下 waiter 必须继续等待而不是返回。
    /// 返回时 waiter future 被 drop（accept 停止），listener 由调用方持有。
    async fn reject_case(port: u16, listener: &tokio::net::TcpListener, state: &str, raw: String) -> String {
        let client = tokio::spawn(send_and_read(port, raw));
        let waiter = wait_authorize(listener, port, state);
        tokio::pin!(waiter);
        tokio::select! {
            r = &mut waiter => panic!("非法回调不得结束会话: {r:?}"),
            r = client => r.unwrap(),
        }
    }

    #[tokio::test]
    async fn wait_authorize_accepts_valid_state() {
        let (port, listener) = probe_listener().await.unwrap();
        let st = "abc123";
        let req = http_get(&format!("state={st}&refreshToken=rt"), &format!("127.0.0.1:{port}"));
        let client = tokio::spawn(send_and_read(port, req));
        let q = wait_authorize(&listener, port, st).await.unwrap();
        let resp = client.await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
        assert_eq!(q, format!("state={st}&refreshToken=rt"));
    }

    #[tokio::test]
    async fn wait_authorize_rejects_wrong_state() {
        let (port, listener) = probe_listener().await.unwrap();
        let resp = reject_case(
            port,
            &listener,
            "real-state",
            http_get("state=forged&refreshToken=evil", &format!("127.0.0.1:{port}")),
        )
        .await;
        assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    }

    #[tokio::test]
    async fn wait_authorize_rejects_missing_state() {
        let (port, listener) = probe_listener().await.unwrap();
        let resp = reject_case(
            port,
            &listener,
            "real-state",
            http_get("refreshToken=rt", &format!("127.0.0.1:{port}")),
        )
        .await;
        assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    }

    #[tokio::test]
    async fn wait_authorize_rejects_bad_host() {
        // DNS rebinding 场景：攻击者域名解析到 127.0.0.1，Host 头不是 127.0.0.1:port
        let (port, listener) = probe_listener().await.unwrap();
        let resp = reject_case(
            port,
            &listener,
            "real-state",
            http_get("state=real-state", "evil.example.com"),
        )
        .await;
        assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    }

    #[tokio::test]
    async fn wait_authorize_rejects_non_get() {
        let (port, listener) = probe_listener().await.unwrap();
        let raw = format!(
            "POST /authorize?state=real-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        );
        let resp = reject_case(port, &listener, "real-state", raw).await;
        assert!(resp.starts_with("HTTP/1.1 405"), "{resp}");
    }

    #[tokio::test]
    async fn wait_authorize_reads_headers_across_chunks() {
        // H5 回归：query 超过旧实现的单次 8192 读取窗口时不得截断
        let (port, listener) = probe_listener().await.unwrap();
        let st = "s".repeat(32);
        let padding = "x".repeat(10 * 1024);
        let req = http_get(&format!("pad={padding}&state={st}"), &format!("127.0.0.1:{port}"));
        let client = tokio::spawn(send_and_read(port, req));
        let q = wait_authorize(&listener, port, &st).await.unwrap();
        let resp = client.await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
        assert!(q.ends_with(&format!("state={st}")));
    }

    #[tokio::test]
    async fn wait_authorize_rejects_oversized_headers() {
        // 超过 64 KiB 上限：413 拒绝，且不得把残缺请求当回调
        let (port, listener) = probe_listener().await.unwrap();
        let padding = "x".repeat(80 * 1024);
        let resp = reject_case(
            port,
            &listener,
            "real-state",
            http_get(&format!("pad={padding}&state=real-state"), &format!("127.0.0.1:{port}")),
        )
        .await;
        assert!(resp.starts_with("HTTP/1.1 413"), "{resp}");
    }

    #[test]
    fn find_header_end_scans() {
        assert_eq!(find_header_end(b"GET / HTTP/1.1\r\nHost: a\r\n\r\nbody"), Some(27));
        assert_eq!(find_header_end(b"no end yet"), None);
    }
}
