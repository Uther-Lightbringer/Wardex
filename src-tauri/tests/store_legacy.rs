// Legacy-format compatibility tests driven by fixture files captured in the
// old (C++/Qt) on-disk shape: float timestamps, missing optional keys,
// appended rows without `segments`, stray leading "…" content, unknown
// fields that must pass through untouched.

use std::fs;

use wardex_lib::store::sessions::{load_messages, SessionStore};
use wardex_lib::store::{AgentStore, Paths};

fn fixture(name: &str) -> Vec<u8> {
    fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn temp_paths() -> (tempfile::TempDir, Paths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::new(tmp.path().to_path_buf());
    (tmp, paths)
}

const SESSION_ID: &str = "3f2c9a1e-7b4d-4e8f-9c0a-1d2e3f4a5b6c";

fn plant_session(paths: &Paths) {
    let dir = paths.session_dir(SESSION_ID);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("meta.json"), fixture("legacy_meta.json")).unwrap();
    fs::write(dir.join("messages.jsonl"), fixture("legacy_messages.jsonl")).unwrap();
}

#[test]
fn legacy_meta_loads_with_float_ts_and_defaults() {
    let (_tmp, paths) = temp_paths();
    plant_session(&paths);
    let mut store = SessionStore::load(paths.clone());

    // Index scan picked the session up (readable meta), sorted by updatedAt.
    assert_eq!(store.list().len(), 1);
    let row = &store.list()[0];
    assert_eq!(row.id, SESSION_ID);
    assert_eq!(row.updated_at, 1753690000000, "float ms parsed via f64");
    assert!(!row.pinned, "absent pinned key defaults to false");

    let meta = store.meta_for(SESSION_ID).unwrap();
    assert_eq!(meta.title, "看下这张图里的布局问题");
    assert_eq!(meta.acp_session_id, None, "absent acpSessionId stays unset");
    assert!(meta.extra.contains_key("unknownFutureField"));
}

#[test]
fn legacy_meta_roundtrip_keeps_unknown_field_and_writes_pinned() {
    let (_tmp, paths) = temp_paths();
    plant_session(&paths);
    let mut store = SessionStore::load(paths.clone());

    store.set_session_pinned(SESSION_ID, true).unwrap();
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.session_meta_path(SESSION_ID)).unwrap()).unwrap();
    assert_eq!(raw["pinned"], serde_json::json!(true));
    assert_eq!(
        raw["unknownFutureField"],
        serde_json::json!({"keep": true, "nested": [1, 2, 3]}),
        "unknown keys survive rewrites"
    );
    // pin must not bump updatedAt; the float shape is normalized to the
    // integer form on write (§0: 写盘用整数形态的 number)
    assert_eq!(raw["updatedAt"], serde_json::json!(1753690000000i64));
}

#[test]
fn legacy_messages_load() {
    let (_tmp, paths) = temp_paths();
    plant_session(&paths);
    let msgs = load_messages(&paths, SESSION_ID);
    assert_eq!(msgs.len(), 3);

    // Row 1: appended user row — no segments on disk, synthesized on load.
    let m1 = &msgs[0];
    assert_eq!(m1.role, "user");
    assert_eq!(m1.attachments.len(), 1, "attachments passed through verbatim");
    assert!(m1.attachments[0].ends_with(".png"));
    assert_eq!(m1.segments.len(), 1);
    assert_eq!(m1.segments[0]["kind"], serde_json::json!("text"));

    // Row 2: stray leading "…" scrubbed; float timestamp parsed.
    let m2 = &msgs[1];
    assert_eq!(m2.content, "我先读一下文件。", "leading ellipsis scrubbed");
    assert_eq!(m2.created_at, 1753689005000);

    // Row 3: rewritten row keeps its own segments verbatim (no synthesis).
    let m3 = &msgs[2];
    let kinds: Vec<&str> = m3
        .segments
        .iter()
        .filter_map(|s| s.get("kind").and_then(serde_json::Value::as_str))
        .collect();
    assert_eq!(kinds, ["thinking", "text", "tool"]);
}

