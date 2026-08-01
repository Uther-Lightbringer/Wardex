// usage_backfill 命令：把功能上线前的历史会话用量从各家 CLI 档案一次性
// 补进 usage.json。
//
// 遍历 sessions store 的全部会话 meta，provider 为 kimi/claude/codex 且
// 有 acpSessionId 的，用 chat/wire.rs 的全量解析（offset 0、不 EOF 对齐、
// 与增量 ArchiveReader 状态无关）逐条生成 UsageRecord：
// - ts 用档案记录自带时间戳（kimi time ms；claude/codex 的 ISO timestamp）
// - model 用会话 meta.model（档案里的占位符不采）
// 防重：usage.json 里已有该 session_id 的记录时，只补 ts 早于已有最早
// 记录的；一条都补不进就计入 skipped。全部收集完一次原子写。

use std::path::PathBuf;

use serde::Serialize;

use crate::chat::wire::{self, ArchiveKind};
use crate::store::{Paths, SessionMeta, StoreRegistry, UsageRecord, UsageStore};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillSummary {
    /// 扫描的会话总数。
    pub sessions: u64,
    /// 命中的档案文件数。
    pub files: u64,
    /// 新增记录数。
    pub added: u64,
    /// 已有更全数据而跳过的会话数。
    pub skipped: u64,
}

pub fn backfill(stores: &mut StoreRegistry) -> BackfillSummary {
    let metas: Vec<SessionMeta> = {
        let ids: Vec<String> = stores.sessions.list().iter().map(|r| r.id.clone()).collect();
        ids.into_iter()
            .filter_map(|id| stores.sessions.meta_for(&id))
            .collect()
    };
    let paths = stores.paths.clone();
    backfill_with(&mut stores.usage, &paths, &metas, &|kind| match kind {
        ArchiveKind::Kimi => wire::kimi_home(),
        ArchiveKind::Claude => wire::claude_home(),
        ArchiveKind::Codex => wire::codex_home(),
    })
}

