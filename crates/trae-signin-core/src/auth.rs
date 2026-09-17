//! 凭证文件解析与原子写回（兼容嵌套与扁平两种历史形态，对齐原 Go `auth.Parse`）
//!
//! 文件：`auths/trae-{uid}.json`
//!
//! 嵌套形态：
//! ```json
//! { "auth": { "accessToken": "...", "refreshToken": "...", "expiresAt": 1786858238,
//!             "domain": "trae.cn", "apiHost": "https://api.trae.com.cn",
//!             "machineId": "...", "deviceId": "..." },
//!   "account": { "uid": "...", "enterpriseId": "", "nickname": "..." } }
//! ```
//!
//! 扁平形态：上述字段直接置于顶层（accessToken / uid / nickname ...）。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 单个账号的完整凭证
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Credential {
    #[serde(rename = "accessToken", default)]
    pub access_token: String,
    #[serde(rename = "refreshToken", default)]
    pub refresh_token: String,
    /// 统一为秒级 Unix 时间戳（解析时兼容秒/毫秒）
    #[serde(rename = "expiresAt", default)]
    pub expires_at: i64,
    #[serde(default)]
    pub domain: String,
    #[serde(rename = "apiHost", default)]
    pub api_host: String,
    #[serde(rename = "machineId", default)]
    pub machine_id: String,
    #[serde(rename = "deviceId", default)]
    pub device_id: String,
    #[serde(default)]
    pub uid: String,
    #[serde(rename = "enterpriseId", default)]
    pub enterprise_id: String,
    #[serde(default)]
    pub nickname: String,
}

/// 过期时间归一化：`> 1e12` 视为毫秒，除以 1000
pub fn normalize_expiry(v: i64) -> i64 {
    if v > 1_000_000_000_000 {
        v / 1000
    } else {
        v
    }
}

/// 凭证文件名：`trae-{uid}.json`
pub fn credential_file_name(uid: &str) -> String {
    format!("trae-{uid}.json")
}

/// auths 子目录
pub fn auths_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("auths")
}

/// 解析凭证 JSON（宽容：兼容嵌套/扁平、未知字段忽略、缺字段为默认值）
pub fn parse_credential(text: &str) -> Result<Credential, crate::CoreError> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    parse_credential_value(&v)
}

