//! Model-list probing for the per-Agent model picker.
//!
//! Two sources:
//! - [`fetch_models`]: OpenAI-compatible `GET {baseUrl}/models`. The baseUrl
//!   may point at a chat-completions endpoint; the `/chat/completions` suffix
//!   is stripped before appending `/models`.
//! - [`kimi_model_aliases`]: aliases from the `[models]` table of
//!   `~/.kimi-code/config.toml` (kimi CLI's own configured models).

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::store::agents::Agent;
use crate::store::paths::Paths;

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

/// Effective effort list: the agent's selected levels, or every known level
/// when the selection is empty (empty = unrestricted).
pub fn effective_efforts(effort_options: &[String]) -> Vec<&str> {
    if effort_options.is_empty() {
        EFFORT_LEVELS.to_vec()
    } else {
        effort_options.iter().map(String::as_str).collect()
    }
}

/// Default level for a declared model: the agent's default when it is among
/// the allowed levels, else the first allowed one.
pub(crate) fn pick_default_effort<'a>(efforts: &'a [&'a str], default: &'a str) -> &'a str {
    let d = default.trim();
    if !d.is_empty() && efforts.contains(&d) {
        d
    } else {
        efforts[0]
    }
}

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

fn model_block(
    alias: &str,
    provider_key: &str,
    model_id: &str,
    efforts: &[&str],
    default_effort: &str,
    max_context: u32,
) -> String {
    let efforts = efforts
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[models.\"{}\"]\nprovider = \"{}\"\nmodel = \"{}\"\nmax_context_size = {max_context}\ncapabilities = [ \"thinking\", \"tool_use\" ]\nsupport_efforts = [ {efforts} ]\ndefault_effort = \"{}\"",
        toml_escape(alias),
        toml_escape(provider_key),
        toml_escape(model_id),
        toml_escape(default_effort),
    )
}