fn backfill_with(
    usage: &mut UsageStore,
    paths: &Paths,
    metas: &[SessionMeta],
    home_of: &dyn Fn(ArchiveKind) -> Option<PathBuf>,
) -> BackfillSummary {
    let mut summary = BackfillSummary::default();
    let mut new_records = Vec::new();
    for meta in metas {
        summary.sessions += 1;
        let Some(kind) = ArchiveKind::for_provider(&meta.provider) else {
            continue;
        };
        let Some(sid) = meta.acp_session_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(home) = home_of(kind) else {
            continue;
        };
        let (files, recs) = wire::read_archive_full(kind, &home, sid, &meta.work_dir);
        if files == 0 {
            continue;
        }
        summary.files += files as u64;
        let earliest = usage.earliest_ts(&meta.id);
        let mut added_here = 0u64;
        for r in recs {
            if earliest.is_some_and(|min| r.ts >= min) {
                continue;
            }
            new_records.push(UsageRecord {
                ts: r.ts,
                session_id: meta.id.clone(),
                agent_id: meta.agent_id.clone(),
                agent_name: meta.agent_name.clone(),
                model: meta.model.clone(),
                input_tokens: r.usage.input_tokens,
                output_tokens: r.usage.output_tokens,
                total_tokens: r.usage.total_tokens,
                cached_read_tokens: r.usage.cached_read_tokens,
                cached_write_tokens: r.usage.cached_write_tokens,
                thought_tokens: r.usage.thought_tokens,
            });
            added_here += 1;
        }
        if earliest.is_some() && added_here == 0 {
            summary.skipped += 1;
        }
        summary.added += added_here;
    }
    if let Err(e) = usage.append_many(paths, new_records) {
        log::warn!("usage backfill save failed: {e}");
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, provider: &str, acp_sid: &str, work_dir: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            provider: provider.to_string(),
            acp_session_id: Some(acp_sid.to_string()),
            agent_id: format!("agent-{id}"),
            agent_name: format!("Agent {id}"),
            model: "m-1".to_string(),
            work_dir: work_dir.to_string(),
            ..Default::default()
        }
    }

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().join("data"));
        (tmp, paths)
    }

    fn rec(session_id: &str, ts: i64, total: u64) -> UsageRecord {
        UsageRecord {
            ts,
            session_id: session_id.to_string(),
            total_tokens: total,
            ..Default::default()
        }
    }

    fn claude_archive(home: &PathBuf, slug: &str, sid: &str, lines: &[String]) {
        let dir = home.join("projects").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{sid}.jsonl")), format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn claude_req(req: &str, ts: &str, input: u64, output: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"requestId\":\"{req}\",\"timestamp\":\"{ts}\",\"message\":{{\"usage\":{{\"input_tokens\":{input},\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":{output}}}}}}}"
        )
    }

    #[test]
    fn backfill_claude_records_deduped_with_ts() {
        let (_t, paths) = temp_paths();
        let claude_home = tempfile::tempdir().unwrap();
        let home = claude_home.path().to_path_buf();
        claude_archive(
            &home,
            "C--p",
            "sid1",
            &[
                claude_req("r1", "2026-07-30T09:00:00.000Z", 100, 5),
                claude_req("r1", "2026-07-30T09:00:01.000Z", 18762, 149),
                claude_req("r2", "2026-07-31T10:00:00.000Z", 20, 10),
            ],
        );

        let mut usage = UsageStore::default();
        let metas = vec![meta("s1", "claude", "sid1", "C:/p")];
        let home2 = home.clone();
        let s = backfill_with(&mut usage, &paths, &metas, &move |kind| {
            (kind == ArchiveKind::Claude).then(|| home2.clone())
        });

        assert_eq!(s.sessions, 1);
        assert_eq!(s.files, 1);
        // r1 去重成一条（output 最大那行，ts 用该行 timestamp）+ r2。
        assert_eq!(s.added, 2);
        assert_eq!(s.skipped, 0);
        let rows = usage.records();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].total_tokens, 18762 + 149);
        assert_eq!(rows[0].session_id, "s1");
        assert_eq!(rows[0].agent_id, "agent-s1");
        assert_eq!(rows[0].agent_name, "Agent s1");
        assert_eq!(rows[0].model, "m-1");
        assert_eq!(
            rows[0].ts,
            chrono::DateTime::parse_from_rfc3339("2026-07-30T09:00:01.000Z")
                .unwrap()
                .timestamp_millis()
        );
        // 磁盘上已持久化（append_many 一次原子写）。
        assert!(paths.usage_path().is_file());
    }

    #[test]
    fn backfill_skips_sessions_with_fuller_data() {
        let (_t, paths) = temp_paths();
        let claude_home = tempfile::tempdir().unwrap();
        let home = claude_home.path().to_path_buf();
        claude_archive(
            &home,
            "C--p",
            "sid1",
            &[
                claude_req("r1", "2026-07-30T09:00:00.000Z", 10, 5),
                claude_req("r2", "2026-08-01T09:00:00.000Z", 20, 10),
            ],
        );
        let ts = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
        };

        // 已有记录的最早 ts 在两条档案记录之间：只补更早的那条。
        let mut usage = UsageStore::default();
        usage
            .append(&paths, rec("s1", ts("2026-07-31T00:00:00.000Z"), 999))
            .unwrap();
        let metas = vec![meta("s1", "claude", "sid1", "C:/p")];
        let home2 = home.clone();
        let s = backfill_with(&mut usage, &paths, &metas, &move |kind| {
            (kind == ArchiveKind::Claude).then(|| home2.clone())
        });
        assert_eq!(s.added, 1, "only the earlier record backfilled");
        assert_eq!(s.skipped, 0);
        assert_eq!(usage.records().len(), 2);

        // 再跑一遍：没有更早的了 → 整个会话跳过。
        let home3 = home.clone();
        let s = backfill_with(&mut usage, &paths, &metas, &move |kind| {
            (kind == ArchiveKind::Claude).then(|| home3.clone())
        });
        assert_eq!(s.added, 0);
        assert_eq!(s.skipped, 1);
        assert_eq!(usage.records().len(), 2, "no duplicates");
    }

    #[test]
    fn backfill_kimi_multi_agent_files() {
        let (_t, paths) = temp_paths();
        let kimi_home = tempfile::tempdir().unwrap();
        let home = kimi_home.path().to_path_buf();
        let line = |time: i64, input: u64, output: u64| {
            format!(
                "{{\"type\":\"usage.record\",\"usage\":{{\"inputOther\":{input},\"output\":{output},\"inputCacheRead\":0,\"inputCacheCreation\":0}},\"usageScope\":\"turn\",\"time\":{time}}}"
            )
        };
        for (agent, lines) in [
            ("main", vec![line(1000, 10, 5), line(2000, 20, 8)]),
            ("agent-0", vec![line(3000, 30, 12)]),
        ] {
            let dir = home
                .join("sessions")
                .join("wd_p_h")
                .join("session_ksid")
                .join("agents")
                .join(agent);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("wire.jsonl"), format!("{}\n", lines.join("\n"))).unwrap();
        }

        let mut usage = UsageStore::default();
        let metas = vec![meta("s-k", "kimi", "ksid", "C:/p")];
        let home2 = home.clone();
        let s = backfill_with(&mut usage, &paths, &metas, &move |kind| {
            (kind == ArchiveKind::Kimi).then(|| home2.clone())
        });
        assert_eq!(s.files, 2);
        assert_eq!(s.added, 3, "每条 usage.record 一条 UsageRecord");
        let mut ts: Vec<i64> = usage.records().iter().map(|r| r.ts).collect();
        ts.sort();
        assert_eq!(ts, vec![1000, 2000, 3000]);
        assert_eq!(
            usage.records().iter().map(|r| r.total_tokens).sum::<u64>(),
            15 + 28 + 42
        );
    }

    #[test]
    fn backfill_codex_and_unsupported_provider() {
        let (_t, paths) = temp_paths();
        let codex_home = tempfile::tempdir().unwrap();
        let home = codex_home.path().to_path_buf();
        let dir = home.join("sessions").join("2026").join("07").join("31");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rollout-t-csid.jsonl"),
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-07-31T08:00:00.000Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":4,\"output_tokens\":10,\"reasoning_output_tokens\":3,\"total_tokens\":110}}}}\n",
        )
        .unwrap();

        let mut usage = UsageStore::default();
        let metas = vec![
            meta("s-c", "codex", "csid", ""),
            meta("s-x", "custom", "whatever", ""), // 不支持的 provider
        ];
        let home2 = home.clone();
        let s = backfill_with(&mut usage, &paths, &metas, &move |kind| {
            (kind == ArchiveKind::Codex).then(|| home2.clone())
        });
        assert_eq!(s.sessions, 2);
        assert_eq!(s.files, 1);
        assert_eq!(s.added, 1);
        let r = &usage.records()[0];
        assert_eq!(r.total_tokens, 110, "档案给的 total 直接采");
        assert_eq!(r.cached_read_tokens, Some(4));
        assert_eq!(r.thought_tokens, Some(3));
        assert!(r.ts > 0);
    }
}