fn parse_credential_value(v: &serde_json::Value) -> Result<Credential, crate::CoreError> {
    let (auth, account) = match (v.get("auth"), v.get("account")) {
        (Some(a), Some(c)) => (a, c),
        _ => (v, v), // 扁平形态：auth 与 account 字段都在顶层
    };
    let s = |obj: &serde_json::Value, key: &str| -> String {
        obj.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let mut cred = Credential {
        access_token: s(auth, "accessToken"),
        refresh_token: s(auth, "refreshToken"),
        expires_at: 0,
        domain: s(auth, "domain"),
        api_host: s(auth, "apiHost"),
        machine_id: s(auth, "machineId"),
        device_id: s(auth, "deviceId"),
        uid: s(account, "uid"),
        enterprise_id: s(account, "enterpriseId"),
        nickname: s(account, "nickname"),
    };
    if let Some(x) = auth.get("expiresAt").and_then(|x| x.as_i64()) {
        cred.expires_at = normalize_expiry(x);
    }
    // 扁平形态下 uid 可能与 auth 混在同层，兜底取顶层 uid
    if cred.uid.is_empty() {
        cred.uid = s(v, "uid");
    }
    if cred.nickname.is_empty() {
        cred.nickname = s(v, "nickname");
    }
    if cred.refresh_token.is_empty() && cred.access_token.is_empty() {
        return Err(crate::CoreError::Other(
            "凭证缺少 accessToken / refreshToken".into(),
        ));
    }
    if cred.uid.is_empty() {
        return Err(crate::CoreError::Other("凭证缺少 uid".into()));
    }
    Ok(cred)
}

/// 凭证 → 落盘 JSON：嵌套 `{auth, account}` 形态（PLAN 决策 #8，与 Go CLI / Actions 互通）
pub fn to_file_value(cred: &Credential) -> serde_json::Value {
    serde_json::json!({
        "auth": {
            "accessToken": cred.access_token,
            "refreshToken": cred.refresh_token,
            "expiresAt": cred.expires_at,
            "domain": cred.domain,
            "apiHost": cred.api_host,
            "machineId": cred.machine_id,
            "deviceId": cred.device_id
        },
        "account": {
            "uid": cred.uid,
            "enterpriseId": cred.enterprise_id,
            "nickname": cred.nickname
        }
    })
}

/// 原子写：先写 tmp 再 rename 覆盖
pub fn save_credential(data_dir: &Path, cred: &Credential) -> Result<PathBuf, crate::CoreError> {
    let dir = auths_dir(data_dir);
    fs::create_dir_all(&dir)?;
    let path = dir.join(credential_file_name(&cred.uid));
    atomic_write_json(&path, &to_file_value(cred))?;
    Ok(path)
}

/// 删除凭证文件；返回是否存在
pub fn delete_credential(data_dir: &Path, uid: &str) -> Result<bool, crate::CoreError> {
    let path = auths_dir(data_dir).join(credential_file_name(uid));
    if path.exists() {
        fs::remove_file(&path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// 列出 auths/ 下全部凭证（忽略解析失败的文件）
pub fn list_credentials(data_dir: &Path) -> Result<Vec<Credential>, crate::CoreError> {
    let dir = auths_dir(data_dir);
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut entries: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let text = match fs::read_to_string(e.path()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Ok(c) = parse_credential(&text) {
            out.push(c);
        }
    }
    Ok(out)
}

/// 原子写 JSON：tmp + rename（同目录，保证同一文件系统）
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), crate::CoreError> {
    // tmp 名带 pid + 递增序号：并发写同一凭证文件时不会互相覆盖同名 tmp
    static TMP_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.{seq}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, text)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // 清理自己的 tmp；失败分支不可能删到别人的 tmp
            let _ = fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED: &str = r#"{
      "auth": { "accessToken": "at-1", "refreshToken": "rt-1", "expiresAt": 1786858238,
                "domain": "trae.cn", "apiHost": "https://api.trae.com.cn",
                "machineId": "m1", "deviceId": "d1" },
      "account": { "uid": "u-001", "enterpriseId": "", "nickname": "甲" }
    }"#;

    const FLAT: &str = r#"{
      "accessToken": "at-2", "refreshToken": "rt-2", "expiresAt": 1786858238000,
      "uid": "u-002", "nickname": "乙"
    }"#;

    #[test]
    fn parse_nested() {
        let c = parse_credential(NESTED).unwrap();
        assert_eq!(c.access_token, "at-1");
        assert_eq!(c.refresh_token, "rt-1");
        assert_eq!(c.expires_at, 1786858238);
        assert_eq!(c.uid, "u-001");
        assert_eq!(c.nickname, "甲");
        assert_eq!(c.api_host, "https://api.trae.com.cn");
        assert_eq!(c.machine_id, "m1");
    }

    #[test]
    fn parse_flat_and_millis_expiry() {
        let c = parse_credential(FLAT).unwrap();
        assert_eq!(c.access_token, "at-2");
        assert_eq!(c.uid, "u-002");
        assert_eq!(c.nickname, "乙");
        // 毫秒归一化为秒
        assert_eq!(c.expires_at, 1786858238);
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_credential("{}").is_err());
        assert!(parse_credential("{\"accessToken\":\"x\"}").is_err()); // 无 uid
    }

    #[test]
    fn normalize_expiry_cases() {
        assert_eq!(normalize_expiry(1_786_858_238), 1_786_858_238);
        assert_eq!(normalize_expiry(1_786_858_238_000), 1_786_858_238);
        assert_eq!(normalize_expiry(1_000_000_000_000), 1_000_000_000_000); // 严格大于才除
        assert_eq!(normalize_expiry(1_000_000_000_001), 1_000_000_000);
    }

    #[test]
    fn save_list_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tscore-test-{}", uuid::Uuid::new_v4()));
        let mut c1 = parse_credential(NESTED).unwrap();
        save_credential(&dir, &c1).unwrap();
        // 同 uid 覆盖更新
        c1.nickname = "甲改".to_string();
        save_credential(&dir, &c1).unwrap();
        let mut c2 = parse_credential(FLAT).unwrap();
        c2.api_host = "https://api.trae.com.cn".into();
        save_credential(&dir, &c2).unwrap();

        let all = list_credentials(&dir).unwrap();
        assert_eq!(all.len(), 2);
        let names: Vec<_> = all.iter().map(|c| c.uid.as_str()).collect();
        assert!(names.contains(&"u-001") && names.contains(&"u-002"));

        // 覆盖后文件内容校验：落盘必须是嵌套形态
        let text = std::fs::read_to_string(
            dir.join("auths").join(credential_file_name("u-001")),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["auth"]["accessToken"], "at-1");
        assert_eq!(v["account"]["nickname"], "甲改");

        assert!(delete_credential(&dir, "u-001").unwrap());
        assert!(!delete_credential(&dir, "u-001").unwrap());
        assert_eq!(list_credentials(&dir).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flat_input_is_saved_as_nested() {
        let dir = std::env::temp_dir().join(format!("tscore-test-{}", uuid::Uuid::new_v4()));
        let c = parse_credential(FLAT).unwrap(); // 扁平输入
        save_credential(&dir, &c).unwrap();
        let text = std::fs::read_to_string(
            dir.join("auths").join(credential_file_name("u-002")),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["auth"]["accessToken"], "at-2");
        assert_eq!(v["auth"]["refreshToken"], "rt-2");
        assert_eq!(v["auth"]["expiresAt"], 1786858238);
        assert_eq!(v["account"]["uid"], "u-002");
        assert_eq!(v["account"]["nickname"], "乙");
        // 顶层不能再是扁平字段，否则等于没修
        assert!(v.get("accessToken").is_none());
        // 自己写出的嵌套文件必须能原样读回
        assert_eq!(parse_credential(&text).unwrap(), c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("tscore-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.json");
        atomic_write_json(&path, &serde_json::json!({"a":1})).unwrap();
        assert!(path.exists());
        // 确认目录内没有 tmp 残留
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
