// Unit tests for the session store: round-trip, legacy formats, write-timing
// contract, LRU, search.

use super::*;
use crate::store::paths::Paths;

fn temp_paths() -> (tempfile::TempDir, Paths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::new(tmp.path().to_path_buf());
    (tmp, paths)
}

fn make_store_with_session(paths: &Paths) -> (SessionStore, String) {
    let mut store = SessionStore::load(paths.clone());
    let agent = AgentSnapshot {
        id: "agent-1".to_string(),
        name: "Kimi 主 Agent".to_string(),
        provider: "kimi".to_string(),
        model: "kimi-for-coding".to_string(),
        ..Default::default()
    };
    let id = store.create_session(&agent, "").unwrap();
    (store, id)
}

#[test]
fn meta_roundtrip_preserves_unknown_fields() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    // Drop the in-memory meta first: the passthrough guarantee concerns what
    // is READ FROM DISK, so inject the unknown key before the next open.
    store.release_session(&id);

    // Hand-inject an unknown key into meta.json, then trigger a rewrite via
    // rename; the unknown key must survive (§0 flatten passthrough).
    let meta_path = paths.session_meta_path(&id);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    value["futureField"] = serde_json::json!({"nested": [1, 2, 3]});
    std::fs::write(&meta_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    store.rename_session(&id, "改名后的会话").unwrap();

    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    assert_eq!(after["futureField"], serde_json::json!({"nested": [1, 2, 3]}));
    assert_eq!(after["title"], serde_json::json!("改名后的会话"));
    // rename must NOT bump updatedAt (§3.1)
    assert_eq!(after["updatedAt"], value["updatedAt"]);
}

#[test]
fn meta_written_indented_and_sorted() {
    let (_tmp, paths) = temp_paths();
    let (_store, id) = make_store_with_session(&paths);
    let text = String::from_utf8(std::fs::read(paths.session_meta_path(&id)).unwrap()).unwrap();
    // 4-space indent (Qt QJsonDocument::Indented)
    assert!(text.contains("\n    \"agentId\""), "meta should be 4-space indented:\n{text}");
    let pos = |k: &str| text.find(&format!("\"{k}\"")).unwrap();
    // alphabetical key order (QJsonObject)
    assert!(pos("agentId") < pos("agentName"));
    assert!(pos("createdAt") < pos("id"));
    assert!(pos("summary") < pos("title"));
    // integer-shaped timestamps
    assert!(!text.contains("createdAt\": 1."));
}

#[test]
fn append_line_has_no_segments_key_rewrite_has_it() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    store
        .append_message(&id, "user", "你好世界", "kimi", "done", &[], "")
        .unwrap();
    let text =
        String::from_utf8(std::fs::read(paths.session_messages_path(&id)).unwrap()).unwrap();
    assert!(!text.contains("\"segments\""), "appended row must not carry segments: {text}");
    assert!(text.contains("\"toolCalls\":[]"));

    // Assistant placeholder + streaming (memory only) + flush → rewrite with segments.
    store
        .append_message(&id, "assistant", "…", "kimi", "pending", &[], "")
        .unwrap();
    let before_flush =
        String::from_utf8(std::fs::read(paths.session_messages_path(&id)).unwrap()).unwrap();
    assert!(before_flush.lines().count() == 2);
    assert!(store.append_last_assistant_content(&id, "回复正文"));
    // streaming chunks must NOT touch the disk
    let during_stream =
        String::from_utf8(std::fs::read(paths.session_messages_path(&id)).unwrap()).unwrap();
    assert_eq!(before_flush, during_stream, "streaming must be zero disk I/O");

    store.flush_last_assistant(&id, Some("done"), None).unwrap();
    let after_flush =
        String::from_utf8(std::fs::read(paths.session_messages_path(&id)).unwrap()).unwrap();
    assert!(after_flush.contains("\"segments\""), "rewrite must carry segments: {after_flush}");
    assert!(after_flush.contains("回复正文"));
}

