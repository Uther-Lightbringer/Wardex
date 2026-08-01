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
    let Some(path) = kimi_config_path() else { return Vec::new() };
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

// ---------------------------------------------------------------------------
// Effort-level sync: declare a custom-baseUrl model in the kimi CLI config so
// its ACP thinking picker shows real effort levels instead of boolean on/off.
// Sections are patched at TEXT level (never a full-document TOML rewrite —
// that would drop the user's comments and formatting).
// ---------------------------------------------------------------------------

/// Thinking effort levels offered in the config UI and declared as
/// `support_efforts`. Served to the frontend via the effort_options command
/// (red line C3: no hardcoded lists in the UI).
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn kimi_config_path() -> Option<std::path::PathBuf> {
    if let Ok(home) = std::env::var("KIMI_CODE_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Some(std::path::PathBuf::from(home).join("config.toml"));
        }
    }
    dirs::home_dir().map(|h| h.join(".kimi-code").join("config.toml"))
}

/// Provider key for a baseUrl: wardex-<host slug>, e.g. wardex-opencode-ai.
/// Empty baseUrl rides the built-in kimi endpoint under a fixed key.
fn wardex_provider_key(base_url: &str) -> String {
    let host = api_root(base_url)
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    let slug: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "wardex-local".to_string()
    } else {
        format!("wardex-{slug}")
    }
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Replace the section whose header line is exactly `header` with `block`
/// (header included), or append it when absent. Everything else in `text`
/// stays byte-identical.
fn upsert_section(text: &str, header: &str, block: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut found = false;
    let mut skipping = false;
    for line in text.lines() {
        let t = line.trim();
        if t == header {
            skipping = true;
            if !found {
                out.push(block.to_string());
                found = true;
            }
            continue;
        }
        if skipping && t.starts_with('[') {
            skipping = false;
        }
        if !skipping {
            out.push(line.to_string());
        }
    }
    if !found {
        if !out.is_empty() {
            out.push(String::new()); // blank line before the appended section
        }
        out.push(block.to_string());
    }
    let mut result = out.join("\n");
    result.push('\n');
    result
}

/// Remove the section with the exact `header` line (no-op when absent).
fn remove_section(text: &str, header: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let t = line.trim();
        if t == header {
            skipping = true;
            continue;
        }
        if skipping && t.starts_with('[') {
            skipping = false;
        }
        if !skipping {
            out.push(line);
        }
    }
    let mut result = out.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

fn provider_block(key: &str, base_url: &str, api_key: &str) -> String {
    let mut lines = vec![format!("[providers.{key}]")];
    if base_url.trim().is_empty() {
        lines.push("type = \"kimi\"".to_string());
    } else {
        lines.push("type = \"openai\"".to_string());
        lines.push(format!("base_url = \"{}\"", toml_escape(&api_root(base_url))));
    }
    if !api_key.trim().is_empty() {
        lines.push(format!("api_key = \"{}\"", toml_escape(api_key.trim())));
    }
    lines.join("\n")
}

fn model_block(alias: &str, provider_key: &str, model_id: &str, default_effort: &str) -> String {
    let efforts = EFFORT_LEVELS
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[models.\"{}\"]\nprovider = \"{}\"\nmodel = \"{}\"\nmax_context_size = 262144\ncapabilities = [ \"thinking\", \"tool_use\" ]\nsupport_efforts = [ {} ]\ndefault_effort = \"{}\"",
        toml_escape(alias),
        toml_escape(provider_key),
        toml_escape(model_id),
        efforts,
        toml_escape(default_effort),
    )
}

