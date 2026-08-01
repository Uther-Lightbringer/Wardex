// agents/ persistence: agents/index.json (list order + default pointer) and
// agents/<id>.json (one file per agent) — data-formats.md §5.
//
// Compatibility rules preserved:
//   - Orphan pickup: every agents/*.json besides index.json is an agent;
//     ids missing from the index are appended to the list tail.
//   - `isDefault` in agent files is a redundant snapshot; the index's
//     `defaultAgentId` is authoritative. When it is empty and agents exist,
//     the first chat-capable (currently: provider == "kimi") agent is picked.
//   - File missing `cliPath` → backfilled "kimi" (old fromJson default);
//     a newly created agent holds "" in memory instead.
//   - apiKey update guard: an empty string or one containing '*' keeps the
//     old value (UI sends back the masked key).

use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::store::json::{de_ms_i64, now_ms, write_value_atomic, JsonError};
use crate::store::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum AgentsError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
    #[error("Agent 不存在")]
    NotFound,
    #[error("该 Provider 不支持对话，无法设为默认")]
    DefaultUnsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    #[serde(rename = "apiKey")]
    pub api_key: String,
    #[serde(rename = "avatarPath")]
    pub avatar_path: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "cliPath", default = "default_cli_path")]
    pub cli_path: String,
    #[serde(rename = "createdAt", deserialize_with = "de_ms_i64")]
    pub created_at: i64,
    /// Per-agent default thinking effort ("" = follow CLI). Non-empty values
    /// make WarDex declare the model in ~/.kimi-code/config.toml with
    /// support_efforts so the ACP thinking picker shows real levels.
    #[serde(rename = "defaultEffort")]
    pub default_effort: String,
    /// Context size (in K = 1024 tokens) stamped into the config.toml model
    /// aliases that the 刷新-button bulk sync writes for this agent.
    /// 0 = fallback 256K.
    #[serde(rename = "maxContextK")]
    pub max_context_k: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "extraArgs")]
    pub extra_args: String,
    pub id: String,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
    /// JSON array TEXT (not a nested object) passed through to ACP session/new.
    #[serde(rename = "mcpServers")]
    pub mcp_servers: String,
    pub model: String,
    pub name: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(rename = "updatedAt", deserialize_with = "de_ms_i64")]
    pub updated_at: i64,
    /// Unknown keys survive a load/save round trip (cross-version safety).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_provider() -> String {
    "kimi".to_string()
}

fn default_cli_path() -> String {
    "kimi".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            avatar_path: String::new(),
            base_url: String::new(),
            cli_path: default_cli_path(),
            created_at: 0,
            default_effort: String::new(),
            max_context_k: 0,
            enabled: true,
            extra_args: String::new(),
            id: String::new(),
            is_default: false,
            mcp_servers: String::new(),
            model: String::new(),
            name: String::new(),
            provider: default_provider(),
            updated_at: 0,
            extra: Map::new(),
        }
    }
}

/// Patch for update_agent; `None` = field not touched (old QVariantMap
/// `contains(key)` semantics).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: Option<String>,
    #[serde(rename = "defaultEffort")]
    pub default_effort: Option<String>,
    #[serde(rename = "maxContextK")]
    pub max_context_k: Option<u32>,
    #[serde(rename = "cliPath")]
    pub cli_path: Option<String>,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "extraArgs")]
    pub extra_args: Option<String>,
    #[serde(rename = "mcpServers")]
    pub mcp_servers: Option<String>,
    #[serde(rename = "avatarPath")]
    pub avatar_path: Option<String>,
    pub enabled: Option<bool>,
}

/// Masked apiKey for display (maskKey): size<=8 → "********", else
/// left(3)+"****"+right(4).
pub fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let n = key.chars().count();
    if n <= 8 {
        return "********".to_string();
    }
    let left: String = key.chars().take(3).collect();
    let right: Vec<char> = key.chars().collect();
    let right: String = right[right.len() - 4..].iter().collect();
    format!("{left}****{right}")
}

