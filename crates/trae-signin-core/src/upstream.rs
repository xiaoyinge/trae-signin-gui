//! 上游 API 客户端 — 全部常量集中此处，上游变更只改一处。
//!
//! 事实来源：Maquer/trae-signin（Go 源码提取，2026-09-14 复核；2026-09-16 再核：
//! 两仓库均无新提交，host/ClientID/版本/端点/`ugHeaders` 与本文件逐字一致）。
//! 注意 Go 侧 `doJSON` **从不解析响应体**，HTTP 200 + `{"code":9074,…}` 在它那里记成功——
//! 所以「按 code 白名单判定」只能靠本项目的 `app.log` 实测积累，不能向它对齐。
//! 解析对未知字段宽容：只取所需字段，不校验多余字段。

use crate::auth::Credential;
use crate::{CheckinStatus, CoreError};
use reqwest::StatusCode;
use serde_json::Value;

// ─────────────────────────── 常量（集中管理） ───────────────────────────

/// OAuth Host（换 token / 用户信息）
pub const OAUTH_HOST: &str = "https://api.trae.com.cn";
/// UG Host（签到 / 积分）
pub const UG_HOST: &str = "https://api.trae.cn";
/// ClientID
pub const CLIENT_ID: &str = "en1oxy7wnw8j9n";
/// IDE 版本
pub const IDE_VERSION: &str = "0.1.43";
/// IDE 构建号（仅记录上游事实；本客户端任何请求都不携带它）
pub const IDE_BUILD: &str = "20260716";
/// User-Agent
pub const USER_AGENT: &str = "Trae/0.1.43";
/// token 刷新缓冲：剩余 ≤ 2 小时即刷新
pub const REFRESH_BUFFER_SECS: i64 = 2 * 3600;
/// UG 签到类请求（status/claim）的 body：`{"req_source":2}`。
///
/// 为什么不是空 body：`req_source` 是客户端产品谱系标识——TRAE 谱系配
/// ClientID `ono9krqynydwx5`、SOLO 谱系配 `en1oxy7wnw8j9n`，官方客户端
/// 二选一从不交叉（社区对 TraeCode CN 2.3.79946 / TraeWork CN 2.3.81345
/// 反编译交叉实证）。本项目的 OAuth 走 `en1oxy7wnw8j9n`（SOLO 谱系），
/// 官方同款 body 为 `req_source=2`。2026-09-04 起上游收紧活动校验：
/// body 与 token 谱系不符（含空 body）的 claim 会被 9074
/// 「当前参与用户太多」拒绝；status 只读不受校验，故表现为
/// 「状态可读、签到被拒」。
pub const CHECKIN_REQ_SOURCE: i64 = 2;
/// 瞬时错误（429/5xx/网络/上游拥塞文案）下的总尝试次数。
/// 取 2 而不是更多：9074「当前参与用户太多」实测更像按客户端限流（同账号同 IP，
/// 客户端三连拒后 1 分钟人工补签即成功），一轮内密集重试只会加深窗口内的拒绝。
pub const UG_ATTEMPTS: usize = 2;

pub const EP_EXCHANGE_TOKEN: &str = "/cloudide/api/v3/trae/oauth/ExchangeToken";
pub const EP_GET_USER_INFO: &str = "/cloudide/api/v3/trae/GetUserInfo";
pub const EP_CHECKIN_STATUS: &str = "/trae/api/v2/ug/checkin_credits/status";
pub const EP_CHECKIN_CLAIM: &str = "/trae/api/v2/ug/checkin_credits/claim";
pub const EP_ENT_USAGE: &str = "/trae/api/v2/pay/ide_user_ent_usage";

// ─────────────────────────── 宽容 JSON 提取 ───────────────────────────

/// 归一化 key：小写并移除下划线/连字符（使 snake_case 与 camelCase 互通）
fn normalize_key(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 在 JSON 树中递归查找 key（忽略大小写与 `_`/`-` 差异，兼容 snake_case 与 camelCase）
pub fn find_key<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    let target = normalize_key(name);
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if normalize_key(k) == target {
                    return Some(val);
                }
            }
            for (_k, val) in map {
                if let Some(found) = find_key(val, name) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(|item| find_key(item, name)),
        _ => None,
    }
}

/// 取值优先序：顶层 → `data` 包装 → 全树递归。
/// `find_key` 本身已保证「先扫本层键」，所以顶层命中从来没问题；这里补的是中间那一层：
/// 没有顶层字段时，显式优先 `data` 包装，而不是落到全树递归的**兄弟子树字典序**上
/// （`{"aa":{"checkedIn":true},"data":{"checkedIn":false}}` 会先命中 `aa`）。
fn find_pref<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    let exact = |obj: &'a Value| {
        obj.as_object()
            .and_then(|m| m.iter().find(|(k, _)| normalize_key(k) == normalize_key(name)).map(|(_, val)| val))
    };
    exact(v)
        .or_else(|| v.get("data").and_then(exact))
        .or_else(|| find_key(v, name))
}