#[test]
fn set_session_project_binds_and_preserves_updated_at() {
    let (_tmp, paths) = temp_paths();
    plant_session(&paths);
    let mut store = SessionStore::load(paths.clone());

    // Binding to a missing dir is refused.
    let missing = _tmp.path().join("no-such-dir").to_string_lossy().into_owned();
    assert!(store.set_session_project(SESSION_ID, &missing).is_err());

    // Bind to a real dir: meta + index row updated, updatedAt untouched.
    let proj = tempfile::tempdir().unwrap();
    let proj_str = proj.path().to_string_lossy().into_owned();
    let want = wardex_lib::store::canonical_dir(&proj_str);
    assert!(store.set_session_project(SESSION_ID, &proj_str).unwrap());

    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.session_meta_path(SESSION_ID)).unwrap()).unwrap();
    assert_eq!(raw["projectDir"], serde_json::json!(want));
    assert_eq!(raw["workDir"], serde_json::json!(want));
    assert_eq!(raw["updatedAt"], serde_json::json!(1753690000000i64), "bind must not bump updatedAt");
    assert_eq!(store.list()[0].project_dir, want);
}

#[test]
fn legacy_agent_file_backfills_cli_path_and_default() {
    let (_tmp, paths) = temp_paths();
    fs::create_dir_all(paths.agents_dir()).unwrap();
    // No index.json at all: the agent file is an orphan and must be picked
    // up; defaultAgentId empty → first chat-capable (kimi) agent selected.
    fs::write(
        paths.agent_file_path("8a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
        fixture("legacy_agent.json"),
    )
    .unwrap();

    let store = AgentStore::load(&paths);
    assert_eq!(store.agents().len(), 1, "orphan agent file picked up");
    let agent = &store.agents()[0];
    assert_eq!(agent.cli_path, "kimi", "missing cliPath backfilled to \"kimi\"");
    assert_eq!(agent.provider, "kimi");
    assert!(agent.enabled, "missing/absent enabled defaults true");
    assert_eq!(store.default_agent_id(), agent.id, "kimi agent auto-selected as default");
    assert!(agent.is_default, "isDefault forced from the index rule, not the file");
}

#[test]
fn api_key_mask_protection_on_update() {
    let (_tmp, paths) = temp_paths();
    fs::create_dir_all(paths.agents_dir()).unwrap();
    fs::write(
        paths.agent_file_path("8a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
        fixture("legacy_agent.json"),
    )
    .unwrap();
    let mut store = AgentStore::load(&paths);
    let id = store.agents()[0].id.clone();

    // Masked write-back (what the UI sends) must NOT clobber the real key.
    let masked = wardex_lib::store::mask_key("sk-legacykey1234567890");
    assert_eq!(masked, "sk-****7890");
    let patch = wardex_lib::store::AgentPatch {
        api_key: Some(masked),
        ..Default::default()
    };
    store.update_agent(&paths, &id, &patch).unwrap();
    assert_eq!(store.get(&id).unwrap().api_key, "sk-legacykey1234567890");

    // Empty string also keeps the old value.
    let patch = wardex_lib::store::AgentPatch {
        api_key: Some(String::new()),
        ..Default::default()
    };
    store.update_agent(&paths, &id, &patch).unwrap();
    assert_eq!(store.get(&id).unwrap().api_key, "sk-legacykey1234567890");

    // A real new key is applied.
    let patch = wardex_lib::store::AgentPatch {
        api_key: Some("sk-newkey999".to_string()),
        ..Default::default()
    };
    store.update_agent(&paths, &id, &patch).unwrap();
    assert_eq!(store.get(&id).unwrap().api_key, "sk-newkey999");
    // …and persisted to the agent file.
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.agent_file_path(&id)).unwrap()).unwrap();
    assert_eq!(raw["apiKey"], serde_json::json!("sk-newkey999"));
}