/// Single funnel so the store never grows `if provider == …` (red line C3).
/// Delegates to the provider registry's chatCapable flag — all four
/// providers (kimi/claude/codex/custom) are chat-capable.
pub fn provider_supports_chat(provider: &str) -> bool {
    crate::provider::chat_capable(provider)
}

#[derive(Debug, Clone, Default)]
pub struct AgentStore {
    agents: Vec<Agent>,
    default_agent_id: String,
}

impl AgentStore {
    /// loadFromDisk: read the index, pick up orphan files, apply the default
    /// rules. Tolerant — unreadable individual files are skipped.
    pub fn load(paths: &Paths) -> Self {
        paths.ensure_layout();
        let mut store = Self::default();

        let index = crate::store::json::read_object(&paths.agents_index_path());
        store.default_agent_id = index
            .get("defaultAgentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut ids: Vec<String> = index
            .get("agents")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        // Orphan pickup: any agents/*.json besides index.json is an agent.
        if let Ok(entries) = fs::read_dir(paths.agents_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "index.json" || !name.ends_with(".json") {
                    continue;
                }
                let id = &name[..name.len() - 5];
                if !ids.iter().any(|i| i == id) {
                    ids.push(id.to_string());
                }
            }
        }

        for id in &ids {
            let Ok(bytes) = fs::read(paths.agent_file_path(id)) else {
                continue;
            };
            let Ok(mut agent) = serde_json::from_slice::<Agent>(&bytes) else {
                continue;
            };
            if agent.id.is_empty() {
                agent.id = id.clone(); // backfill from the file name
            }
            // isDefault is authoritative in the index, not the file.
            agent.is_default = agent.id == store.default_agent_id;
            store.agents.push(agent);
        }

        if store.default_agent_id.is_empty() && !store.agents.is_empty() {
            // pick first usable kimi
            if let Some(a) = store
                .agents
                .iter_mut()
                .find(|a| provider_supports_chat(&a.provider))
            {
                a.is_default = true;
                store.default_agent_id = a.id.clone();
            }
        }
        store
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    pub fn default_agent_id(&self) -> &str {
        &self.default_agent_id
    }

    pub fn get(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn default_agent(&self) -> Option<&Agent> {
        self.get(&self.default_agent_id)
    }

    /// createAgent: provider kimi / model moonshot-v1-auto / cliPath "" (left
    /// empty so config auto-detect can fill it), first agent becomes default.
    pub fn create_agent(&mut self, paths: &Paths, name: &str) -> Result<String, AgentsError> {
        let now = now_ms();
        let agent = Agent {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            name: if name.is_empty() {
                "新 Agent".to_string()
            } else {
                name.to_string()
            },
            provider: "kimi".to_string(),
            model: "moonshot-v1-auto".to_string(),
            cli_path: String::new(),
            enabled: true,
            is_default: self.agents.is_empty(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let id = agent.id.clone();
        if agent.is_default {
            self.default_agent_id = id.clone();
        }
        self.save_agent(paths, &agent)?;
        self.agents.push(agent);
        self.save_index(paths)?;
        Ok(id)
    }

    /// updateAgent: trims strings, lowercases provider, guards the apiKey
    /// against empty/masked write-backs, bumps updatedAt, rewrites the agent
    /// file and the index.
    pub fn update_agent(
        &mut self,
        paths: &Paths,
        id: &str,
        patch: &AgentPatch,
    ) -> Result<(), AgentsError> {
        let Some(pos) = self.agents.iter().position(|a| a.id == id) else {
            return Err(AgentsError::NotFound);
        };
        let a = &mut self.agents[pos];
        if let Some(v) = &patch.name {
            a.name = v.trim().to_string();
        }
        if let Some(v) = &patch.provider {
            a.provider = v.trim().to_lowercase();
        }
        if let Some(v) = &patch.model {
            a.model = v.trim().to_string();
        }
        if let Some(v) = &patch.base_url {
            a.base_url = v.trim().to_string();
        }
        if let Some(v) = &patch.default_effort {
            a.default_effort = v.trim().to_lowercase();
        }
        if let Some(v) = patch.max_context_k {
            a.max_context_k = v;
        }
        if let Some(v) = &patch.cli_path {
            a.cli_path = v.trim().to_string();
        }
        if let Some(k) = &patch.api_key {
            // empty or still masked -> keep old
            if !k.is_empty() && !k.contains('*') {
                a.api_key = k.clone();
            }
        }
        if let Some(v) = &patch.extra_args {
            a.extra_args = v.clone();
        }
        // Stored as-is (JSON text); validity is checked when a session starts.
        if let Some(v) = &patch.mcp_servers {
            a.mcp_servers = v.clone();
        }
        // Empty string clears back to the built-in default avatar.
        if let Some(v) = &patch.avatar_path {
            a.avatar_path = v.trim().to_string();
        }
        if let Some(v) = patch.enabled {
            a.enabled = v;
        }
        a.updated_at = now_ms();
        let agent = a.clone();
        self.save_agent(paths, &agent)?;
        self.save_index(paths)?;
        Ok(())
    }

    /// removeAgent: delete the file + drop from the index; when the default
    /// agent is removed the list's first agent takes over.
    pub fn remove_agent(&mut self, paths: &Paths, id: &str) -> Result<bool, AgentsError> {
        let Some(pos) = self.agents.iter().position(|a| a.id == id) else {
            return Ok(false);
        };
        self.agents.remove(pos);
        let _ = fs::remove_file(paths.agent_file_path(id));

        if self.default_agent_id == id {
            self.default_agent_id.clear();
            if let Some(first) = self.agents.first_mut() {
                first.is_default = true;
                self.default_agent_id = first.id.clone();
                let first = first.clone();
                self.save_agent(paths, &first)?;
            }
        }
        self.save_index(paths)?;
        Ok(true)
    }

    /// setDefault: rewrites isDefault in every agent file whose flag changed.
    pub fn set_default(&mut self, paths: &Paths, id: &str) -> Result<(), AgentsError> {
        let Some(pos) = self.agents.iter().position(|a| a.id == id) else {
            return Err(AgentsError::NotFound);
        };
        if !provider_supports_chat(&self.agents[pos].provider) {
            return Err(AgentsError::DefaultUnsupported);
        }
        for r in 0..self.agents.len() {
            let def = r == pos;
            if self.agents[r].is_default != def {
                self.agents[r].is_default = def;
                let agent = self.agents[r].clone();
                self.save_agent(paths, &agent)?;
            }
        }
        self.default_agent_id = id.to_string();
        self.save_index(paths)?;
        Ok(())
    }

    pub fn save_index(&self, paths: &Paths) -> Result<(), AgentsError> {
        let mut root = Map::new();
        root.insert(
            "agents".to_string(),
            Value::Array(
                self.agents
                    .iter()
                    .map(|a| Value::String(a.id.clone()))
                    .collect(),
            ),
        );
        root.insert(
            "defaultAgentId".to_string(),
            Value::String(self.default_agent_id.clone()),
        );
        write_value_atomic(&paths.agents_index_path(), &Value::Object(root))?;
        Ok(())
    }

    pub fn save_agent(&self, paths: &Paths, agent: &Agent) -> Result<(), AgentsError> {
        write_value_atomic(&paths.agent_file_path(&agent.id), agent)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_key_rules() {
        assert_eq!(mask_key(""), "");
        assert_eq!(mask_key("short"), "********");
        assert_eq!(mask_key("12345678"), "********");
        assert_eq!(mask_key("sk-abcdefghijk"), "sk-****hijk");
    }
}