/// Declare `model_id` in the kimi CLI config with full effort levels.
/// Creates the config file when missing. Idempotent.
pub fn sync_kimi_effort_model(
    model_id: &str,
    base_url: &str,
    api_key: &str,
    default_effort: &str,
) -> Result<(), String> {
    let path = kimi_config_path().ok_or("无法定位 kimi config.toml")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let pkey = wardex_provider_key(base_url);
    let pheader = format!("[providers.{pkey}]");
    let mheader = format!("[models.\"{}\"]", model_id);
    let text = upsert_section(&text, &pheader, &provider_block(&pkey, base_url, api_key));
    let text = upsert_section(
        &text,
        &mheader,
        &model_block(model_id, &pkey, model_id, default_effort),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// Remove the model section sync_kimi_effort_model wrote (no-op when
/// absent). The shared wardex-<host> provider section is left in place: it is
/// harmless alone and other agents' models may still reference it.
pub fn remove_kimi_effort_model(model_id: &str) -> Result<(), String> {
    let Some(path) = kimi_config_path() else { return Ok(()) };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mheader = format!("[models.\"{}\"]", model_id);
    let text = remove_section(&text, &mheader);
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Bulk sync (配置页「刷新」): declare EVERY model of an agent's endpoint in
// config.toml, scoped to that agent. The provider section is per-agent
// ([providers.wardex-agent-<id>]) so apiKey/baseUrl of different agents never
// mix; model sections that referenced this agent's provider but fell out of
// the latest /models list are removed. Effort lines (support_efforts /
// default_effort, written via the 默认思考强度 path) survive the rewrite.
// ---------------------------------------------------------------------------

/// Per-agent provider key (TOML bare-key safe: agent ids are alphanumeric).
fn wardex_agent_provider_key(agent_id: &str) -> String {
    format!("wardex-agent-{}", agent_id.trim())
}

/// Collect the `support_efforts` / `default_effort` lines of an existing
/// model section ("" when absent).
fn extract_effort_lines(text: &str, header: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_sec = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_sec = t == header;
            continue;
        }
        if in_sec && (t.starts_with("support_efforts") || t.starts_with("default_effort")) {
            out.push(t.to_string());
        }
    }
    out.join("\n")
}

fn agent_model_block(alias: &str, provider_key: &str, model_id: &str, max_context: u32, effort_lines: &str) -> String {
    let mut lines = vec![
        format!("[models.\"{}\"]", toml_escape(alias)),
        format!("provider = \"{}\"", toml_escape(provider_key)),
        format!("model = \"{}\"", toml_escape(model_id)),
        format!("max_context_size = {max_context}"),
        "capabilities = [ \"thinking\", \"tool_use\" ]".to_string(),
    ];
    if !effort_lines.is_empty() {
        lines.push(effort_lines.to_string());
    }
    lines.join("\n")
}

/// Headers of model sections whose provider is `provider_key`, plus the
/// alias of each, parsed at text level.
fn model_sections_for_provider<'a>(text: &'a str, provider_key: &str) -> Vec<(String, String)> {
    let provider_line = format!("provider = \"{provider_key}\"");
    let mut out: Vec<(String, String)> = Vec::new();
    let mut cur_header = String::new();
    let mut cur_has_provider = false;
    let mut flush = |header: &str, has: bool, out: &mut Vec<(String, String)>| {
        if has {
            if let Some(alias) = header
                .strip_prefix("[models.\"")
                .and_then(|h| h.strip_suffix("\"]"))
            {
                out.push((header.to_string(), alias.to_string()));
            }
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            flush(&cur_header, cur_has_provider, &mut out);
            cur_header = t.to_string();
            cur_has_provider = false;
            continue;
        }
        if t == provider_line {
            cur_has_provider = true;
        }
    }
    flush(&cur_header, cur_has_provider, &mut out);
    out
}

