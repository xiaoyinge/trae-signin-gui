//! 登录链接生成与回调解析
//!
//! 登录链接：`https://www.trae.cn/authorization?` + query
//! 回调：`http://127.0.0.1:{port}/authorize?refreshToken=..&userInfo=..&userJwt=..`

use crate::CoreError;

/// 登录链接所需参数
#[derive(Debug, Clone)]
pub struct LoginParams {
    /// 回调端口（先探测再生成链接）
    pub port: u16,
    /// 32 位 hex，每账号独立生成并随凭证保存
    pub machine_id: String,
    /// 16 位纯数字 Aha 设备号（`gen_device_id` 生成），每账号独立生成并随凭证保存
    pub device_id: String,
    /// 32 位 hex 追踪 id；上游回调时以 `loginTraceID` 原样带回，作为防注入比对值（PLAN §10 #7）
    pub login_trace_id: String,
}

pub const LOGIN_BASE_URL: &str = "https://www.trae.cn/authorization";
pub const PLUGIN_VERSION: &str = "2.3.62834";

/// 生成登录链接（query 顺序与上游一致）
///
/// `login_trace_id` 随登录 URL 发起，上游回调时以 `loginTraceID` **原样带回**（2026-09-29
/// 实测），作为回调防注入比对值。PLAN §10 #7 的修正：原方案把 `state` 拼进
/// `auth_callback_url`，但上游授权页**不接受带 query 的回调地址**（页面直接报
/// 「登录失败/网络错误」，2026-09-29 实验 B 实锤），故回调地址回归纯
/// `http://127.0.0.1:{port}/authorize`，比对改走上游自带的 loginTraceID 回传通道——
/// 防注入强度不变：该值 128 位熵，只存在于本机会话与登录 URL 中，伪造回调无从得知。
pub fn build_login_url(p: &LoginParams) -> String {
    let callback = format!("http://127.0.0.1:{}/authorize", p.port);
    format!(
        "{LOGIN_BASE_URL}?login_version=1&auth_from=solo&login_channel=native_ide\
&plugin_version={PLUGIN_VERSION}&auth_type=local&client_id={}&redirect=0\
&login_trace_id={}&auth_callback_url={}&machine_id={}&device_id={}\
&x_device_id={}&x_machine_id={}&x_device_brand=PC&x_device_type=PC\
&x_os_version=1.0&x_app_version={}&x_app_type=stable",
        crate::upstream::CLIENT_ID,
        p.login_trace_id,
        urlencode(&callback),
        p.machine_id,
        p.device_id,
        p.device_id,
        p.machine_id,
        crate::upstream::IDE_VERSION,
    )
}

/// 生成随机的 machine id（32 位 hex simple uuid）
pub fn gen_machine_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 生成设备号（16 位十进制数字，官方 Aha 设备号格式）。
///
/// 为什么不能用 32 位 hex UUID：2026-09-16 起上游对 claim 的活动校验会检查
/// `X-Device-Id` 形态，非 16 位纯数字设备号被一律以 9074「当前参与用户太多」
/// 拒绝（status 只读不受校验，故表现为"状态可读、签到被拒"）。
/// 官方客户端本地存储的设备号即 16 位数字 Aha 号（社区反编译实证）。
pub fn gen_device_id() -> String {
    let n = uuid::Uuid::new_v4().as_u128() % 9_000_000_000_000_000 + 1_000_000_000_000_000;
    n.to_string()
}

/// deviceId 是否符合官方 Aha 设备号形态：纯数字、12~20 位。
///
/// 2026-09-29 从「严格 16 位」放宽（PLAN §9）：社区实测存在 **15 位真号**
/// （吾爱破解 tid=2130100 v1.1——那台机器 `storage.json` 里就是 15 位），旧实现会把
/// 它判非法并静默替换成随机生成号；若账号已绑定原真号，替换后的新号 claim 一律
/// 9074，等于把能用的号弄坏。「16 位」是观察不是规则，12~20 与该帖防呆边界一致。
/// 32 位 hex UUID 仍不合法（claim 会 9074，见 `upstream` 的 `ensure_device_id_format`）。
pub fn is_valid_device_id(d: &str) -> bool {
    (12..=20).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit())
}

