//! Model-list probing for the per-Agent model picker.
//!
//! Two sources:
//! - [`fetch_models`]: OpenAI-compatible `GET {baseUrl}/models`. The baseUrl
//!   may point at a chat-completions endpoint; the `/chat/completions` suffix
//!   is stripped before appending `/models`.
//! - [`kimi_model_aliases`]: aliases from the `[models]` table of
//!   `~/.kimi-code/config.toml` (kimi CLI's own configured models).

use serde::Deserialize;

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// Normalize a base URL to its API root: trim trailing slashes and a
/// trailing `/chat/completions` suffix so `{root}/models` can be appended.
pub fn api_root(base_url: &str) -> String {
    let mut s = base_url.trim().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/models"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.trim_end_matches('/').to_string();
        }
    }
    s
}

/// GET `{baseUrl}/models` and return the model ids. `api_key`, when
/// non-empty, is sent as a Bearer token. Errors come back as strings for the
/// frontend to show verbatim.
pub fn fetch_models(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    let root = api_root(base_url);
    if root.is_empty() {
        return Err("baseUrl 为空".to_string());
    }
    let url = format!("{root}/models");
    let mut req = ureq::get(&url).timeout(std::time::Duration::from_secs(15));
    if !api_key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", api_key.trim()));
    }
    let resp = req.call().map_err(|e| format!("请求 {url} 失败: {e}"))?;
    let parsed: ModelsResponse = resp
        .into_json()
        .map_err(|e| format!("解析 {url} 响应失败: {e}"))?;
    let mut ids: Vec<String> = parsed.data.into_iter().map(|m| m.id).collect();
    ids.sort();
    Ok(ids)
}

/// Model aliases from the `[models]` table of `~/.kimi-code/config.toml`.
/// Missing file / missing table yields an empty list (not an error).
pub fn kimi_model_aliases() -> Vec<String> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let path = home.join(".kimi-code").join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(doc) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut aliases: Vec<String> = doc
        .get("models")
        .and_then(|m| m.as_table())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    aliases.sort();
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_root_strips_suffixes() {
        assert_eq!(api_root("https://api.deepseek.com/v1"), "https://api.deepseek.com/v1");
        assert_eq!(api_root("https://api.deepseek.com/v1/"), "https://api.deepseek.com/v1");
        assert_eq!(
            api_root("https://opencode.ai/zen/go/v1/chat/completions"),
            "https://opencode.ai/zen/go/v1"
        );
        assert_eq!(api_root("  https://x.test/v1  "), "https://x.test/v1");
    }
}