pub fn str_field(v: &Value, name: &str) -> Option<String> {
    find_pref(v, name).and_then(|x| x.as_str()).map(String::from)
}

pub fn i64_field(v: &Value, name: &str) -> Option<i64> {
    let x = find_pref(v, name)?;
    if let Some(n) = x.as_i64() {
        return Some(n);
    }
    if let Some(n) = x.as_f64() {
        return Some(n as i64);
    }
    if let Some(s) = x.as_str() {
        return s.trim().parse::<i64>().ok();
    }
    None
}

fn bool_field(v: &Value, name: &str) -> Option<bool> {
    find_pref(v, name).and_then(|x| x.as_bool())
}

/// 上游繁忙/拥塞文案特征。
///
/// 该类响应以 **HTTP 200** 送达（`code` 取值不作判据），但签到**并未生效**：
/// 实测 history.jsonl 中三次「当前参与用户太多，请稍后再试」后积分仍为 5750，
/// 人工补签后才变 5900。故必须判为瞬时失败并退避重试，不能按 2xx 记成功。
const CONGESTION_HINTS: &[&str] = &[
    "太多", "拥挤", "繁忙", "忙碌", "稍后", "重试", "too many", "busy", "later", "retry",
];

fn is_congestion(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    CONGESTION_HINTS.iter().any(|h| lower.contains(h))
}

// ─────────────────────────── API 客户端 ───────────────────────────

pub struct Upstream {
    http: reqwest::Client,
    oauth_base: String,
    ug_base: String,
    /// 瞬时错误重试的基础退避时长（测试可置 0）
    retry_backoff: std::time::Duration,
}

/// token 刷新结果
#[derive(Debug, Clone)]
pub struct RefreshedToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

/// 用户信息
#[derive(Debug, Clone, PartialEq)]
pub struct UserInfo {
    pub uid: String,
    pub nickname: String,
}

/// 签到执行结果
#[derive(Debug, Clone)]
pub struct CheckinOutcome {
    pub status: CheckinStatus,
    pub message: String,
}

/// 单账号签到全流程结果（含错误类型标记，复核结论 #8）
#[derive(Debug, Clone)]
pub struct SigninResult {
    pub outcome: CheckinOutcome,
    pub credits: Option<i64>,
    /// refreshToken 失效 → 需重新登录
    pub need_relogin: bool,
    /// 网络/超时类失败 → 可重试
    pub refresh_failed: bool,
    /// 状态查询或 claim 因瞬时错误未拿到结果 → 这一轮对该账号无进展，可稍后再来
    pub retryable: bool,
}

impl Default for Upstream {
    fn default() -> Self {
        Self::new()
    }
}

impl Upstream {
    pub fn new() -> Self {
        Self::with_bases(OAUTH_HOST, UG_HOST)
    }

    /// 可注入 base URL（测试用 httpmock 拦截）
    pub fn with_bases(oauth_base: &str, ug_base: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client build");
        Self {
            http,
            oauth_base: oauth_base.trim_end_matches('/').into(),
            ug_base: ug_base.trim_end_matches('/').into(),
            // 两次尝试之间隔 20s：秒级退避仍落在同一个限流窗口内，等于白发
            retry_backoff: std::time::Duration::from_secs(20),
        }
    }

    /// 覆盖瞬时错误重试的基础退避时长
    pub fn with_retry_backoff(mut self, d: std::time::Duration) -> Self {
        self.retry_backoff = d;
        self
    }

    // ── OAuth ──

