// usage.json: per-turn token usage records (one entry per agent turn that
// reported usage), plus the aggregated report behind the `usage_report`
// command. Records are kept in append order on disk; the aggregated views
// are computed in memory only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::acp::events::TurnUsage;
use crate::store::json::{de_ms_i64, now_ms, write_value_atomic, JsonError};
use crate::store::paths::Paths;
use crate::store::sessions::MessageRow;

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

    /// 记录 → 回合用量（回填挂到消息行的字段形状与实时 usage 一致）。
    pub fn to_turn_usage(&self) -> TurnUsage {
        TurnUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: self.total_tokens,
            cached_read_tokens: self.cached_read_tokens,
            cached_write_tokens: self.cached_write_tokens,
            thought_tokens: self.thought_tokens,
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

/// Per-session usage for the info panel (`session_usage` command): totals
/// plus cached/thought token sums and the context estimate (latest record's
/// input — kimi prompts carry the full context each round).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageView {
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_read_tokens: u64,
    pub cached_write_tokens: u64,
    pub thought_tokens: u64,
    pub context_tokens: u64,
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

    /// 回填用批量追加：全部进内存后一次原子写。
    pub fn append_many(&mut self, paths: &Paths, records: Vec<UsageRecord>) -> Result<(), UsageError> {
        if records.is_empty() {
            return Ok(());
        }
        self.records.extend(records);
        self.save(paths)
    }

    /// 该会话已有记录的最早 ts（回填防重用）；无记录返回 None。
    pub fn earliest_ts(&self, session_id: &str) -> Option<i64> {
        self.records
            .iter()
            .filter(|r| r.session_id == session_id)
            .map(|r| r.ts)
            .min()
    }

    /// Aggregate ONE session's records (append order = time order, so the
    /// last matching record is the latest turn). Returns None when the
    /// session has no usage records at all.
    pub fn for_session(&self, session_id: &str) -> Option<SessionUsageView> {
        let mut v = SessionUsageView::default();
        let mut found = false;
        for r in &self.records {
            if r.session_id != session_id {
                continue;
            }
            found = true;
            v.turns += 1;
            v.input_tokens += r.input_tokens;
            v.output_tokens += r.output_tokens;
            v.total_tokens += r.total_tokens;
            v.cached_read_tokens += r.cached_read_tokens.unwrap_or(0);
            v.cached_write_tokens += r.cached_write_tokens.unwrap_or(0);
            v.thought_tokens += r.thought_tokens.unwrap_or(0);
            v.context_tokens = r.input_tokens;
        }
        found.then_some(v)
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

/// 把 usage.json 里该会话的回填记录按时间归组挂到 messages.jsonl 中缺失
/// usage 的 assistant 行上。
///
/// 背景：usage_backfill 只把上线前的历史回合写进 usage.json（用量页/会话
/// 信息面板聚合用），messages.jsonl 里旧回合的行没有 usage 字段，气泡头部
/// 的 ↑↓ 用量因此不显示。这里在读取（session_messages）时做一次非破坏性
/// 合并，规则：
/// - 行 created_at = 回合开始、记录 ts = 回合结束，故一回合的记录 ts 恒在
///   [本行 created_at, 下一行 created_at) 内 —— 记录归到「created_at ≤ ts」
///   的最后一个 assistant 行就是它所在回合的行，无需时间窗口。
/// - 同一回合的多条记录（工具循环里多次 LLM 调用，claude/opencode 档案
///   逐次记录）叠加成该回合的总用量，避免只采到一次调用。
/// - 已有 usage 的行（新回合实时记录）不覆盖，其记录因归到它名下而被
///   跳过，不重复计入。
/// 不做任何磁盘回写，重开会话即自愈；user/command 等行不挂。
pub fn attach_usage_backfill(rows: &mut [MessageRow], usage: &UsageStore, session_id: &str) {
    let mut recs: Vec<&UsageRecord> = usage
        .records()
        .iter()
        .filter(|r| r.session_id == session_id)
        .collect();
    if recs.is_empty() {
        return;
    }
    recs.sort_by_key(|r| r.ts);
    // assistant 行下标（created_at 递增）；has_usage 的行只用于定位回合，
    // 不接收回填记录。
    let assistants: Vec<(usize, i64, bool)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.role == "assistant")
        .map(|(i, r)| (i, r.created_at, r.usage.is_some()))
        .collect();
    if assistants.is_empty() {
        return;
    }
    let mut sums: Vec<(usize, TurnUsage)> = Vec::new();
    for rec in &recs {
        let Some(&(idx, _, has_usage)) = assistants
            .iter()
            .rev()
            .find(|&&(_, created, _)| created <= rec.ts)
        else {
            continue; // 早于第一个 assistant 行：无主记录，忽略
        };
        if has_usage {
            continue; // 实时回合已计入，回填记录不重复
        }
        match sums.iter_mut().find(|(i, _)| *i == idx) {
            Some((_, u)) => u.add_from(rec),
            None => sums.push((idx, rec.to_turn_usage())),
        }
    }
    for (idx, u) in sums {
        rows[idx].usage = Some(u);
    }
}

impl TurnUsage {
    /// 把一条回填记录累加进回合总用量（同一回合多次 LLM 调用求和）。
    fn add_from(&mut self, rec: &UsageRecord) {
        self.input_tokens += rec.input_tokens;
        self.output_tokens += rec.output_tokens;
        self.total_tokens += rec.total_tokens;
        self.cached_read_tokens = opt_add(self.cached_read_tokens, rec.cached_read_tokens);
        self.cached_write_tokens = opt_add(self.cached_write_tokens, rec.cached_write_tokens);
        self.thought_tokens = opt_add(self.thought_tokens, rec.thought_tokens);
    }
}

fn opt_add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
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

    #[test]
    fn for_session_aggregates_one_session() {
        let mut store = UsageStore::default();
        let mut r1 = rec("s1", "a1", "Kimi", "k2", 100);
        r1.cached_read_tokens = Some(40);
        r1.thought_tokens = Some(10);
        let mut r2 = rec("s1", "a1", "Kimi", "k2", 300);
        r2.cached_read_tokens = Some(200);
        r2.cached_write_tokens = Some(30);
        let mut r3 = rec("s2", "a2", "Claude", "sonnet", 200);
        r3.cached_read_tokens = Some(150);
        store.records.extend([r1, r2, r3]);

        let v = store.for_session("s1").expect("session has records");
        assert_eq!(v.turns, 2);
        assert_eq!(v.input_tokens, 200);
        assert_eq!(v.output_tokens, 200);
        assert_eq!(v.total_tokens, 400);
        assert_eq!(v.cached_read_tokens, 240);
        assert_eq!(v.cached_write_tokens, 30);
        assert_eq!(v.thought_tokens, 10);
        // Latest record's input wins the context estimate.
        assert_eq!(v.context_tokens, 150);

        assert!(store.for_session("nope").is_none());
    }

    #[test]
    fn attach_usage_backfill_matches_chronological_rows() {
        let mut store = UsageStore::default();
        let mut r1 = rec("s1", "a1", "Kimi", "k2", 100);
        r1.ts = 10_000;
        let mut r2 = rec("s1", "a1", "Kimi", "k2", 300);
        r2.ts = 20_000;
        store.records.extend([r1, r2]);

        let row = |role: &str, created: i64, usage: bool| MessageRow {
            id: format!("{role}-{created}"),
            role: role.into(),
            created_at: created,
            usage: usage.then_some(TurnUsage::default()),
            ..Default::default()
        };
        // 回合1（无 usage 行）→ 回合2（无 usage 行）→ 已有 usage 的行。
        let mut rows = vec![
            row("user", 9_000, false),
            row("assistant", 9_100, false),
            row("user", 19_000, false),
            row("assistant", 19_100, false),
            row("assistant", 25_000, true),
        ];
        attach_usage_backfill(&mut rows, &store, "s1");
        assert_eq!(rows[1].usage.as_ref().map(|u| u.total_tokens), Some(100));
        assert_eq!(rows[3].usage.as_ref().map(|u| u.total_tokens), Some(300));
        assert_eq!(
            rows[4].usage.as_ref().map(|u| u.total_tokens),
            Some(0),
            "已有 usage 的行不覆盖"
        );
        // 别的会话的记录不串。
        attach_usage_backfill(&mut rows, &store, "other");
        assert_eq!(rows[1].usage.as_ref().map(|u| u.total_tokens), Some(100));
        assert_eq!(rows[3].usage.as_ref().map(|u| u.total_tokens), Some(300));
    }

    #[test]
    fn attach_usage_backfill_sums_tool_loop_records_into_one_turn() {
        // 一个回合内多次 LLM 调用（claude/opencode 档案逐次记录）：
        // 挂同一行时叠加，而不是只采最近一条。
        let mut store = UsageStore::default();
        for (ts, total) in [(10_000u64, 100u64), (10_200, 200), (10_400, 50)] {
            let mut r = rec("s1", "a1", "Kimi", "k2", total);
            r.ts = ts as i64;
            store.records.push(r);
        }
        let mut rows = vec![MessageRow {
            id: "a1".into(),
            role: "assistant".into(),
            created_at: 9_900,
            ..Default::default()
        }];
        attach_usage_backfill(&mut rows, &store, "s1");
        let u = rows[0].usage.as_ref().expect("usage attached");
        assert_eq!(u.total_tokens, 100 + 200 + 50, "同回合记录求和");
        assert_eq!(u.input_tokens, 50 + 100 + 25);
        assert_eq!(u.output_tokens, 50 + 100 + 25);
    }

    #[test]
    fn attach_usage_backfill_partial_session_skips_live_rows() {
        // 功能上线中点：前两回合是历史（无 usage，回填记录），后两回合是
        // 实时（已有 usage）。实时行的记录必须跳过，不能串到前面的历史行。
        let mut store = UsageStore::default();
        let mut r1 = rec("s1", "a1", "Kimi", "k2", 100);
        r1.ts = 10_000;
        let mut r2 = rec("s1", "a1", "Kimi", "k2", 200);
        r2.ts = 30_000;
        let mut r3 = rec("s1", "a1", "Kimi", "k2", 999);
        r3.ts = 40_000; // 实时回合 1 的记录
        let mut r4 = rec("s1", "a1", "Kimi", "k2", 888);
        r4.ts = 60_000; // 实时回合 2 的记录
        store.records.extend([r1, r2, r3, r4]);

        let row = |role: &str, created: i64, usage: bool| MessageRow {
            id: format!("{role}-{created}"),
            role: role.into(),
            created_at: created,
            usage: usage.then_some(TurnUsage {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut rows = vec![
            row("user", 9_000, false),
            row("assistant", 9_100, false),
            row("user", 29_000, false),
            row("assistant", 29_100, false),
            row("user", 39_000, false),
            row("assistant", 39_100, true),
            row("user", 59_000, false),
            row("assistant", 59_100, true),
        ];
        attach_usage_backfill(&mut rows, &store, "s1");
        assert_eq!(rows[1].usage.as_ref().map(|u| u.total_tokens), Some(100));
        assert_eq!(rows[3].usage.as_ref().map(|u| u.total_tokens), Some(200));
        assert_eq!(rows[5].usage.as_ref().map(|u| u.total_tokens), Some(3), "实时行保留原值");
        assert_eq!(rows[7].usage.as_ref().map(|u| u.total_tokens), Some(3), "实时行保留原值");
    }

    #[test]
    fn attach_usage_backfill_skips_user_rows_and_other_sessions() {
        let mut store = UsageStore::default();
        let mut r = rec("s1", "a1", "Kimi", "k2", 7);
        r.ts = 50_000;
        store.records.push(r);
        let mut rows = vec![
            MessageRow {
                id: "u1".into(),
                role: "user".into(),
                created_at: 49_000,
                ..Default::default()
            },
            MessageRow {
                id: "a1".into(),
                role: "assistant".into(),
                created_at: 49_100,
                ..Default::default()
            },
        ];
        attach_usage_backfill(&mut rows, &store, "s1");
        assert!(rows[0].usage.is_none(), "user 行不挂");
        assert_eq!(rows[1].usage.as_ref().map(|u| u.total_tokens), Some(7));
    }
}
