//! OpenAI Codex (ChatGPT 订阅) device-code OAuth → ~/.pi/agent/auth.json.
//!
//! Ported from third_party/pi packages/ai/src/auth/oauth/openai-codex.ts:
//! pi only exposes login through its interactive TUI (`/login`), which WarDex
//! never runs — sessions go through `pi --mode rpc`. So the GUI reimplements
//! the flow itself and writes the credential into the same auth.json pi reads.
//! The credential is GLOBAL (one slot per provider): every pi agent that picks
//! an openai-codex/* model shares it.

use base64::Engine;
use serde_json::{Map, Value};

use crate::store::json::write_value_atomic;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
const PROVIDER_ID: &str = "openai-codex";
/// JWT claim carrying the ChatGPT account id (pi uses it for the
/// `chatgpt-account-id` request header).
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

fn auth_json_path() -> Result<std::path::PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".pi").join("agent").join("auth.json"))
        .ok_or_else(|| "无法定位用户主目录".to_string())
}

fn read_auth_json() -> Result<Map<String, Value>, String> {
    let path = auth_json_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str::<Value>(&s)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?
            .as_object()
            .cloned()
            .ok_or_else(|| format!("{} 不是 JSON 对象", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

fn write_auth_json(root: &Map<String, Value>) -> Result<(), String> {
    let path = auth_json_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_value_atomic(&path, &Value::Object(root.clone())).map_err(|e| e.to_string())?;
    // Match pi's AUTH_FILE_WRITE_OPTIONS (mode 0600 — holds bearer tokens).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Decode the JWT payload segment (no signature check — the token is handed
/// straight back to OpenAI; we only need the account id for display/storage).
fn jwt_account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    let json: Value = serde_json::from_slice(&bytes).ok()?;
    json.get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

struct TokenSet {
    access: String,
    refresh: String,
    expires_ms: i64,
}

fn read_token_body(body: &str, op: &str) -> Result<TokenSet, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| format!("Codex token {op} 响应不是 JSON: {e}"))?;
    let access = json["access_token"].as_str().unwrap_or("");
    let refresh = json["refresh_token"].as_str().unwrap_or("");
    let expires_in = json["expires_in"].as_i64().unwrap_or(0);
    if access.is_empty() || refresh.is_empty() || expires_in <= 0 {
        return Err(format!("Codex token {op} 响应缺字段: {body}"));
    }
    Ok(TokenSet {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires_ms: chrono::Utc::now().timestamp_millis() + expires_in * 1000,
    })
}

fn exchange_code(code: &str, verifier: &str) -> Result<TokenSet, String> {
    let body = format!(
        "grant_type=authorization_code&client_id={CLIENT_ID}&code={}&code_verifier={}&redirect_uri={}",
        url_encode(code),
        url_encode(verifier),
        url_encode(DEVICE_REDIRECT_URI),
    );
    let resp = ureq::post(TOKEN_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);
    let resp = resp.map_err(|e| http_err("Codex token exchange", e))?;
    read_token_body(&resp.into_string().map_err(|e| e.to_string())?, "exchange")
}

fn url_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn http_err(op: &str, e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let text = resp.into_string().unwrap_or_default();
            format!("{op} 失败 (HTTP {code}){}", if text.is_empty() { String::new() } else { format!(": {text}") })
        }
        ureq::Error::Transport(t) => format!("{op} 网络错误: {t}"),
    }
}