/// 从 TRAE 客户端 `storage.json` 文本中提取真实设备号。
///
/// 号写在**键名**上：`iCubeAuthInfo://icube-dc:{12~20位数字}`（无需解密，2026-09-29 实测）。
/// 值一律忽略（其中可能含 token）。解析失败 / 无命中 → 空列表。
pub fn extract_device_ids_from_storage_json(text: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(obj) = v.as_object() {
        for k in obj.keys() {
            if let Some(d) = k.strip_prefix("iCubeAuthInfo://icube-dc:") {
                let d = d.trim();
                if is_valid_device_id(d) && !out.iter().any(|x: &String| x == d) {
                    out.push(d.to_string());
                }
            }
        }
    }
    out
}

/// 探测本机 TRAE 客户端（CN 版 / 国际版）的 storage.json 并提取真实设备号。
///
/// 未安装 / 未登录 / 文件异常 → 空列表。调用方自行决定采用策略
/// （唯一号可自动填入；多号无法判定归属，应交由用户手动选择）。
pub fn detect_client_device_ids() -> Vec<String> {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return Vec::new();
    };
    for dir in ["Trae CN", "Trae"] {
        let path = std::path::Path::new(&appdata)
            .join(dir)
            .join("User")
            .join("globalStorage")
            .join("storage.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let ids = extract_device_ids_from_storage_json(&text);
            if !ids.is_empty() {
                return ids;
            }
        }
    }
    Vec::new()
}

/// 生成 32 位 hex trace id，同时充当回调 state：
/// 回调到达时必须原样带回，防止本机任意进程/网页向回调端口注入伪造凭证
/// （PLAN §10 #7；2026-09-23 拍板从 16 位升级到 128 位熵）。
pub fn gen_trace_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 极简 percent-encode（组件级：保留字母数字与 -._~）
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// percent-decode（含 '+' → 空格）
pub fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| {
                    std::str::from_utf8(h)
                        .ok()
                        .and_then(|x| u8::from_str_radix(x, 16).ok())
                });
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 把 query string 解析为有序键值对（不重建 URL，仅拆分）
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let mut it = pair.splitn(2, '=');
            let k = urldecode(it.next().unwrap_or(""));
            let v = urldecode(it.next().unwrap_or(""));
            (k, v)
        })
        .collect()
}

/// 回调解析结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallbackData {
    /// query 中直接的 refreshToken（优先）
    pub refresh_token: String,
    /// userInfo JSON 内的 UserID
    pub uid: String,
    /// userInfo JSON 内的 ScreenName
    pub screen_name: String,
    /// userJwt.Token（无 refreshToken 时的兜底 access token）
    pub fallback_access_token: String,
    /// userJwt.TokenExpireAt（兼容秒/毫秒）
    pub expires_at: Option<i64>,
}

impl CallbackData {
    /// 是否具备可登录的最低条件
    pub fn is_usable(&self) -> bool {
        !self.refresh_token.is_empty() || !self.fallback_access_token.is_empty()
    }
}