    /// refreshToken → accessToken（ExchangeToken）
    pub async fn exchange_token(&self, refresh_token: &str) -> Result<RefreshedToken, CoreError> {
        let url = format!("{}{EP_EXCHANGE_TOKEN}", self.oauth_base);
        let body = serde_json::json!({
            "ClientID": CLIENT_ID,
            "RefreshToken": refresh_token,
            "ClientSecret": "-",
            "UserID": ""
        });
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let (kind, ce) = CoreError::classify_refresh(&e);
                match kind {
                    crate::RefreshErrorKind::AuthExpired => ce,
                    crate::RefreshErrorKind::Retryable => ce,
                }
            })?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(CoreError::AuthExpired(format!(
                    "ExchangeToken HTTP {status}: {text}"
                )));
            }
            return Err(CoreError::Retryable(format!(
                "ExchangeToken HTTP {status}: {text}"
            )));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| {
            CoreError::Other(format!("ExchangeToken 响应解析失败: {e}: {text}"))
        })?;
        let access_token = str_field(&v, "accessToken")
            .or_else(|| str_field(&v, "token"))
            .ok_or_else(|| CoreError::Other("ExchangeToken 未返回 accessToken".into()))?;
        let refresh_token = str_field(&v, "refreshToken").unwrap_or_else(|| refresh_token.into());
        let now = chrono::Utc::now().timestamp();
        let expires_at = i64_field(&v, "tokenExpireAt")
            .or_else(|| i64_field(&v, "expiresAt"))
            .map(crate::auth::normalize_expiry)
            .unwrap_or_else(|| {
                // 无过期时刻时用 duration 推算
                i64_field(&v, "tokenExpireDuration")
                    .map(crate::auth::normalize_expiry)
                    .map(|d| now + d)
                    .unwrap_or(now + 7 * 86400)
            });
        Ok(RefreshedToken {
            access_token,
            refresh_token,
            expires_at,
        })
    }

    /// 取 uid / nickname（header x-cloudide-token）
    pub async fn get_user_info(&self, access_token: &str) -> Result<UserInfo, CoreError> {
        let url = format!("{}{EP_GET_USER_INFO}", self.oauth_base);
        let resp = self
            .http
            .post(&url)
            .header("x-cloudide-token", access_token)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(CoreError::Other(format!(
                "GetUserInfo HTTP {status}: {text}"
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| CoreError::Other(format!("GetUserInfo 响应解析失败: {e}")))?;
        let uid = str_field(&v, "userID")
            .or_else(|| str_field(&v, "uid"))
            .ok_or_else(|| CoreError::Other("GetUserInfo 未返回 UserID".into()))?;
        let nickname = str_field(&v, "screenName").unwrap_or_default();
        Ok(UserInfo { uid, nickname })
    }

    // ── UG（签到 / 积分） ──

    /// UG 请求头，对齐 Go 客户端 `ugHeaders`：
    /// `Authorization` / `Accept` / `X-User-Region` 必发，`X-Device-Id` 仅非空时发。
    /// `Content-Type` 由 `.json()` 负责，`User-Agent` 由 client builder 负责。
    fn ug_request(&self, cred: &Credential, url: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(url)
            .header(
                "Authorization",
                format!("Cloud-IDE-JWT {}", cred.access_token),
            )
            .header("Accept", "application/json")
            .header("X-User-Region", "CN");
        if !cred.device_id.is_empty() {
            req = req.header("X-Device-Id", &cred.device_id);
        }
        req
    }

    /// UG 错误分级：401/403 认证失效；408/429/5xx 与传输层错误为瞬时（可重试）；其余不可重试
    fn classify_ug(what: &str, status: StatusCode, text: &str) -> CoreError {
        let snippet: String = text.chars().take(200).collect();
        let msg = format!("{what} HTTP {status}: {snippet}");
        match status.as_u16() {
            401 | 403 => CoreError::AuthExpired(msg),
            408 | 429 | 500..=599 => CoreError::Retryable(msg),
            _ => CoreError::Other(msg),
        }
    }

    /// 瞬时错误退避重试：第 n 次失败后等 `retry_backoff * 2^(n-1)` + 抖动
    async fn with_ug_retry<T, F, Fut>(&self, what: &str, f: F) -> Result<T, CoreError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, CoreError>>,
    {
        let mut attempt = 1usize;
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let transient =
                        matches!(e, CoreError::Retryable(_) | CoreError::Http(_));
                    if !transient || attempt >= UG_ATTEMPTS {
                        return Err(e);
                    }
                    let base = self.retry_backoff * (1u32 << (attempt - 1));
                    // 抖动随退避规模走：退避为 0（测试）时不引入等待
                    let jitter = if base.is_zero() {
                        std::time::Duration::ZERO
                    } else {
                        std::time::Duration::from_millis(
                            (chrono::Utc::now().timestamp_subsec_nanos() / 1_000_000) as u64 % 500,
                        )
                    };
                    log::warn!(
                        "{what} 瞬时失败，{:?} 后第 {}/{} 次尝试: {e}",
                        base + jitter,
                        attempt + 1,
                        UG_ATTEMPTS
                    );
                    attempt += 1;
                    tokio::time::sleep(base + jitter).await;
                }
            }
        }
    }

    /// 查询签到状态：返回 (今日是否已签, 是否启用)
    pub async fn checkin_status(&self, cred: &Credential) -> Result<(bool, bool), CoreError> {
        self.with_ug_retry("checkin status", || self.checkin_status_once(cred))
            .await
    }

    async fn checkin_status_once(&self, cred: &Credential) -> Result<(bool, bool), CoreError> {
        let url = format!("{}{EP_CHECKIN_STATUS}", self.ug_base);
        let resp = self
            .ug_request(cred, &url)
            .json(&serde_json::json!({ "req_source": CHECKIN_REQ_SOURCE }))
            .send()
            .await
            .map_err(|e| CoreError::Retryable(format!("checkin status 网络错误: {e}")))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(Self::classify_ug("checkin status", status, &text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| CoreError::Other(format!("checkin status 解析失败: {e}")))?;
        let checked_in = bool_field(&v, "checkedIn").unwrap_or(false);
        let enable = bool_field(&v, "enable").unwrap_or(true);
        Ok((checked_in, enable))
    }

    /// 执行签到（claim）
    ///
    /// 判定规则（三条路径实测/比对得来）：
    /// - `HTTP >= 400` → 失败；文案含"已签到/already check"归 ALREADY；401/403 → AuthExpired，408/429/5xx → Retryable
    /// - `HTTP 2xx` + 拥塞文案（"当前参与用户太多，请稍后再试" 等，**与 `code` 取值无关**）→ **Retryable**：签到确实没生效，交给 `with_ug_retry` 退避重试
    /// - `HTTP 2xx` + 其余情况 → 成功（对齐 Go `doJSON`：它从不解析响应体，只看状态码）
    ///
    /// `code` 只按「顶层 → `data` → 全树」优先序取，避免嵌套同名字段误判。完整响应体落 info 日志（`{数据目录}/app.log`），
    /// 用于核实上游 `code` 语义——文案匹配只是兜底，不是判据的最终形态。
    pub async fn checkin_claim(&self, cred: &Credential) -> Result<CheckinOutcome, CoreError> {
        self.with_ug_retry("claim", || self.checkin_claim_once(cred))
            .await
    }

    async fn checkin_claim_once(&self, cred: &Credential) -> Result<CheckinOutcome, CoreError> {
        let url = format!("{}{EP_CHECKIN_CLAIM}", self.ug_base);
        let resp = self
            .ug_request(cred, &url)
            .json(&serde_json::json!({ "req_source": CHECKIN_REQ_SOURCE }))
            .send()
            .await
            .map_err(|e| CoreError::Retryable(format!("claim 网络错误: {e}")))?;
        let status = resp.status();
        let text = resp.text().await?;
        if status.as_u16() >= 400 {
            // 错误信息含"已签到/already check"归为 ALREADY
            let lower = text.to_lowercase();
            if text.contains("已签到") || lower.contains("already check") {
                return Ok(CheckinOutcome {
                    status: CheckinStatus::Already,
                    message: "已签到".into(),
                });
            }
            return Err(Self::classify_ug("claim", status, &text));
        }
        log::info!("claim HTTP {status} 响应体: {text}");
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let message = str_field(&v, "message")
            .or_else(|| str_field(&v, "msg"))
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        let code = i64_field(&v, "code").unwrap_or(0);
        // 拥塞文案优先于 code：真正生效的一次签到不会回「用户太多，请稍后再试」。
        // 判据若绑在 code != 0 上，就等于依赖一个本机无日志后端、无法核实的外部字段取值。
        if let Some(m) = message.as_deref().filter(|m| is_congestion(m)) {
            return Err(CoreError::Retryable(format!("上游繁忙（code={code}）: {m}")));
        }
        if code != 0 {
            log::warn!("claim code={code} 非零但非拥塞文案，按上游 2xx 语义记为成功: {text}");
        }
        Ok(CheckinOutcome {
            status: CheckinStatus::Ok,
            message: message.unwrap_or_else(|| "签到成功".into()),
        })
    }

    /// 查询积分余额 = sum(user_entitlement_pack_list[].entitlement_base_info.quota.credits_limit)
    pub async fn query_credits(&self, cred: &Credential) -> Result<i64, CoreError> {
        let url = format!("{}{EP_ENT_USAGE}", self.ug_base);
        let resp = self
            .ug_request(cred, &url)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(CoreError::Other(format!(
                "ent usage HTTP {status}: {text}"
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| CoreError::Other(format!("ent usage 解析失败: {e}")))?;
        let mut total = 0i64;
        let packs = find_key(&v, "userEntitlementPackList");
        if let Some(Value::Array(list)) = packs {
            for pack in list {
                if let Some(limit) = find_key(pack, "creditsLimit").and_then(|x| {
                    x.as_i64().or_else(|| x.as_f64().map(|f| f as i64))
                }) {
                    total += limit;
                }
            }
        }
        Ok(total)
    }

    // ── 组合流程 ──

    /// 惰性刷新：expiresAt - now <= 2h（或已过期）时先刷新并回写凭证文件
    pub async fn ensure_fresh_token(
        &self,
        cred: &mut Credential,
        data_dir: &std::path::Path,
    ) -> Result<(), CoreError> {
        let now = chrono::Utc::now().timestamp();
        if cred.expires_at - now > REFRESH_BUFFER_SECS {
            return Ok(()); // 仍新鲜
        }
        if cred.refresh_token.is_empty() {
            return Err(CoreError::AuthExpired("无 refreshToken".into()));
        }
        let refreshed = self.exchange_token(&cred.refresh_token).await?;
        cred.access_token = refreshed.access_token;
        cred.refresh_token = refreshed.refresh_token;
        cred.expires_at = refreshed.expires_at;
        // 写盘失败不阻塞签到（与 ensure_device_id_format 同一原则）：
        // 内存中的新 token 本轮仍有效，下一轮会再尝试回写。
        // 最坏情况（轮换出的 refreshToken 未持久化 + 重启）会在下次刷新时
        // 显式报"需重新登录"，不会静默出错。
        if let Err(e) = crate::auth::save_credential(data_dir, cred) {
            log::warn!(
                "账号 {} token 刷新后写盘失败（本轮先用内存值）: {e}",
                cred.uid
            );
        }
        Ok(())
    }

    /// 迁移历史遗留的非官方格式 deviceId（32 位 hex UUID → 16 位数字 Aha 号）。
    ///
    /// claim 的活动校验会检查 `X-Device-Id` 形态，UUID 形态被 9074 拒绝。
    /// 迁移只影响 UG 请求头；写盘失败不阻塞签到（内存中的新 id 本轮仍生效，
    /// 下一轮会再尝试回写）。
    async fn ensure_device_id_format(
        cred: &mut Credential,
        data_dir: &std::path::Path,
    ) -> bool {
        if crate::login::is_valid_device_id(&cred.device_id) {
            return false;
        }
        cred.device_id = crate::login::gen_device_id();
        match crate::auth::save_credential(data_dir, cred) {
            Ok(_) => {
                log::info!("账号 {} 的 deviceId 为旧 UUID 格式（会触发 9074），已迁移为 16 位数字 Aha 设备号", cred.uid);
                true
            }
            Err(e) => {
                log::warn!("账号 {} deviceId 迁移写盘失败（本轮先用内存值）: {e}", cred.uid);
                true
            }
        }
    }

    /// 单账号签到全流程：刷新 token → status 判定 → claim
    pub async fn signin_account(
        &self,
        cred: &mut Credential,
        data_dir: &std::path::Path,
    ) -> SigninResult {
        Self::ensure_device_id_format(cred, data_dir).await;

        let mut outcome = CheckinOutcome {
            status: CheckinStatus::Unknown,
            message: String::new(),
        };
        let mut credits = None;
        let mut need_relogin = false;
        let mut refresh_failed = false;
        let mut retryable = false;

        match self.ensure_fresh_token(cred, data_dir).await {
            Err(CoreError::AuthExpired(m)) => {
                need_relogin = true;
                outcome.status = CheckinStatus::Failed;
                outcome.message = format!("刷新 token 失败（需重新登录）: {m}");
                return SigninResult { outcome, credits, need_relogin, refresh_failed, retryable };
            }
            Err(e) => {
                refresh_failed = true;
                outcome.status = CheckinStatus::Failed;
                outcome.message = format!("刷新 token 失败: {e}");
                return SigninResult { outcome, credits, need_relogin, refresh_failed, retryable };
            }
            Ok(()) => {}
        }

        match self.checkin_status(cred).await {
            Err(e) => {
                retryable = matches!(e, CoreError::Retryable(_) | CoreError::Http(_));
                outcome.status = CheckinStatus::Failed;
                outcome.message = format!("查询签到状态失败: {e}");
            }
            Ok((checked_in, enable)) => {
                if checked_in {
                    outcome.status = CheckinStatus::Already;
                    outcome.message = "今日已签到".into();
                } else if !enable {
                    outcome.status = CheckinStatus::Disabled;
                    outcome.message = "签到功能未启用".into();
                } else {
                    match self.checkin_claim(cred).await {
                        Ok(o) => outcome = o,
                        Err(e) => {
                            retryable = matches!(e, CoreError::Retryable(_) | CoreError::Http(_));
                            outcome.status = CheckinStatus::Failed;
                            outcome.message = format!("签到失败: {e}");
                        }
                    }
                }
            }
        }

        credits = self.query_credits(cred).await.ok();
        SigninResult { outcome, credits, need_relogin, refresh_failed, retryable }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;

    fn cred_with(token: &str, device: &str) -> Credential {
        Credential {
            access_token: token.into(),
            refresh_token: "rt".into(),
            expires_at: 4_102_444_800, // 2100-01-01，足够远
            domain: "trae.cn".into(),
            api_host: OAUTH_HOST.into(),
            machine_id: "m-1".into(),
            device_id: device.into(),
            uid: "u-1".into(),
            nickname: "测试".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn find_key_case_insensitive() {
        let v: Value = serde_json::json!({"A": {"bC": [{"dE": 5}]}});
        // "bc" 命中对象内层 "bC"（大小写不敏感），返回整个数组
        assert_eq!(find_key(&v, "bc"), Some(&serde_json::json!([{"dE": 5}])));
        assert_eq!(find_key(&v, "de"), Some(&serde_json::json!(5)));
        assert!(find_key(&v, "zz").is_none());
    }

    #[tokio::test]
    async fn exchange_token_parses_pascal_case() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST)
                .path(EP_EXCHANGE_TOKEN)
                .json_body(serde_json::json!({
                    "ClientID": CLIENT_ID,
                    "RefreshToken": "rt-old",
                    "ClientSecret": "-",
                    "UserID": ""
                }));
            then.status(200).body(
                r#"{"AccessToken":"at-new","RefreshToken":"rt-new","TokenExpireAt":1786858238000}"#,
            );
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let t = up.exchange_token("rt-old").await.unwrap();
        assert_eq!(t.access_token, "at-new");
        assert_eq!(t.refresh_token, "rt-new");
        assert_eq!(t.expires_at, 1786858238); // 毫秒归一化
        m.assert_hits(1);
    }

    #[tokio::test]
    async fn exchange_token_duration_fallback() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path(EP_EXCHANGE_TOKEN);
            then.status(200).body(
                r#"{"data":{"accessToken":"at-x","refreshToken":"rt-x","tokenExpireDuration":86400}}"#,
            );
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let t = up.exchange_token("rt").await.unwrap();
        assert_eq!(t.access_token, "at-x");
        let now = chrono::Utc::now().timestamp();
        assert!((t.expires_at - (now + 86400)).abs() <= 5);
        m.assert_hits(1);
    }

    #[tokio::test]
    async fn exchange_token_401_is_auth_expired() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path(EP_EXCHANGE_TOKEN);
            then.status(401).body("unauthorized");
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        match up.exchange_token("bad").await {
            Err(CoreError::AuthExpired(_)) => {}
            other => panic!("expect AuthExpired, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_user_info_variants() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path(EP_GET_USER_INFO);
            then.status(200).body(r#"{"Result":{"UserID":"u-9","ScreenName":"昵称9"}}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let info = up.get_user_info("at").await.unwrap();
        assert_eq!(info.uid, "u-9");
        assert_eq!(info.nickname, "昵称9");
        m.assert_hits(1);
    }

    #[tokio::test]
    async fn status_checked_in_and_disabled() {
        let server = MockServer::start();
        let mut m = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_STATUS);
            then.status(200).body(r#"{"checkedIn":true,"enable":true}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let (ci, en) = up.checkin_status(&cred_with("at", "d")).await.unwrap();
        assert!(ci && en);
        m.delete();

        let mut m2 = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_STATUS);
            then.status(200).body(r#"{"checkedIn":false,"enable":false}"#);
        });
        let (ci, en) = up.checkin_status(&cred_with("at", "d")).await.unwrap();
        assert!(!ci && !en);
        m2.delete();
    }

    #[test]
    fn ug_request_headers_match_go_client() {
        let up = Upstream::new();
        let req = up
            .ug_request(&cred_with("at-1", "dev-1"), "https://api.trae.cn/x")
            .build()
            .unwrap();
        let h = req.headers();
        assert_eq!(h["Authorization"], "Cloud-IDE-JWT at-1");
        assert_eq!(h["Accept"], "application/json");
        assert_eq!(h["X-User-Region"], "CN");
        assert_eq!(h["X-Device-Id"], "dev-1");
        // Go 客户端的 ugHeaders 里没有 X-Machine-Id
        assert!(h.get("X-Machine-Id").is_none());
    }

    #[test]
    fn ug_request_omits_empty_device_id() {
        let up = Upstream::new();
        let req = up
            .ug_request(&cred_with("at-1", ""), "https://api.trae.cn/x")
            .build()
            .unwrap();
        assert!(req.headers().get("X-Device-Id").is_none());
    }

    #[tokio::test]
    async fn claim_success_and_already() {
        let server = MockServer::start();
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        // 任一必需头缺失或 body 契约错误 → mock 不匹配 → 断言失败
        let mut m = server.mock(|when, then| {
            when.method(POST)
                .path(EP_CHECKIN_CLAIM)
                .header("Authorization", "Cloud-IDE-JWT at")
                .header("Accept", "application/json")
                .header("X-User-Region", "CN")
                .header("X-Device-Id", "d")
                .json_body(serde_json::json!({ "req_source": CHECKIN_REQ_SOURCE }));
            then.status(200).body(r#"{"code":0,"message":"签到成功"}"#);
        });
        let out = up.checkin_claim(&cred_with("at", "d")).await.unwrap();
        assert_eq!(out.status, CheckinStatus::Ok);
        m.delete();

        // HTTP 400 + already check 文案 → ALREADY
        let mut m2 = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(400).body(r#"{"message":"already checked in today"}"#);
        });
        let out = up.checkin_claim(&cred_with("at", "d")).await.unwrap();
        assert_eq!(out.status, CheckinStatus::Already);
        m2.delete();

        // 2xx + code!=0 + 非拥塞文案 → 仍记成功（对齐 Go：它从不解析响应体）
        let mut m3 = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(200).body(r#"{"code":4001,"message":"操作成功"}"#);
        });
        let out = up.checkin_claim(&cred_with("at", "d")).await.unwrap();
        assert_eq!(out.status, CheckinStatus::Ok);
        assert_eq!(out.message, "操作成功");
        m3.delete();

        // 2xx + 空 body → 成功
        let m4 = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(200).body("");
        });
        let out = up.checkin_claim(&cred_with("at", "d")).await.unwrap();
        assert_eq!(out.status, CheckinStatus::Ok);
        assert_eq!(out.message, "签到成功");
        m4.assert_hits(1);
    }

    /// status 请求同样携带 req_source 契约（对齐官方客户端，2026-09-04 上游收紧后
    /// 签到类请求的 body 必须与 token 谱系一致）。
    #[tokio::test]
    async fn status_sends_req_source_body() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST)
                .path(EP_CHECKIN_STATUS)
                .json_body(serde_json::json!({ "req_source": CHECKIN_REQ_SOURCE }));
            then.status(200).body(r#"{"checkedIn":false,"enable":true}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let (ci, en) = up.checkin_status(&cred_with("at", "d")).await.unwrap();
        assert!(!ci && en);
        m.assert_hits(1);
    }

    /// 回归锚点（2026-09-16 全天 9074 事故）：signin_account 会把 32 位 hex UUID
    /// 形态的旧 deviceId 迁移为 16 位数字 Aha 设备号并回写凭证文件；已是合法
    /// 格式的 deviceId 原样保留。
    #[tokio::test]
    async fn signin_migrates_legacy_hex_device_id() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_STATUS);
            then.status(200).body(r#"{"checkedIn":true,"enable":true}"#);
        });
        server.mock(|when, then| {
            when.method(POST).path(EP_ENT_USAGE);
            then.status(200).body(r#"{}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let dir = std::env::temp_dir().join(format!("tsup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // 旧格式：32 位 hex → 签到过程中迁移
        let mut cred = cred_with("at", "1942097ad2664caeb07cd7a4a6446a57");
        cred.uid = "u-mig".into();
        let r = up.signin_account(&mut cred, &dir).await;
        assert_eq!(r.outcome.status, CheckinStatus::Already);
        assert!(
            crate::login::is_valid_device_id(&cred.device_id),
            "签到后 deviceId 应为 16 位数字，实际 {}",
            cred.device_id
        );
        // 回写：重新读盘能拿到新 deviceId
        let reloaded = crate::auth::list_credentials(&dir)
            .unwrap()
            .into_iter()
            .find(|c| c.uid == "u-mig")
            .expect("迁移后的凭证应已落盘");
        assert_eq!(reloaded.device_id, cred.device_id);

        // 合法格式：保持不变
        let keep = "1234567890123456";
        let mut cred2 = cred_with("at", keep);
        cred2.uid = "u-keep".into();
        let _ = up.signin_account(&mut cred2, &dir).await;
        assert_eq!(cred2.device_id, keep);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归锚点：上游以 HTTP 200 + code!=0 + "当前参与用户太多，请稍后再试" 响应时，
    /// 签到**确实没生效**（实测 history.jsonl：三次该响应后积分仍 5750，人工补签才 5900）。
    /// 必须判为瞬时失败并退避重试，既不能记成功（骗用户），也不能记不可重试的失败（用户只能手动补签）。
    #[tokio::test]
    async fn claim_retries_upstream_busy_message() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(200)
                .body(r#"{"code":5001,"message":"当前参与用户太多，请稍后再试"}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""))
            .with_retry_backoff(std::time::Duration::ZERO);
        match up.checkin_claim(&cred_with("at", "d")).await {
            Err(CoreError::Retryable(msg)) => {
                assert!(msg.contains("当前参与用户太多"), "{msg}");
                assert!(msg.contains("5001"), "{msg}");
            }
            other => panic!("expect Retryable, got {other:?}"),
        }
        m.assert_hits(UG_ATTEMPTS);
    }

    /// 拥塞判定不得依赖 `code`：`code: 0` 与「无 code 字段」两种响应都回同样的文案，
    /// 签到同样没生效。若判据是 `code != 0`，这两份都会记成假成功。
    #[tokio::test]
    async fn claim_retries_busy_message_regardless_of_code() {
        for body in [
            r#"{"code":0,"message":"当前参与用户太多，请稍后再试"}"#,
            r#"{"message":"当前参与用户太多，请稍后再试"}"#,
        ] {
            let server = MockServer::start();
            let m = server.mock(|when, then| {
                when.method(POST).path(EP_CHECKIN_CLAIM);
                then.status(200).body(body);
            });
            let up = Upstream::with_bases(&server.url(""), &server.url(""))
                .with_retry_backoff(std::time::Duration::ZERO);
            match up.checkin_claim(&cred_with("at", "d")).await {
                Err(CoreError::Retryable(msg)) => assert!(msg.contains("当前参与用户太多"), "{msg}"),
                other => panic!("expect Retryable for {body}, got {other:?}"),
            }
            m.assert_hits(UG_ATTEMPTS);
        }
    }

    #[test]
    fn field_lookup_prefers_shallow_and_data_wrapper() {
        // 顶层字段恒优先于嵌套同名字段
        let v: Value = serde_json::json!({"aa":{"checkedIn":true},"checkedIn":false});
        assert_eq!(bool_field(&v, "checkedIn"), Some(false));
        // 无顶层时优先 data 包装，而不是落到兄弟子树的字典序上（全树扫描会先命中 aa）
        let v2: Value = serde_json::json!({"aa":{"checkedIn":true},"data":{"checkedIn":false}});
        assert_eq!(bool_field(&v2, "checkedIn"), Some(false));
        // data 包装下的 code 同样可读
        let v3: Value = serde_json::json!({"data":{"code":5001}});
        assert_eq!(i64_field(&v3, "code"), Some(5001));
    }

    #[tokio::test]
    async fn claim_retries_transient_5xx() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(503).body("busy");
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""))
            .with_retry_backoff(std::time::Duration::ZERO);
        match up.checkin_claim(&cred_with("at", "d")).await {
            Err(CoreError::Retryable(msg)) => assert!(msg.contains("503"), "{msg}"),
            other => panic!("expect Retryable, got {other:?}"),
        }
        m.assert_hits(UG_ATTEMPTS);
    }

    #[tokio::test]
    async fn claim_401_is_auth_expired_without_retry() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(401).body("unauthorized");
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        match up.checkin_claim(&cred_with("at", "d")).await {
            Err(CoreError::AuthExpired(_)) => {}
            other => panic!("expect AuthExpired, got {other:?}"),
        }
        m.assert_hits(1);
    }

    #[tokio::test]
    async fn credits_sum() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path(EP_ENT_USAGE);
            then.status(200).body(
                r#"{"user_entitlement_pack_list":[
                     {"entitlement_base_info":{"quota":{"credits_limit":5100}}},
                     {"entitlement_base_info":{"quota":{"credits_limit":300}}}
                   ]}"#,
            );
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let total = up.query_credits(&cred_with("at", "d")).await.unwrap();
        assert_eq!(total, 5400);
        m.assert_hits(1);
    }

    /// 回归：刷新成功但凭证写盘失败（如盘满/只读）时，ensure_fresh_token 仍 Ok，
    /// 内存中的新 token 生效——写盘失败不得中断本轮签到（与 deviceId 迁移同原则）。
    #[tokio::test]
    async fn ensure_fresh_token_survives_save_failure() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path(EP_EXCHANGE_TOKEN);
            then.status(200).body(r#"{"AccessToken":"at-new","RefreshToken":"rt-new","TokenExpireAt":4102444800000}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let dir = std::env::temp_dir().join(format!("tsup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // 占用 auths 名字为普通文件 → save_credential 的 create_dir_all 必败
        std::fs::write(dir.join("auths"), b"not a dir").unwrap();

        let mut cred = cred_with("at-old", "d");
        cred.expires_at = chrono::Utc::now().timestamp() - 10; // 已过期 → 必须刷新
        up.ensure_fresh_token(&mut cred, &dir).await.unwrap();
        assert_eq!(cred.access_token, "at-new");
        assert_eq!(cred.refresh_token, "rt-new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn signin_full_flow_mock() {
        let server = MockServer::start();
        // status: 未签、启用 → claim 成功 → 查积分
        server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_STATUS);
            then.status(200).body(r#"{"checkedIn":false,"enable":true}"#);
        });
        server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_CLAIM);
            then.status(200).body(r#"{"code":0,"message":"OK"}"#);
        });
        server.mock(|when, then| {
            when.method(POST).path(EP_ENT_USAGE);
            then.status(200).body(
                r#"{"user_entitlement_pack_list":[{"entitlement_base_info":{"quota":{"credits_limit":123}}}]}"#,
            );
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let dir = std::env::temp_dir().join(format!("tsup-{}", uuid::Uuid::new_v4()));
        let mut cred = cred_with("at", "d");
        let r = up.signin_account(&mut cred, &dir).await;
        assert_eq!(r.outcome.status, CheckinStatus::Ok);
        assert_eq!(r.credits, Some(123));
        assert!(!r.need_relogin && !r.refresh_failed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn signin_already_flow_mock() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path(EP_CHECKIN_STATUS);
            then.status(200).body(r#"{"checkedIn":true,"enable":true}"#);
        });
        server.mock(|when, then| {
            when.method(POST).path(EP_ENT_USAGE);
            then.status(200).body(r#"{}"#);
        });
        let up = Upstream::with_bases(&server.url(""), &server.url(""));
        let dir = std::env::temp_dir().join(format!("tsup-{}", uuid::Uuid::new_v4()));
        let mut cred = cred_with("at", "d");
        let r = up.signin_account(&mut cred, &dir).await;
        assert_eq!(r.outcome.status, CheckinStatus::Already);
        assert_eq!(r.credits, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