/// Declare `model_id` in the kimi CLI config with the agent's allowed effort
/// levels as `support_efforts` (empty selection = every level), so the ACP
/// thinking picker offers exactly those. `default_effort` picks the initial
/// level (empty or not allowed → the first allowed one). Creates the config
/// file when missing. Idempotent.
pub fn sync_kimi_effort_model(
    model_id: &str,
    base_url: &str,
    api_key: &str,
    effort_options: &[String],
    default_effort: &str,
    max_context_k: u32,
) -> Result<(), String> {
    let path = kimi_config_path().ok_or("无法定位 kimi config.toml")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let pkey = wardex_provider_key(base_url);
    let pheader = format!("[providers.{pkey}]");
    let mheader = format!("[models.\"{}\"]", model_id);
    let text = upsert_section(&text, &pheader, &provider_block(&pkey, base_url, api_key));
    let efforts = effective_efforts(effort_options);
    let default = pick_default_effort(&efforts, default_effort);
    let max_context = if max_context_k == 0 { 256 * 1024 } else { max_context_k * 1024 };
    let text = upsert_section(
        &text,
        &mheader,
        &model_block(model_id, &pkey, model_id, &efforts, default, max_context),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Per-agent opencode.json overrides (models.rs effort sync, opencode flavor).
// Each agent's model + effort selection render into ONE isolated file under
// %AppData%/WarDex/opencode/<agentId>.json, injected via OPENCODE_CONFIG at
// spawn — so editing one agent's model/efforts never touches any other agent
// or the user's global ~/.config/opencode/opencode.json.
// ---------------------------------------------------------------------------

/// Render a per-agent opencode.json override from `agent.model` + its
/// `effort_options`. Returns "" when the agent declares no model (no
/// override; the global opencode config applies as-is).
///
/// - baseUrl on the built-in OpenCode Go endpoint → the `opencode-go`
///   provider (apiKey rides the OPENCODE_API_KEY env Wardex already sets);
/// - any other baseUrl → a `wardex-opencode-<host>` custom provider
///   (`@ai-sdk/openai-compatible`) with baseURL + apiKey inline;
/// - each allowed effort level becomes one variant
///   `{reasoningEffort, thinking: enabled}`; the default effort's options are
///   copied into the model's base `options`, so "no variant picked" behaves
///   exactly like the default and the ACP effort picker's fallback matches;
/// - top-level `model` points at the agent's model so a fresh ACP session
///   starts on it.
pub fn render_opencode_config(agent: &Agent) -> String {
    let model_id = agent.model.trim();
    if model_id.is_empty() {
        return String::new();
    }
    let base_url = agent.base_url.trim();
    let builtin_go = base_url.is_empty() || base_url.contains("opencode.ai/zen/go");
    let provider_key = if builtin_go {
        "opencode-go".to_string()
    } else {
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
        format!(
            "wardex-opencode-{}",
            if slug.is_empty() { "local" } else { slug }
        )
    };

    let efforts = effective_efforts(&agent.effort_options);
    let default_effort = pick_default_effort(&efforts, &agent.default_effort);
    let mut variants = Map::new();
    for level in &efforts {
        variants.insert(
            (*level).to_string(),
            Value::Object(Map::from_iter([
                ("reasoningEffort".into(), Value::String((*level).into())),
                (
                    "thinking".into(),
                    Value::Object(Map::from_iter([("type".into(), Value::String("enabled".into()))])),
                ),
            ])),
        );
    }
    let mut model_obj = Map::new();
    if let Some(def) = variants.get(default_effort) {
        model_obj.insert("options".to_string(), def.clone());
    }
    model_obj.insert("variants".to_string(), Value::Object(variants));

    let mut models = Map::new();
    models.insert(model_id.to_string(), Value::Object(model_obj));

    let mut prov = Map::new();
    if !builtin_go {
        let mut opts = Map::new();
        opts.insert("baseURL".to_string(), Value::String(api_root(base_url)));
        if !agent.api_key.trim().is_empty() {
            opts.insert("apiKey".to_string(), Value::String(agent.api_key.trim().to_string()));
        }
        prov.insert("npm".to_string(), Value::String("@ai-sdk/openai-compatible".to_string()));
        prov.insert("options".to_string(), Value::Object(opts));
    }
    prov.insert("models".to_string(), Value::Object(models));

    let mut provider = Map::new();
    provider.insert(provider_key.clone(), Value::Object(prov));

    let mut root = Map::new();
    root.insert("provider".to_string(), Value::Object(provider));
    root.insert("model".to_string(), Value::String(format!("{provider_key}/{model_id}")));
    // Pin opencode's DEFAULT variant to the resolved effort so the ACP effort
    // picker reports it as the current value. Without this, opencode falls back
    // to the first of the model's reasoning_options (e.g. `low`) regardless of
    // the base `options.reasoningEffort` above.
    root.insert(
        "agent".to_string(),
        Value::Object(Map::from_iter([(
            "build".to_string(),
            Value::Object(Map::from_iter([(
                "variant".to_string(),
                Value::String(default_effort.to_string()),
            )])),
        )])),
    );
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

/// Write the per-agent opencode.json override (rewrites only when the content
/// changed). Returns the path to inject via `OPENCODE_CONFIG`, or `None` when
/// the agent has no override — a stale file from an earlier edit is removed so
/// the global opencode config applies cleanly.
pub fn write_opencode_config(
    paths: &Paths,
    agent: &Agent,
) -> Result<Option<std::path::PathBuf>, String> {
    let rendered = render_opencode_config(agent);
    let path = paths.opencode_config_file_path(&agent.id);
    if rendered.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if std::fs::read_to_string(&path).unwrap_or_default() != rendered {
        std::fs::write(&path, &rendered).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    }
    Ok(Some(path))
}

/// Render a pi custom provider (provider "pi") into the models.json body.
/// pi's baseUrl does NOT ride an env var — it lives in `~/.pi/agent/models.json`
/// as a custom provider keyed by the endpoint host. Returns `""` when the agent
/// has no baseUrl/model to render (pi then uses its own ~/.pi config).
pub fn render_pi_provider(agent: &Agent) -> String {
    let base_url = agent.base_url.trim();
    let model = agent.model.trim();
    if base_url.is_empty() || model.is_empty() {
        return String::new();
    }
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
    let key = format!(
        "wardex-pi-{}",
        if slug.is_empty() { "local".to_string() } else { slug }
    );
    let mut provider = Map::new();
    provider.insert("baseUrl".to_string(), Value::String(api_root(base_url)));
    provider.insert("api".to_string(), Value::String("openai-completions".to_string()));
    if !agent.api_key.trim().is_empty() {
        provider.insert("apiKey".to_string(), Value::String(agent.api_key.trim().to_string()));
    }
    provider.insert(
        "models".to_string(),
        Value::Array(vec![Value::Object(Map::from_iter([
            ("id".to_string(), Value::String(model.to_string())),
            // pi models.json models DEFAULT to text-only
            // (coding-agent/src/core/provider-composer.ts modelFromJson:
            // `definition.input ?? ["text"]`), and a text-only model makes pi
            // strip every image — the read tool replaces the file with
            // "[Current model does not support images…]" and
            // transform-messages.ts swaps the user's pasted screenshot for
            // "(image omitted…)". Wardex already sends images unconditionally
            // (PiDriver::image_supported() == true), so the custom model must
            // declare the image modality or pictures never reach the model.
            (
                "input".to_string(),
                Value::Array(vec![Value::String("text".to_string()), Value::String("image".to_string())]),
            ),
            // pi exposes thinking levels only for reasoning-capable models
            // (ai/src/models.ts getSupportedThinkingLevels).
            ("reasoning".to_string(), Value::Bool(true)),
            // pi's openai-completions auto-detect treats unknown providers as
            // "standard OpenAI" and sends the system prompt with role
            // "developer" when reasoning=true; many compatible endpoints
            // (e.g. api.kimi.com) reject it with 400. Force "system" instead
            // (ai/src/api/openai-completions.ts convertMessages).
            (
                "compat".to_string(),
                Value::Object(Map::from_iter([(
                    "supportsDeveloperRole".to_string(),
                    Value::Bool(false),
                )])),
            ),
        ]))]),
    );
    let mut providers = Map::new();
    providers.insert(key, Value::Object(provider));
    let mut root = Map::new();
    root.insert("providers".to_string(), Value::Object(providers));
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

/// Write the rendered pi provider into `~/.pi/agent/models.json`. Merges into
/// any existing user file (pi's other providers survive); removes only the
/// wardex-pi-* keys Wardex owns. Returns the provider key, or None when the
/// agent has nothing to render.
pub fn write_pi_models(agent: &Agent) -> Result<Option<String>, String> {
    let rendered = render_pi_provider(agent);
    let Some(home) = dirs::home_dir() else {
        return Err("无法定位用户主目录".to_string());
    };
    let path = home.join(".pi").join("agent").join("models.json");
    if rendered.is_empty() {
        return Ok(None);
    }
    let mut root: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    let providers = root
        .as_object_mut()
        .expect("object")
        .entry("providers".to_string())
        .or_insert(Value::Object(Map::new()))
        .as_object_mut()
        .expect("object");
    // Strip stale wardex-pi-* keys, then insert the fresh one.
    let old_keys: Vec<String> = providers
        .keys()
        .filter(|k| k.starts_with("wardex-pi-"))
        .cloned()
        .collect();
    for k in old_keys {
        providers.remove(&k);
    }
    let fresh: Value = serde_json::from_str(&rendered).map_err(|e| e.to_string())?;
    let fresh_key = fresh["providers"]
        .as_object()
        .and_then(|o| o.keys().next())
        .cloned()
        .ok_or("渲染失败")?;
    let fresh_providers = fresh["providers"].as_object().cloned().unwrap_or_default();
    for (k, v) in fresh_providers {
        providers.insert(k, v);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap_or_default())
        .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(Some(fresh_key))
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
    fn provider_key_slugs_host() {
        assert_eq!(
            wardex_provider_key("https://opencode.ai/zen/go/v1/chat/completions"),
            "wardex-opencode-ai"
        );
        assert_eq!(wardex_provider_key(""), "wardex-local");
    }

    #[test]
    fn effective_efforts_fallback_to_all_levels() {
        assert_eq!(
            effective_efforts(&[]),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(effective_efforts(&["high".to_string(), "low".to_string()]), vec!["high", "low"]);
    }

    #[test]
    fn pick_default_effort_prefers_allowed() {
        let all = ["low", "medium", "high"];
        assert_eq!(pick_default_effort(&all, "medium"), "medium");
        assert_eq!(pick_default_effort(&all, ""), "low");
        // a default outside the allowed set falls back to the first
        assert_eq!(pick_default_effort(&["low".to_string()].iter().map(|s| s.as_str()).collect::<Vec<_>>(), "max"), "low");
    }

    /// Serializes the tests that override the process-wide KIMI_CODE_HOME env
    /// var (they would race otherwise).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn agent_with_efforts(effort_options: Vec<String>) -> Agent {
        Agent {
            id: "agent-1".to_string(),
            model: "deepseek-v4-flash".to_string(),
            base_url: "https://opencode.ai/zen/go/v1".to_string(),
            effort_options,
            ..Default::default()
        }
    }

    #[test]
    fn render_pi_provider_disables_developer_role() {
        let agent = Agent {
            id: "agent-1".to_string(),
            model: "k3-256k".to_string(),
            base_url: "https://api.kimi.com/coding/v1".to_string(),
            ..Default::default()
        };
        let rendered = render_pi_provider(&agent);
        let v: Value = serde_json::from_str(&rendered).unwrap();
        let provider = &v["providers"]["wardex-pi-api-kimi-com"];
        assert_eq!(provider["baseUrl"], "https://api.kimi.com/coding/v1");
        let model = &provider["models"][0];
        assert_eq!(model["reasoning"], true);
        // kimi's endpoint 400s on role "developer"; force "system"
        assert_eq!(model["compat"]["supportsDeveloperRole"], false);
    }

    /// pi defaults a models.json model to text-only, which makes it strip every
    /// image (read tool + user attachments). The declared modality must
    /// include "image" or screenshots never reach the model.
    #[test]
    fn render_pi_provider_declares_image_input() {
        let agent = Agent {
            id: "agent-1".to_string(),
            model: "step-5-preview".to_string(),
            base_url: "https://api.stepfun.com/step_plan/v1".to_string(),
            ..Default::default()
        };
        let rendered = render_pi_provider(&agent);
        let v: Value = serde_json::from_str(&rendered).unwrap();
        let model = &v["providers"]["wardex-pi-api-stepfun-com"]["models"][0];
        assert_eq!(
            model["input"],
            serde_json::json!(["text", "image"]),
            "text-only is pi's default and drops every image"
        );
        assert_eq!(model["id"], "step-5-preview");
    }

    #[test]
    fn render_opencode_no_model_yields_nothing() {
        let agent = Agent {
            id: "agent-1".to_string(),
            model: "".to_string(),
            ..Default::default()
        };
        assert_eq!(render_opencode_config(&agent), "");
    }

    #[test]
    fn render_opencode_builtin_go_provider() {
        // Selected levels drive the variants (default = max, the first)
        let agent = agent_with_efforts(vec![
            "max".to_string(),
            "high".to_string(),
            "low".to_string(),
        ]);
        let out = render_opencode_config(&agent);
        let doc: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(doc["model"], "opencode-go/deepseek-v4-flash");
        let model = &doc["provider"]["opencode-go"]["models"]["deepseek-v4-flash"];
        assert!(model.get("options").is_some());
        assert_eq!(model["options"]["reasoningEffort"], "max");
        assert_eq!(model["options"]["thinking"]["type"], "enabled");
        assert_eq!(model["variants"]["high"]["reasoningEffort"], "high");
        // only the selected levels exist — no off variant, no xhigh
        let variants = model["variants"].as_object().unwrap();
        assert_eq!(variants.len(), 3);
        assert!(variants.get("off").is_none());
        assert!(variants.get("xhigh").is_none());
        // max is the default (agent.default_effort is empty -> first level)
        let first = variants.keys().next().unwrap();
        assert_eq!(first, "max");
        // the resolved default is also pinned as opencode's build-agent variant
        assert_eq!(doc["agent"]["build"]["variant"], "max");
    }

    #[test]
    fn render_opencode_empty_efforts_falls_back_to_all() {
        let agent = agent_with_efforts(vec![]);
        let doc: Value = serde_json::from_str(&render_opencode_config(&agent)).expect("valid json");
        let variants = doc["provider"]["opencode-go"]["models"]["deepseek-v4-flash"]["variants"]
            .as_object()
            .unwrap();
        assert_eq!(variants.len(), EFFORT_LEVELS.len());
        assert!(variants.contains_key("xhigh"));
    }

    #[test]
    fn render_opencode_custom_provider_inlines_credentials() {
        let agent = Agent {
            id: "agent-2".to_string(),
            model: "glm-5".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            effort_options: vec!["high".to_string()],
            ..Default::default()
        };
        let doc: Value = serde_json::from_str(&render_opencode_config(&agent)).expect("valid json");
        let prov = &doc["provider"]["wardex-opencode-api-example-com"];
        assert_eq!(prov["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(prov["options"]["baseURL"], "https://api.example.com/v1");
        assert_eq!(prov["options"]["apiKey"], "sk-test");
        assert_eq!(doc["model"], "wardex-opencode-api-example-com/glm-5");
        // default_effort empty -> the single selected level is the default
        assert_eq!(
            doc["provider"]["wardex-opencode-api-example-com"]["models"]["glm-5"]["options"]
                ["reasoningEffort"],
            "high"
        );
    }

    #[test]
    fn write_opencode_config_writes_and_prunes_stale_file() {
        use crate::store::paths::Paths;
        let dir = std::env::temp_dir().join(format!("wardex-opencode-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths::new(dir.clone());
        let agent = Agent {
            id: "agent-1".to_string(),
            model: "".to_string(),
            ..Default::default()
        };
        // no model -> no file, returns None
        let r = write_opencode_config(&paths, &agent).unwrap();
        assert!(r.is_none());
        // with a model -> file written, path returned
        let mut agent = agent;
        agent.model = "deepseek-v4-flash".to_string();
        let r = write_opencode_config(&paths, &agent).unwrap().expect("path");
        assert!(r.exists());
        assert!(r.to_string_lossy().contains("agent-1.json"));
        // clearing the model removes the stale file
        agent.model = "".to_string();
        let r = write_opencode_config(&paths, &agent).unwrap();
        assert!(r.is_none());
        assert!(!paths.opencode_config_file_path("agent-1").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_kimi_effort_model_writes_selected_efforts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("wardex-effort-sync-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Safety: test-local override of the config path root.
        unsafe { std::env::set_var("KIMI_CODE_HOME", &dir) };
        sync_kimi_effort_model(
            "deepseek-v4-flash",
            "https://x.test/v1",
            "sk-k",
            &["high".to_string(), "low".to_string()],
            "",
            256,
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(text.contains("[providers.wardex-x-test]"));
        assert!(text.contains("[models.\"deepseek-v4-flash\"]"));
        assert!(text.contains("support_efforts = [ \"high\", \"low\" ]"));
        assert!(text.contains("default_effort = \"high\""));
        assert!(text.contains("max_context_size = 262144"));
        // empty selection = every level, default falls back to the first
        sync_kimi_effort_model(
            "deepseek-v4-flash",
            "https://x.test/v1",
            "sk-k",
            &[],
            "",
            256,
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(text.contains(&format!(
            "support_efforts = [ {} ]",
            EFFORT_LEVELS
                .iter()
                .map(|e| format!("\"{e}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )));
        // a default not among the selected levels falls back to the first
        sync_kimi_effort_model(
            "deepseek-v4-flash",
            "https://x.test/v1",
            "sk-k",
            &["low".to_string()],
            "max",
            256,
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(text.contains("support_efforts = [ \"low\" ]"));
        assert!(text.contains("default_effort = \"low\""));
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
            &["high", "low"],
            "high",
            262144,
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
            Some(2)
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