/// 解析 `/authorize?...` 的完整 query（不含 `?`）
pub fn parse_callback_query(query: &str) -> Result<CallbackData, CoreError> {
    let pairs = parse_query(query);
    let get = |name: &str| -> Option<String> {
        pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };

    let mut data = CallbackData {
        refresh_token: get("refreshToken").unwrap_or_default(),
        ..Default::default()
    };

    // 解析失败必须上抛（PLAN §10 #7）：静默跳过会保存出残缺凭证
    // （如 userJwt 被截断后 JSON 解析失败 → 假过期时间 / 空 token）。
    if let Some(user_info_raw) = get("userInfo") {
        let v: serde_json::Value = serde_json::from_str(&user_info_raw)
            .map_err(|e| CoreError::Other(format!("userInfo JSON 解析失败: {e}")))?;
        data.uid = crate::upstream::str_field(&v, "UserID")
            .or_else(|| crate::upstream::str_field(&v, "uid"))
            .unwrap_or_default();
        data.screen_name = crate::upstream::str_field(&v, "ScreenName").unwrap_or_default();
    }

    if let Some(jwt_raw) = get("userJwt") {
        let v: serde_json::Value = serde_json::from_str(&jwt_raw)
            .map_err(|e| CoreError::Other(format!("userJwt JSON 解析失败: {e}")))?;
        data.fallback_access_token =
            crate::upstream::str_field(&v, "Token").unwrap_or_default();
        if let Some(e) = crate::upstream::i64_field(&v, "TokenExpireAt") {
            data.expires_at = Some(crate::auth::normalize_expiry(e));
        }
    }

    if !data.is_usable() {
        return Err(CoreError::Other(
            "回调缺少 refreshToken / userJwt.Token，无法登录".into(),
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_url_contains_required_params() {
        let p = LoginParams {
            port: 18080,
            machine_id: "m123".into(),
            device_id: "d123".into(),
            login_trace_id: gen_trace_id(),
        };
        let url = build_login_url(&p);
        assert!(url.starts_with("https://www.trae.cn/authorization?"));
        assert!(url.contains("client_id=en1oxy7wnw8j9n"));
        assert!(url.contains("login_version=1"));
        assert!(url.contains("auth_from=solo"));
        assert!(url.contains("login_channel=native_ide"));
        assert!(url.contains("plugin_version=2.3.62834"));
        assert!(url.contains("auth_type=local"));
        assert!(url.contains("redirect=0"));
        assert!(url.contains(&format!("login_trace_id={}", p.login_trace_id)));
        // 回调地址不带 query：上游授权页不接受带 ?state= 的回调地址（2026-09-29 实验 B 实锤），
        // 防注入比对走 login_trace_id 参数（回调以 loginTraceID 原样带回）
        assert!(url.contains(&format!(
            "auth_callback_url={}",
            urlencode("http://127.0.0.1:18080/authorize")
        )));
        assert!(url.contains("machine_id=m123"));
        assert!(url.contains("device_id=d123"));
        assert!(url.contains("x_device_id=d123"));
        assert!(url.contains("x_machine_id=m123"));
        assert!(url.contains("x_device_brand=PC"));
        assert!(url.contains("x_device_type=PC"));
        assert!(url.contains("x_os_version=1.0"));
        assert!(url.contains("x_app_version=0.1.43"));
        assert!(url.contains("x_app_type=stable"));
    }

    #[test]
    fn url_encode_decode_roundtrip() {
        let raw = "http://127.0.0.1:18080/authorize?a=1&b=中 文";
        let enc = urlencode(raw);
        assert!(!enc.contains(' '));
        assert!(!enc.contains('&') || enc == raw);
        assert_eq!(urldecode(&enc), raw);
        assert_eq!(urldecode("a+b"), "a b");
    }

    #[test]
    fn parse_callback_full() {
        let user_info = serde_json::json!({"UserID":"u-77","ScreenName":"小明"}).to_string();
        let user_jwt = serde_json::json!({
            "Token":"at-1","RefreshToken":"rt-1","TokenExpireAt":1786858238000i64
        })
        .to_string();
        let q = format!(
            "code=0&refreshToken={}&userInfo={}&userJwt={}",
            urlencode("rt-direct"),
            urlencode(&user_info),
            urlencode(&user_jwt)
        );
        let d = parse_callback_query(&q).unwrap();
        assert_eq!(d.refresh_token, "rt-direct");
        assert_eq!(d.uid, "u-77");
        assert_eq!(d.screen_name, "小明");
        assert_eq!(d.fallback_access_token, "at-1");
        assert_eq!(d.expires_at, Some(1786858238));
        assert!(d.is_usable());
    }

    #[test]
    fn parse_callback_fallback_token_only() {
        let user_jwt = serde_json::json!({"Token":"at-only"}).to_string();
        let q = format!("userJwt={}", urlencode(&user_jwt));
        let d = parse_callback_query(&q).unwrap();
        assert!(d.refresh_token.is_empty());
        assert_eq!(d.fallback_access_token, "at-only");
    }

    #[test]
    fn parse_callback_rejects_empty() {
        assert!(parse_callback_query("code=0&state=x").is_err());
    }

    #[test]
    fn gen_device_id_is_16_digit_aha_format() {
        for _ in 0..50 {
            let d = gen_device_id();
            assert!(is_valid_device_id(&d), "应为 16 位纯数字，实际 {d}");
            let n: u128 = d.parse().unwrap();
            assert!((1_000_000_000_000_000..10_000_000_000_000_000).contains(&n));
        }
    }

    /// 设备号形态边界（2026-09-29 放宽，PLAN §9）：
    /// 15 位真号实测存在（tid=2130100 v1.1），不得再被误杀；32 位 hex 仍不合法。
    #[test]
    fn device_id_format_boundaries() {
        // 32 位 hex UUID 形态必须被判为不合法（9074 风控触发形态）
        assert!(!is_valid_device_id("1942097ad2664caeb07cd7a4a6446a57"));
        assert!(!is_valid_device_id(""));
        // 15 位真号（旧实现误杀）
        assert!(is_valid_device_id("123456789012345"));
        // 17 位纯数字（旧实现同样误杀）
        assert!(is_valid_device_id("12345678901234567"));
        // 防呆边界 12~20
        assert!(is_valid_device_id("123456789012"));
        assert!(is_valid_device_id("12345678901234567890"));
        assert!(!is_valid_device_id("12345678901"));
        assert!(!is_valid_device_id("123456789012345678901"));
        // 含非数字不合法
        assert!(!is_valid_device_id("123456789012345a"));
    }

    /// 从客户端 storage.json 提取设备号：键名 `iCubeAuthInfo://icube-dc:{号}`（2026-09-29 实测形态）
    #[test]
    fn extract_client_device_ids_from_storage() {
        let text = r#"{
            "iCubeAuthInfo://icube-dc:1112223334445566": "{\"token\":\"...\"}",
            "other.key": 1,
            "iCubeAuthInfo://icube-dc: 1112223334445566 ": "dup-with-spaces",
            "iCubeAuthInfo://icube-dc:1942097ad2664caeb07cd7a4a6446a57": "hex-ignored"
        }"#;
        assert_eq!(
            extract_device_ids_from_storage_json(text),
            vec!["1112223334445566"]
        );
        assert!(extract_device_ids_from_storage_json("not json").is_empty());
        assert!(extract_device_ids_from_storage_json("{}").is_empty());
    }

    #[test]
    fn gen_ids_format() {
        assert_eq!(gen_machine_id().len(), 32);
        assert!(gen_machine_id().chars().all(|c| c.is_ascii_hexdigit()));
        // state 绑定要求 128 位熵：32 位 hex（PLAN §10 #7）
        assert_eq!(gen_trace_id().len(), 32);
        assert!(gen_trace_id().chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// 残缺回调必须上抛而非静默兜底（PLAN §10 #7）：
    /// userJwt 被截断时 JSON 解析失败，静默跳过会保存出空 token / 假过期凭证。
    #[test]
    fn parse_callback_rejects_malformed_json() {
        // userInfo 坏 JSON
        let q = format!("refreshToken=rt&userInfo={}", urlencode("{bad json"));
        assert!(parse_callback_query(&q).is_err());
        // userJwt 坏 JSON
        let q = format!("refreshToken=rt&userJwt={}", urlencode("{\"Token\":"));
        assert!(parse_callback_query(&q).is_err());
    }
}