fn store_codex_credential(tokens: &TokenSet, account_id: Option<&str>) -> Result<(), String> {
    let mut root = read_auth_json()?;
    let mut cred = Map::new();
    cred.insert("type".to_string(), Value::String("oauth".to_string()));
    cred.insert("access".to_string(), Value::String(tokens.access.clone()));
    cred.insert("refresh".to_string(), Value::String(tokens.refresh.clone()));
    cred.insert("expires".to_string(), Value::from(tokens.expires_ms));
    if let Some(id) = account_id {
        cred.insert("accountId".to_string(), Value::String(id.to_string()));
    }
    root.insert(PROVIDER_ID.to_string(), Value::Object(cred));
    write_auth_json(&root)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Step 1: request a device code. Returns the user_code the user types on the
/// verification page plus the opaque device_auth_id the poll step needs.
pub fn login_start() -> Result<Value, String> {
    let resp = ureq::post(DEVICE_USER_CODE_URL)
        .send_json(ureq::json!({ "client_id": CLIENT_ID }));
    let resp = resp.map_err(|e| http_err("Codex device code 请求", e))?;
    let body = resp.into_string().map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_str(&body).map_err(|e| format!("device code 响应不是 JSON: {e}"))?;
    let device_auth_id = json["device_auth_id"].as_str().unwrap_or("");
    let user_code = json["user_code"].as_str().unwrap_or("");
    let interval = json["interval"].as_i64()
        .or_else(|| json["interval"].as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or(5);
    if device_auth_id.is_empty() || user_code.is_empty() || interval <= 0 {
        return Err(format!("device code 响应缺字段: {body}"));
    }
    Ok(serde_json::json!({
        "deviceAuthId": device_auth_id,
        "userCode": user_code,
        "verificationUrl": VERIFICATION_URI,
        "intervalSecs": interval,
    }))
}

/// Step 2: ONE poll attempt. The frontend re-calls this every `intervalSecs`.
/// Returns {"status":"pending"|"slow_down"} or, once the user authorizes,
/// {"status":"done","accountId"} after exchanging the code and writing
/// ~/.pi/agent/auth.json.
pub fn login_poll(device_auth_id: &str, user_code: &str) -> Result<Value, String> {
    let resp = ureq::post(DEVICE_TOKEN_URL)
        .send_json(ureq::json!({
            "device_auth_id": device_auth_id,
            "user_code": user_code,
        }));
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(403, _)) | Err(ureq::Error::Status(404, _)) => {
            return Ok(serde_json::json!({ "status": "pending" }));
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let err_code = serde_json::from_str::<Value>(&text).ok()
                .and_then(|j| match j.get("error") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Object(o)) => o.get("code").and_then(|c| c.as_str()).map(|s| s.to_string()),
                    _ => None,
                });
            return match err_code.as_deref() {
                Some("deviceauth_authorization_pending") => Ok(serde_json::json!({ "status": "pending" })),
                Some("slow_down") => Ok(serde_json::json!({ "status": "slow_down" })),
                _ => Err(format!("Codex device auth 失败 (HTTP {code}){}", if text.is_empty() { String::new() } else { format!(": {text}") })),
            };
        }
        Err(ureq::Error::Transport(t)) => return Err(format!("Codex device auth 网络错误: {t}")),
    };
    let body = resp.into_string().map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_str(&body).map_err(|e| format!("device auth 响应不是 JSON: {e}"))?;
    let code = json["authorization_code"].as_str().unwrap_or("");
    let verifier = json["code_verifier"].as_str().unwrap_or("");
    if code.is_empty() || verifier.is_empty() {
        return Err(format!("device auth 响应缺字段: {body}"));
    }
    let tokens = exchange_code(code, verifier)?;
    let account_id = jwt_account_id(&tokens.access)
        .ok_or_else(|| "无法从 access token 中解析 ChatGPT 账号 id".to_string())?;
    store_codex_credential(&tokens, Some(&account_id))?;
    Ok(serde_json::json!({ "status": "done", "accountId": account_id }))
}

/// Login state for the config page. `loggedIn` means a usable oauth credential
/// exists — an expired `expires` is NOT logged-out (pi refreshes silently).
pub fn auth_status() -> Result<Value, String> {
    let root = read_auth_json()?;
    let cred = root.get(PROVIDER_ID);
    let logged_in = cred
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        == Some("oauth")
        && cred.and_then(|c| c.get("access")).and_then(|a| a.as_str()).map(|s| !s.is_empty()) == Some(true);
    let account_id = cred
        .and_then(|c| c.get("accountId"))
        .and_then(|a| a.as_str())
        .map(|s| s.to_string());
    Ok(serde_json::json!({ "loggedIn": logged_in, "accountId": account_id }))
}

/// Remove the openai-codex credential (leaves every other provider alone).
pub fn logout() -> Result<(), String> {
    let mut root = read_auth_json()?;
    root.remove(PROVIDER_ID);
    write_auth_json(&root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_account_id_reads_claim() {
        // header.payload.signature — payload {"https://api.openai.com/auth":{"chatgpt_account_id":"acct-42"}}
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-42"}}"#);
        let token = format!("x.{payload}.y");
        assert_eq!(jwt_account_id(&token), Some("acct-42".to_string()));
        assert_eq!(jwt_account_id("a.b.c"), None);
        assert_eq!(jwt_account_id(""), None);
    }

    #[test]
    fn token_body_parses() {
        let t = read_token_body(
            r#"{"access_token":"a","refresh_token":"r","expires_in":3600}"#,
            "exchange",
        )
        .unwrap();
        assert_eq!(t.access, "a");
        assert_eq!(t.refresh, "r");
        assert!(t.expires_ms > 0);
        assert!(read_token_body(r#"{"access_token":"a"}"#, "exchange").is_err());
    }
}