#[test]
fn legacy_rows_load_with_synthesis_and_scrub() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    // Hand-write a legacy messages.jsonl: float timestamps, no segments key,
    // stray leading "…" content, a corrupt line, a pending placeholder.
    let lines = [
        // legacy assistant row: thinking + content + toolCalls, no segments,
        // FLOAT timestamp, leading "…" to be scrubbed
        r#"{"id":"m1","role":"assistant","content":"…正文内容","createdAt":1753689000000.5,"provider":"kimi","status":"done","thinking":"想一想","toolCalls":[{"toolCallId":"c1","title":"Read","status":"completed"}],"attachments":[]}"#,
        "this is not json",
        r#"{"id":"m2","role":"assistant","content":"…","createdAt":1753689001000,"provider":"kimi","status":"pending","thinking":"","toolCalls":[],"attachments":[]}"#,
        "", // empty line skipped
    ];
    std::fs::write(paths.session_messages_path(&id), lines.join("\n")).unwrap();

    store.release_session(&id);
    assert!(store.ensure_open(&id));
    let msgs = store.messages(&id).unwrap();
    assert_eq!(msgs.len(), 3, "corrupt line becomes an empty-object row");
    let m1 = &msgs[0];
    assert_eq!(m1.content, "正文内容", "leading ellipsis scrubbed");
    assert_eq!(m1.created_at, 1753689000000, "float ts parsed via f64");
    // legacy synthesis: thinking → text → tools
    let kinds: Vec<&str> = m1
        .segments
        .iter()
        .filter_map(|s| s.get("kind").and_then(Value::as_str))
        .collect();
    assert_eq!(kinds, ["thinking", "text", "tool"]);
    assert_eq!(m1.segments[2]["toolCallId"], Value::String("c1".into()));
    // corrupt line → empty-object row with defaults (old behavior keeps it)
    assert_eq!(msgs[1].id, "");
    assert_eq!(msgs[1].status, "done");
    // pending placeholder: no text segment synthesized from "…"
    let m2 = &msgs[2];
    assert_eq!(m2.status, "pending");
    assert!(m2.segments.is_empty());
}

#[test]
fn corrupt_line_defaults_status_done() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    std::fs::write(
        paths.session_messages_path(&id),
        "{\"role\":\"assistant\",\"content\":\"无状态行\"}\n",
    )
    .unwrap();
    store.release_session(&id);
    assert!(store.ensure_open(&id));
    let msgs = store.messages(&id).unwrap();
    assert_eq!(msgs[0].status, "done", "missing status defaults to done");
    assert_eq!(msgs[0].created_at, 0);
}

#[test]
fn title_hint_and_summary_rules() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    let long_user = "这是一条足够长的用户消息用来触发二十四字符标题截断的规则验证一下";
    store
        .append_message(&id, "user", long_user, "kimi", "done", &[], "")
        .unwrap();
    let meta = store.meta_for(&id).unwrap();
    assert_eq!(meta.title.chars().count(), 25, "left(24) + ellipsis");
    assert!(meta.title.ends_with('…'));
    assert!(meta.title.starts_with("这是一条"));
    assert_eq!(meta.message_count, 1);

    let long_summary: String = "摘".repeat(100);
    store
        .append_message(&id, "assistant", &long_summary, "kimi", "done", &[], "")
        .unwrap();
    let meta = store.meta_for(&id).unwrap();
    assert_eq!(meta.summary.chars().count(), 81, "left(80) + ellipsis");
    assert!(meta.summary.ends_with('…'));
}

#[test]
fn discard_empty_sessions_keeps_unreadable_meta() {
    let (_tmp, paths) = temp_paths();
    // empty session (meta readable, messageCount 0) → discarded
    let (mut store, empty_id) = make_store_with_session(&paths);
    store.release_session(&empty_id);
    // session with a message → kept
    let (_, used_id) = {
        let agent = AgentSnapshot::default();
        let id = store.create_session(&agent, "").unwrap();
        store.append_message(&id, "user", "hi", "", "done", &[], "").unwrap();
        ((), id)
    };
    store.release_session(&used_id);
    // corrupt-meta session → kept
    let corrupt_id = "corrupt-session-dir";
    let dir = paths.session_dir(corrupt_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("meta.json"), b"{not json").unwrap();

    store.discard_empty_sessions();
    assert!(!paths.session_dir(&empty_id).exists(), "empty session discarded");
    assert!(paths.session_dir(&used_id).exists(), "used session kept");
    assert!(paths.session_dir(corrupt_id).exists(), "corrupt meta kept");
}