/// Rewrite an agent's whole namespace in config.toml from a fresh /models
/// list. `max_context_k` is in K (1024 tokens); 0 falls back to 256K.
/// Returns the number of model aliases written.
pub fn sync_agent_models(
    agent_id: &str,
    base_url: &str,
    api_key: &str,
    model_ids: &[String],
    max_context_k: u32,
) -> Result<usize, String> {
    if agent_id.trim().is_empty() {
        return Err("Agent 尚未保存，无法同步".to_string());
    }
    let path = kimi_config_path().ok_or("无法定位 kimi config.toml")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let pkey = wardex_agent_provider_key(agent_id);
    let max_context = if max_context_k == 0 { 256 * 1024 } else { max_context_k * 1024 };

    // Drop this agent's aliases that the endpoint no longer lists.
    let keep: std::collections::HashSet<&str> = model_ids.iter().map(String::as_str).collect();
    let mut text = text;
    for (header, alias) in model_sections_for_provider(&text, &pkey) {
        if !keep.contains(alias.as_str()) {
            text = remove_section(&text, &header);
        }
    }

    let pheader = format!("[providers.{pkey}]");
    text = upsert_section(&text, &pheader, &provider_block(&pkey, base_url, api_key));

    for id in model_ids {
        let mheader = format!("[models.\"{}\"]", id);
        // Preserve effort lines the user configured via 默认思考强度.
        let efforts = extract_effort_lines(&text, &mheader);
        text = upsert_section(
            &text,
            &mheader,
            &agent_model_block(id, &pkey, id, max_context, &efforts),
        );
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(model_ids.len())
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

    #[test]
    fn upsert_appends_then_replaces_in_place() {
        let base = "# 用户注释\ndefault_model = \"kimi-code/k3\"\n\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\n";
        let block = "[providers.wardex-opencode-ai]\ntype = \"openai\"\nbase_url = \"https://opencode.ai/zen/go/v1\"";
        // append: original text untouched, block at the end
        let t1 = upsert_section(base, "[providers.wardex-opencode-ai]", block);
        assert!(t1.starts_with(base));
        assert!(t1.contains(block));
        // replace: comment + other sections preserved, no duplicate header
        let block2 = "[providers.wardex-opencode-ai]\ntype = \"openai\"\nbase_url = \"https://other/v1\"";
        let t2 = upsert_section(&t1, "[providers.wardex-opencode-ai]", block2);
        assert!(t2.contains("# 用户注释"));
        assert!(t2.contains("[providers.\"managed:kimi-code\"]"));
        assert!(t2.contains("https://other/v1"));
        assert!(!t2.contains("https://opencode.ai/zen/go/v1"));
        assert_eq!(t2.matches("[providers.wardex-opencode-ai]").count(), 1);
    }

    #[test]
    fn remove_section_deletes_only_target() {
        let text = "[a]\nx = 1\n\n[models.\"deepseek-v4-flash\"]\nmodel = \"deepseek-v4-flash\"\n\n[b]\ny = 2\n";
        let out = remove_section(text, "[models.\"deepseek-v4-flash\"]");
        assert!(!out.contains("deepseek"));
        assert!(out.contains("[a]\nx = 1"));
        assert!(out.contains("[b]\ny = 2"));
        // absent header is a no-op
        assert_eq!(remove_section(text, "[nope]"), text);
    }

    #[test]
    fn provider_key_slugs_host() {
        assert_eq!(
            wardex_provider_key("https://opencode.ai/zen/go/v1/chat/completions"),
            "wardex-opencode-ai"
        );
        assert_eq!(wardex_provider_key(""), "wardex-local");
    }

    #[test]
    fn sections_for_provider_and_effort_extraction() {
        let text = "[models.\"glm-5\"]\nprovider = \"wardex-agent-a1\"\nmodel = \"glm-5\"\nsupport_efforts = [ \"low\", \"high\" ]\ndefault_effort = \"high\"\n\n[models.\"qwen3.5-plus\"]\nprovider = \"wardex-agent-b2\"\nmodel = \"qwen3.5-plus\"\n\n[models.\"deepseek-v4-flash\"]\nprovider = \"wardex-agent-a1\"\nmodel = \"deepseek-v4-flash\"\n";
        let secs = model_sections_for_provider(text, "wardex-agent-a1");
        assert_eq!(
            secs.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>(),
            vec!["glm-5", "deepseek-v4-flash"]
        );
        let efforts = extract_effort_lines(text, "[models.\"glm-5\"]");
        assert!(efforts.contains("support_efforts"));
        assert!(efforts.contains("default_effort = \"high\""));
        assert_eq!(extract_effort_lines(text, "[models.\"qwen3.5-plus\"]"), "");
    }

    #[test]
    fn sync_agent_models_writes_namespace_and_prunes() {
        let dir = std::env::temp_dir().join(format!("wardex-sync-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Safety: test-local override of the config path root.
        unsafe { std::env::set_var("KIMI_CODE_HOME", &dir) };
        let ids = vec!["glm-5".to_string(), "deepseek-v4-flash".to_string()];
        let n = sync_agent_models("a1", "https://x.test/v1", "sk-k", &ids, 256).unwrap();
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(text.contains("[providers.wardex-agent-a1]"));
        assert!(text.contains("max_context_size = 262144"));
        assert!(text.contains("[models.\"glm-5\"]"));
        // second sync with a shorter list prunes glm-5
        let n = sync_agent_models("a1", "https://x.test/v1", "sk-k", &ids[1..], 256).unwrap();
        assert_eq!(n, 1);
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(!text.contains("[models.\"glm-5\"]"));
        assert!(text.contains("[models.\"deepseek-v4-flash\"]"));
        unsafe { std::env::remove_var("KIMI_CODE_HOME") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_blocks_parse_as_toml() {
        let p = provider_block("wardex-opencode-ai", "https://opencode.ai/zen/go/v1", "sk-test");
        let m = model_block(
            "deepseek-v4-flash",
            "wardex-opencode-ai",
            "deepseek-v4-flash",
            "high",
        );
        let doc = format!("{p}\n\n{m}\n").parse::<toml::Value>().expect("valid toml");
        let models = doc.get("models").and_then(|m| m.as_table()).unwrap();
        let entry = models.get("deepseek-v4-flash").unwrap();
        assert_eq!(
            entry.get("default_effort").and_then(|v| v.as_str()),
            Some("high")
        );
        assert_eq!(
            entry
                .get("support_efforts")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(EFFORT_LEVELS.len())
        );
        let providers = doc.get("providers").and_then(|p| p.as_table()).unwrap();
        let prov = providers.get("wardex-opencode-ai").unwrap();
        assert_eq!(prov.get("type").and_then(|v| v.as_str()), Some("openai"));
        assert_eq!(
            prov.get("base_url").and_then(|v| v.as_str()),
            Some("https://opencode.ai/zen/go/v1")
        );
    }
}
