// usage.json: per-turn token usage records (one entry per agent turn that
// reported usage), plus the aggregated report behind the `usage_report`
// command. Records are kept in append order on disk; the aggregated views
// are computed in memory only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::acp::events::TurnUsage;
use crate::store::json::{de_ms_i64, now_ms, write_value_atomic, JsonError};
use crate::store::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageRecord {
    #[serde(rename = "ts", deserialize_with = "de_ms_i64")]
    pub ts: i64,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    /// Model the turn ran on; "" when unknown (older records, no picker).
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u64>,
}

impl UsageRecord {
    pub fn new(
        session_id: &str,
        agent_id: &str,
        agent_name: &str,
        model: &str,
        usage: &TurnUsage,
    ) -> Self {
        Self {
            ts: now_ms(),
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            agent_name: agent_name.to_string(),
            model: model.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            cached_write_tokens: usage.cached_write_tokens,
            thought_tokens: usage.thought_tokens,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct UsageFile {
    records: Vec<UsageRecord>,
}

/// Token totals shared by every aggregation level.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

impl UsageTotals {
    fn add(&mut self, r: &UsageRecord) {
        self.turns += 1;
        self.input_tokens += r.input_tokens;
        self.output_tokens += r.output_tokens;
        self.total_tokens += r.total_tokens;
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReport {
    pub model: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReport {
    pub agent_id: String,
    pub agent_name: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
    pub models: Vec<ModelReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReport {
    pub session_id: String,
    pub agent_name: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub grand: UsageTotals,
    pub agents: Vec<AgentReport>,
    pub sessions: Vec<SessionReport>,
}

#[derive(Debug, Default)]
pub struct UsageStore {
    records: Vec<UsageRecord>,
}

impl UsageStore {
    pub fn load(paths: &Paths) -> Self {
        let file: UsageFile = std::fs::read(paths.usage_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            records: file
                .records
                .into_iter()
                .filter(|r| !r.session_id.is_empty())
                .collect(),
        }
    }

    pub fn records(&self) -> &[UsageRecord] {
        &self.records
    }

    pub fn append(&mut self, paths: &Paths, record: UsageRecord) -> Result<(), UsageError> {
        self.records.push(record);
        self.save(paths)
    }

    pub fn save(&self, paths: &Paths) -> Result<(), UsageError> {
        paths.ensure_layout();
        let file = UsageFile {
            records: self.records.clone(),
        };
        write_value_atomic(&paths.usage_path(), &file)?;
        Ok(())
    }

    /// Aggregated view for the `usage_report` command: grand totals, per
    /// agent (with per-model breakdown) and per session. Agents and sessions
    /// are sorted by totalTokens desc.
    pub fn report(&self) -> UsageReport {
        let mut grand = UsageTotals::default();
        // BTreeMap: deterministic grouping; order is fixed by the final sort.
        let mut agents: BTreeMap<String, (String, UsageTotals, BTreeMap<String, UsageTotals>)> =
            BTreeMap::new();
        let mut sessions: BTreeMap<String, (String, UsageTotals)> = BTreeMap::new();

        for r in &self.records {
            grand.add(r);
            let agent = agents
                .entry(r.agent_id.clone())
                .or_insert_with(|| (r.agent_name.clone(), UsageTotals::default(), BTreeMap::new()));
            agent.0 = r.agent_name.clone(); // latest name wins on renames
            agent.1.add(r);
            agent
                .2
                .entry(r.model.clone())
                .or_default()
                .add(r);
            let session = sessions
                .entry(r.session_id.clone())
                .or_insert_with(|| (r.agent_name.clone(), UsageTotals::default()));
            session.0 = r.agent_name.clone();
            session.1.add(r);
        }

        let mut agents: Vec<AgentReport> = agents
            .into_iter()
            .map(|(agent_id, (agent_name, totals, models))| {
                let mut models: Vec<ModelReport> = models
                    .into_iter()
                    .map(|(model, totals)| ModelReport { model, totals })
                    .collect();
                models.sort_by_key(|m| std::cmp::Reverse(m.totals.total_tokens));
                AgentReport {
                    agent_id,
                    agent_name,
                    totals,
                    models,
                }
            })
            .collect();
        agents.sort_by_key(|a| std::cmp::Reverse(a.totals.total_tokens));

        let mut sessions: Vec<SessionReport> = sessions
            .into_iter()
            .map(|(session_id, (agent_name, totals))| SessionReport {
                session_id,
                agent_name,
                totals,
            })
            .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.totals.total_tokens));

        UsageReport {
            grand,
            agents,
            sessions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(session: &str, agent: &str, name: &str, model: &str, total: u64) -> UsageRecord {
        UsageRecord {
            session_id: session.into(),
            agent_id: agent.into(),
            agent_name: name.into(),
            model: model.into(),
            input_tokens: total / 2,
            output_tokens: total - total / 2,
            total_tokens: total,
            ..Default::default()
        }
    }

    #[test]
    fn report_aggregates_and_sorts() {
        let mut store = UsageStore::default();
        store.records.push(rec("s1", "a1", "Kimi", "k2", 100));
        store.records.push(rec("s1", "a1", "Kimi", "k2-thinking", 300));
        store.records.push(rec("s2", "a2", "Claude", "sonnet", 200));

        let report = store.report();
        assert_eq!(report.grand.turns, 3);
        assert_eq!(report.grand.total_tokens, 600);

        // Agents sorted by totalTokens desc: a1 (400) before a2 (200).
        assert_eq!(report.agents.len(), 2);
        assert_eq!(report.agents[0].agent_id, "a1");
        assert_eq!(report.agents[0].totals.total_tokens, 400);
        assert_eq!(report.agents[0].models.len(), 2);
        assert_eq!(report.agents[0].models[0].model, "k2-thinking");

        // Sessions sorted by totalTokens desc: s1 (400) before s2 (200).
        assert_eq!(report.sessions.len(), 2);
        assert_eq!(report.sessions[0].session_id, "s1");
        assert_eq!(report.sessions[0].agent_name, "Kimi");
        assert_eq!(report.sessions[1].session_id, "s2");
    }
}