#[test]
fn message_kind_roundtrip_and_legacy_default() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    store
        .append_message(&id, "user", "⏰ 提醒时间到：喝水", "kimi", "done", &[], "reminder")
        .unwrap();
    store.append_message(&id, "user", "hi", "kimi", "done", &[], "").unwrap();
    // 标记行落盘带 kind 键；普通行不写该键（旧格式不变）。
    let raw = std::fs::read_to_string(paths.session_messages_path(&id)).unwrap();
    let mut lines = raw.lines();
    assert!(lines.next().unwrap().contains("\"kind\":\"reminder\""));
    assert!(!lines.next().unwrap().contains("\"kind\""));
    // 重载后标记保留；无 kind 键的旧行默认空串。
    store.release_session(&id);
    assert!(store.ensure_open(&id));
    let msgs = store.messages(&id).unwrap();
    assert_eq!(msgs[0].kind, "reminder");
    assert_eq!(msgs[1].kind, "");
}

#[test]
fn lru_evicts_oldest_beyond_five() {
    let (_tmp, paths) = temp_paths();
    let mut store = SessionStore::load(paths.clone());
    let agent = AgentSnapshot::default();
    let mut ids = Vec::new();
    for _ in 0..7 {
        let id = store.create_session(&agent, "").unwrap();
        store.append_message(&id, "user", "x", "", "done", &[], "").unwrap();
        ids.push(id);
    }
    // 7 sessions touched; at most 5 resident, oldest evicted.
    assert_eq!(store.open_count(), MAX_OPEN_SESSIONS);
    assert!(!store.is_open(&ids[0]));
    assert!(!store.is_open(&ids[1]));
    assert!(store.is_open(&ids[6]));
    // Re-opening an evicted session reloads from disk.
    assert!(store.ensure_open(&ids[0]));
    assert_eq!(store.messages(&ids[0]).unwrap().len(), 1);
    assert_eq!(store.open_count(), MAX_OPEN_SESSIONS);
}

#[test]
fn flush_rewrites_empty_reply_placeholder() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    store
        .append_message(&id, "assistant", "…", "kimi", "pending", &[], "")
        .unwrap();
    store.flush_last_assistant(&id, Some("done"), None).unwrap();
    let msgs = store.messages(&id).unwrap();
    assert_eq!(msgs[0].content, "（空回复）");
}

#[test]
fn upsert_tool_merges_non_null_and_forces_kind() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    store
        .append_message(&id, "assistant", "…", "kimi", "pending", &[], "")
        .unwrap();
    let mut t1 = Map::new();
    t1.insert("toolCallId".into(), Value::String("c1".into()));
    t1.insert("title".into(), Value::String("Read".into()));
    t1.insert("status".into(), Value::String("in_progress".into()));
    assert!(store.upsert_last_assistant_tool(&id, &t1));
    // null fields do NOT overwrite; kind forced to "tool" in the segment
    let mut t2 = Map::new();
    t2.insert("toolCallId".into(), Value::String("c1".into()));
    t2.insert("title".into(), Value::Null);
    t2.insert("status".into(), Value::String("completed".into()));
    t2.insert("kind".into(), Value::String("read".into()));
    assert!(store.upsert_last_assistant_tool(&id, &t2));

    let msgs = store.messages(&id).unwrap();
    let row = &msgs[0];
    assert_eq!(row.tool_calls.len(), 1);
    assert_eq!(row.tool_calls[0]["title"], Value::String("Read".into()));
    assert_eq!(row.tool_calls[0]["status"], Value::String("completed".into()));
    let tool_seg = row
        .segments
        .iter()
        .find(|s| s.get("toolCallId").and_then(Value::as_str) == Some("c1"))
        .unwrap();
    assert_eq!(tool_seg["kind"], Value::String("tool".into()), "segment kind forced to tool");
    assert_eq!(row.status, "streaming");
    // empty toolCallId drops the update
    let mut bad = Map::new();
    bad.insert("toolCallId".into(), Value::String("".into()));
    assert!(!store.upsert_last_assistant_tool(&id, &bad));
}

#[test]
fn workspace_path_priority() {
    let (_tmp, paths) = temp_paths();
    let (mut store, id) = make_store_with_session(&paths);
    // projectless session → sessions/<id>/workspace
    let ws = store.workspace_path_for(&id);
    assert!(ws.contains("workspace"), "projectless → app workspace: {ws}");

    // project session → projectDir
    let proj = tempfile::tempdir().unwrap();
    let id2 = store
        .create_session(&AgentSnapshot::default(), proj.path().to_str().unwrap())
        .unwrap();
    let ws2 = store.workspace_path_for(&id2);
    assert_eq!(ws2, canonical_dir(proj.path().to_str().unwrap()));
}

#[test]
fn search_hits_snippets_and_title_only() {
    let (_tmp, paths) = temp_paths();
    let mut store = SessionStore::load(paths.clone());
    let agent = AgentSnapshot::default();
    let s1 = store.create_session(&agent, "").unwrap();
    for i in 0..5 {
        store
            .append_message(&s1, "user", &format!("目标关键词 第{i}条"), "kimi", "done", &[], "")
            .unwrap();
    }
    // pending placeholder must not match
    store
        .append_message(&s1, "assistant", "…", "kimi", "pending", &[], "")
        .unwrap();
    let s2 = store.create_session(&agent, "").unwrap();
    store.rename_session(&s2, "关键词标题").unwrap();

    let engine = SearchEngine::new();
    let targets = store.search_targets();
    let outcome = engine.search(&targets, "关键词", 50);
    let SearchOutcome::Done { results, .. } = outcome else {
        panic!("expected Done");
    };
    // s1: 5 content hits, capped to 3 delivered, newest first
    let content_hits: Vec<_> = results.iter().filter(|r| !r.title_only).collect();
    assert_eq!(content_hits.len(), 3);
    assert_eq!(content_hits[0].hit_count, 5, "full count reported");
    assert!(content_hits[0].snippet.contains("第4条"), "newest hit first");
    // s2: title-only
    let title_hits: Vec<_> = results.iter().filter(|r| r.title_only).collect();
    assert_eq!(title_hits.len(), 1);
    assert_eq!(title_hits[0].session_id, s2);
    assert_eq!(title_hits[0].hit_count, 0);
}

#[test]
fn search_generation_supersedes() {
    let engine = SearchEngine::new();
    let first = engine.search(&[], "q", 50);
    let SearchOutcome::Done { generation: g1, .. } = first else {
        panic!();
    };
    // second call bumps the generation
    let second = engine.search(&[], "q2", 50);
    let SearchOutcome::Done { generation: g2, .. } = second else {
        panic!();
    };
    assert!(g2 > g1);
    assert!(!engine.is_current(g1));
    assert!(engine.is_current(g2));
    // empty query still bumps the generation (cancel semantics)
    let third = engine.search(&[], "   ", 50);
    let SearchOutcome::Done { generation: g3, results } = third else {
        panic!();
    };
    assert!(results.is_empty());
    assert!(!engine.is_current(g2));
    assert!(engine.is_current(g3));
}

#[test]
fn search_snippet_context_marks() {
    let (_tmp, paths) = temp_paths();
    let mut store = SessionStore::load(paths.clone());
    let id = store
        .create_session(&AgentSnapshot::default(), "")
        .unwrap();
    let long = format!("{}NEEDLE{}", "前".repeat(100), "后".repeat(100));
    store
        .append_message(&id, "user", &long, "kimi", "done", &[], "")
        .unwrap();
    let engine = SearchEngine::new();
    let SearchOutcome::Done { results, .. } = engine.search(&store.search_targets(), "needle", 50)
    else {
        panic!();
    };
    assert_eq!(results.len(), 1);
    let snippet = &results[0].snippet;
    assert!(snippet.starts_with('…') && snippet.ends_with('…'));
    assert!(snippet.contains("NEEDLE"));
    // 40 chars of context each side
    assert_eq!(snippet.chars().count(), 2 + 40 + 6 + 40);
}

#[test]
fn case_insensitive_search_matches() {
    let (_tmp, paths) = temp_paths();
    let mut store = SessionStore::load(paths.clone());
    let id = store
        .create_session(&AgentSnapshot::default(), "")
        .unwrap();
    store
        .append_message(&id, "user", "Hello World", "kimi", "done", &[], "")
        .unwrap();
    let engine = SearchEngine::new();
    let SearchOutcome::Done { results, .. } =
        engine.search(&store.search_targets(), "hello", 50)
    else {
        panic!();
    };
    assert_eq!(results.len(), 1);
}
