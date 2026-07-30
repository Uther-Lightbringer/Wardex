// Domain behavior tests: projects / prefs / prompts / workspace / agents.
// Each store is pointed at an isolated temp data root via Paths::new.

use std::fs;

use wardex_lib::store::media;
use wardex_lib::store::workspace;
use wardex_lib::store::{
    AgentStore, Paths, ProjectStore, PromptStore, UserPrefs,
};

fn temp_paths() -> (tempfile::TempDir, Paths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::new(tmp.path().to_path_buf());
    (tmp, paths)
}

// ---- projects ----

#[test]
fn projects_dedupe_case_insensitive_and_cap_8() {
    let (_tmp, paths) = temp_paths();
    let mut store = ProjectStore::load(&paths);
    store.touch_project(&paths, "C:/ws/Alpha").unwrap();
    store.touch_project(&paths, "c:/ws/alpha/").unwrap(); // same dir, other case
    assert_eq!(store.recent().len(), 1, "case-insensitive dedupe");
    // canonical form: forward slashes, no trailing slash
    assert_eq!(store.recent()[0].path, "c:/ws/alpha");

    for i in 0..10 {
        store.touch_project(&paths, &format!("C:/ws/p{i}")).unwrap();
    }
    assert_eq!(store.recent().len(), 8, "recent capped at 8");
    assert_eq!(store.recent()[0].path, "C:/ws/p9");
}

#[test]
fn projects_alias_rules_and_case_sensitive_keys_quirk() {
    let (_tmp, paths) = temp_paths();
    let mut store = ProjectStore::load(&paths);

    // trim + left(24)
    let long_alias = "别".repeat(30);
    store.set_alias(&paths, "C:/ws/Foo", &format!("  {long_alias}  ")).unwrap();
    assert_eq!(store.aliases()["C:/ws/Foo"].chars().count(), 24);

    // LEGACY QUIRK: alias keys are case-SENSITIVE (old QHash exact match) —
    // same directory with different casing yields two keys. Keep it.
    store.set_alias(&paths, "c:/ws/foo", "lower").unwrap();
    assert_eq!(store.aliases().len(), 2);
    assert_eq!(store.aliases()["c:/ws/foo"], "lower");

    // empty alias removes the key
    store.set_alias(&paths, "c:/ws/foo", "   ").unwrap();
    assert!(!store.aliases().contains_key("c:/ws/foo"));

    // display fallback chain
    assert_eq!(store.display_name_for("C:/ws/Foo"), "别".repeat(24));
    assert_eq!(store.display_name_for("C:/ws/Bar"), "Bar");
    assert_eq!(store.display_name_for("C:/"), "C:\\", "drive root → native path");

    // persisted + reloaded; empty alias values dropped on load
    let reloaded = ProjectStore::load(&paths);
    assert_eq!(reloaded.aliases().len(), 1);
}

// ---- prefs ----

#[test]
fn prefs_clamps_and_defaults() {
    let (_tmp, paths) = temp_paths();
    // missing file → defaults
    let prefs = UserPrefs::load(&paths);
    assert_eq!(prefs.permission_mode(), "default");
    assert_eq!(prefs.user_name(), "阿尔萨斯", "empty stored name falls back");
    assert_eq!(prefs.preview_width(), 0);
    assert_eq!(prefs.preview_height(), 0);
    assert!((prefs.font_scale() - 1.0).abs() < f64::EPSILON);

    // out-of-range file values are clamped on load
    fs::write(
        paths.user_prefs_path(),
        br#"{"fontScale": 5.0, "previewWidth": 100, "previewHeight": -3, "permissionMode": "yolo"}"#,
    )
    .unwrap();
    let prefs = UserPrefs::load(&paths);
    assert!((prefs.font_scale() - 1.30).abs() < f64::EPSILON);
    assert_eq!(prefs.preview_width(), 320, "nonzero clamps into [320,4096]");
    assert_eq!(prefs.preview_height(), 0, "<=0 stays 0 (unset)");
    assert_eq!(prefs.permission_mode(), "yolo", "load does NOT whitelist (setter does)");

    let mut prefs = UserPrefs::load(&paths);
    prefs.set_permission_mode(&paths, "bogus").unwrap();
    assert_eq!(prefs.permission_mode(), "default", "setter whitelist fallback");
    prefs.set_font_scale(&paths, 0.5).unwrap();
    assert!((prefs.font_scale() - 0.85).abs() < f64::EPSILON);
    prefs
        .set_user_name(&paths, "  这是一个非常非常非常非常非常非常长的用户名三十字  ")
        .unwrap();
    assert_eq!(prefs.user_name().chars().count(), 24);
}

#[test]
fn prefs_panel_layout_tolerates_missing_and_roundtrips() {
    let (_tmp, paths) = temp_paths();
    // Old file without panelLayout loads fine.
    fs::write(paths.user_prefs_path(), br#"{"userName": "x"}"#).unwrap();
    let mut prefs = UserPrefs::load(&paths);
    assert!(prefs.panel_layout().is_empty());
    assert!(prefs.panel_layout_for("git").is_none());

    prefs
        .set_panel_layout(
            &paths,
            "git",
            &wardex_lib::store::PanelLayoutEntry {
                open: Some(true),
                height: Some(220),
                order: None,
                extra: Default::default(),
            },
        )
        .unwrap();
    let reloaded = UserPrefs::load(&paths);
    let entry = reloaded.panel_layout_for("git").unwrap();
    assert_eq!(entry.open, Some(true));
    assert_eq!(entry.height, Some(220));
    assert_eq!(entry.order, None);
    // the file now carries the new field alongside the old ones
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.user_prefs_path()).unwrap()).unwrap();
    assert_eq!(raw["userName"], serde_json::json!("x"));
    assert!(raw["panelLayout"]["git"]["open"].as_bool().unwrap());
}

// ---- prompts ----

#[test]
fn prompts_seed_verbatim_and_no_reseed_after_delete_all() {
    let (_tmp, paths) = temp_paths();
    let mut store = PromptStore::load(&paths);
    assert_eq!(store.rows().len(), 3, "first launch seeds 3 templates");
    let expected = [
        ("代码审查", "请审查以下代码，指出潜在的 bug、边界条件问题和可改进点，并给出具体的修改建议：\n"),
        ("解释代码", "请逐段解释以下代码的作用、实现思路和关键细节：\n"),
        ("重构建议", "请分析以下代码的结构，在保持行为不变的前提下给出具体的重构方案：\n"),
    ];
    for (row, (name, text)) in store.rows().iter().zip(expected) {
        assert_eq!(row.name, name);
        assert_eq!(row.text, text, "verbatim seed text incl. trailing \\n");
    }
    assert!(paths.prompts_path().exists(), "seed is persisted immediately");

    // User deletes everything → file exists with an empty array → NO reseed.
    let ids: Vec<String> = store.rows().iter().map(|r| r.id.clone()).collect();
    for id in ids {
        store.remove(&paths, &id).unwrap();
    }
    let reloaded = PromptStore::load(&paths);
    assert!(reloaded.rows().is_empty(), "deleting all templates is a deliberate choice");

    // name fallback: first line of text, left(20)
    let long_text = format!("{}\n第二行", "名".repeat(30));
    let mut store = reloaded;
    store.add(&paths, "", &long_text).unwrap();
    assert_eq!(store.rows()[0].name.chars().count(), 20);
    // empty text ignored
    store.add(&paths, "x", "   ").unwrap();
    assert_eq!(store.rows().len(), 1);
}

// ---- agents ----

#[test]
fn agents_default_transitions() {
    let (_tmp, paths) = temp_paths();
    let mut store = AgentStore::load(&paths);
    let a1 = store.create_agent(&paths, "").unwrap();
    assert_eq!(store.get(&a1).unwrap().name, "新 Agent");
    assert_eq!(store.get(&a1).unwrap().model, "moonshot-v1-auto");
    assert_eq!(store.get(&a1).unwrap().cli_path, "", "new agent cliPath is empty in memory");
    assert_eq!(store.default_agent_id(), a1, "first agent becomes default");

    let a2 = store.create_agent(&paths, "副手").unwrap();
    assert_eq!(store.default_agent_id(), a1);
    // removing the default → list's first takes over
    store.remove_agent(&paths, &a1).unwrap();
    assert_eq!(store.default_agent_id(), a2);
    assert!(store.get(&a2).unwrap().is_default);
    assert!(!paths.agent_file_path(&a1).exists(), "agent file deleted");

    // all registered providers are chat-capable → claude can be default
    store
        .update_agent(
            &paths,
            &a2,
            &wardex_lib::store::AgentPatch {
                provider: Some("Claude".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(store.get(&a2).unwrap().provider, "claude", "provider lowercased");
    store.set_default(&paths, &a2).unwrap();
    assert_eq!(store.default_agent_id(), a2);

    // unregistered provider still cannot be default
    store
        .update_agent(
            &paths,
            &a2,
            &wardex_lib::store::AgentPatch {
                provider: Some("unknown-llm".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let err = store.set_default(&paths, &a2).unwrap_err();
    assert!(err.to_string().contains("不支持对话"));
}

// ---- workspace ----

fn write_file(dir: &std::path::Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

#[test]
fn workspace_file_list_dfs_ignore_and_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_file(root, "src/main.rs", b"fn main() {}");
    write_file(root, "src/lib.rs", b"// lib");
    write_file(root, "node_modules/pkg/index.js", b"x");
    write_file(root, ".git/config", b"x");
    write_file(root, "build/out.o", b"x");
    write_file(root, "docs/logo.png", b"\x89PNG");
    write_file(root, "README.md", b"# hi");

    let all = workspace::workspace_file_list(root, "", 200);
    assert!(all.contains(&"src/main.rs".to_string()));
    assert!(all.contains(&"README.md".to_string()));
    assert!(!all.iter().any(|p| p.contains("node_modules")), "ignored dir not descended");
    assert!(!all.iter().any(|p| p.starts_with(".git")), ".git ignored");
    assert!(!all.iter().any(|p| p.starts_with("build/")), "build ignored");
    assert!(!all.iter().any(|p| p.ends_with(".png")), "images skipped");

    let filtered = workspace::workspace_file_list(root, "LIB", 200);
    assert_eq!(filtered, vec!["src/lib.rs".to_string()], "case-insensitive substring filter");

    let capped = workspace::workspace_file_list(root, "", 2);
    assert_eq!(capped.len(), 2, "maxResults cap");
}

#[test]
fn read_file_range_line_semantics() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let root_str = root.to_string_lossy().replace('\\', "/");
    write_file(root, "a.txt", "l1\r\nl2\r\nl3\nl4\n".as_bytes());

    // whole file, 1-based, \r stripped; the trailing \n leaves a final
    // empty line (same as Qt's QString::split, which keeps empty parts)
    let ok = workspace::read_file_range(&root_str, "a.txt", 0, 0).unwrap();
    assert_eq!(ok.total_lines, 5);
    assert_eq!(ok.lines.len(), 5);
    assert_eq!(ok.lines[0].text, "l1", "trailing \\r stripped");
    assert_eq!(ok.lines[4].text, "");
    assert!(!ok.truncated);

    // single line
    let ok = workspace::read_file_range(&root_str, "a.txt", 3, 0).unwrap();
    assert_eq!(ok.lines.len(), 1);
    assert_eq!(ok.lines[0].n, 3);
    assert_eq!(ok.lines[0].text, "l3");

    // to < from clamps to from
    let ok = workspace::read_file_range(&root_str, "a.txt", 2, 1).unwrap();
    assert_eq!(ok.lines.len(), 1);
    assert_eq!(ok.lines[0].n, 2);

    // from beyond total → range error carrying totalLines
    let err = workspace::read_file_range(&root_str, "a.txt", 99, 0).unwrap_err();
    assert_eq!(err.error, "range");
    assert_eq!(err.total_lines, Some(5));

    // missing file / empty root
    assert_eq!(
        workspace::read_file_range(&root_str, "nope.txt", 0, 0).unwrap_err().error,
        "missing"
    );
    assert_eq!(
        workspace::read_file_range("", "a.txt", 0, 0).unwrap_err().error,
        "missing"
    );
}

#[test]
fn read_file_range_escape_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let root_str = root.to_string_lossy().replace('\\', "/");
    write_file(root, "a.txt", b"x");

    for evil in ["../outside.txt", "..\\..\\win.ini", "C:/Windows/win.ini", "/abs.txt", "sub/../../outside.txt"] {
        let err = workspace::read_file_range(&root_str, evil, 0, 0).unwrap_err();
        assert_eq!(err.error, "escape", "{evil} must be refused");
    }
    // root itself resolves under the root → then "missing" (a dir, not a file)
    let err = workspace::read_file_range(&root_str, ".", 0, 0).unwrap_err();
    assert_eq!(err.error, "missing");
}

#[test]
fn read_file_range_200kb_cap_drops_half_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let root_str = root.to_string_lossy().replace('\\', "/");
    // 100-byte lines; 2100 lines ≈ 210KB → the tail line inside the 200KB
    // read window is cut mid-way and must be dropped.
    let line = "x".repeat(99);
    let mut content = String::new();
    for _ in 0..2100 {
        content.push_str(&line);
        content.push('\n');
    }
    write_file(root, "big.txt", content.as_bytes());

    let ok = workspace::read_file_range(&root_str, "big.txt", 0, 0).unwrap();
    assert!(ok.truncated);
    let head_lines = (workspace::MAX_REF_BYTES / 100) as i64; // full lines in window
    assert_eq!(ok.total_lines, head_lines, "cut half line dropped");
    assert_eq!(ok.lines.last().unwrap().text, line);
}

#[test]
fn read_file_range_gbk_fallback_and_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let root_str = root.to_string_lossy().replace('\\', "/");

    // GBK-encoded Chinese file (no BOM): UTF-8 decode yields U+FFFD → GBK retry.
    let (gbk, _, had_unmappable) = encoding_rs::GBK.encode("第一行中文\n第二行");
    assert!(!had_unmappable);
    write_file(root, "gbk.txt", &gbk);
    let ok = workspace::read_file_range(&root_str, "gbk.txt", 0, 0).unwrap();
    assert_eq!(ok.lines[0].text, "第一行中文");
    assert_eq!(ok.lines[1].text, "第二行");

    // NUL byte → binary, even with a texty extension
    write_file(root, "bin.txt", b"abc\0def");
    assert_eq!(
        workspace::read_file_range(&root_str, "bin.txt", 0, 0).unwrap_err().error,
        "binary"
    );
    // known binary extension short-circuits
    write_file(root, "x.zip", b"PK\x03\x04");
    assert_eq!(
        workspace::read_file_range(&root_str, "x.zip", 0, 0).unwrap_err().error,
        "binary"
    );
}

#[test]
fn preview_and_save_preview_text() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let txt = root.join("note.txt");
    fs::write(&txt, "可编辑文本").unwrap();

    let ok = workspace::preview_file(txt.to_str().unwrap());
    assert!(ok.ok);
    assert_eq!(ok.image, Some(false));
    assert_eq!(ok.text.as_deref(), Some("可编辑文本"));
    assert_eq!(ok.truncated, Some(false));

    // missing / image / binary vocabularies
    assert_eq!(workspace::preview_file(root.join("no.txt").to_str().unwrap()).reason.as_deref(), Some("missing"));
    let png = root.join("i.png");
    fs::write(&png, b"\x89PNG").unwrap();
    let img = workspace::preview_file(png.to_str().unwrap());
    assert!(img.ok && img.image == Some(true) && img.text.is_none(), "images carry no text");
    let zip = root.join("z.zip");
    fs::write(&zip, b"PK").unwrap();
    assert_eq!(workspace::preview_file(zip.to_str().unwrap()).reason.as_deref(), Some("binary"));

    // savePreviewText: Chinese user-facing errors, UTF-8 overwrite
    let saved = workspace::save_preview_text(txt.to_str().unwrap(), "新内容");
    assert!(saved.ok);
    assert_eq!(fs::read_to_string(&txt).unwrap(), "新内容");
    assert_eq!(
        workspace::save_preview_text(root.join("gone.txt").to_str().unwrap(), "x").error.as_deref(),
        Some("文件不存在")
    );
    assert_eq!(
        workspace::save_preview_text(zip.to_str().unwrap(), "x").error.as_deref(),
        Some("二进制文件不可编辑")
    );
    let nul = root.join("n.dat2");
    fs::write(&nul, b"ab\0cd").unwrap();
    assert_eq!(
        workspace::save_preview_text(nul.to_str().unwrap(), "x").error.as_deref(),
        Some("二进制文件不可编辑"),
        "NUL sniff in first 4096 bytes"
    );
}

#[test]
fn git_branch_for_forms() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // not a repo
    assert_eq!(workspace::git_branch_for(root.to_str().unwrap()), "");

    // normal repo
    let git = root.join(".git");
    fs::create_dir_all(&git).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/feature/xyz\n").unwrap();
    assert_eq!(workspace::git_branch_for(root.to_str().unwrap()), "feature/xyz");

    // other ref form
    fs::write(git.join("HEAD"), "ref: refs/tags/v1").unwrap();
    assert_eq!(workspace::git_branch_for(root.to_str().unwrap()), "refs/tags/v1");

    // detached → 7-char short SHA
    fs::write(git.join("HEAD"), "0123456789abcdef").unwrap();
    assert_eq!(workspace::git_branch_for(root.to_str().unwrap()), "0123456");

    // worktree gitfile: .git is a FILE pointing elsewhere
    let tmp2 = tempfile::tempdir().unwrap();
    let wt = tmp2.path();
    let real_git = wt.join("real-gitdir");
    fs::create_dir_all(&real_git).unwrap();
    fs::write(real_git.join("HEAD"), "ref: refs/heads/wtbranch").unwrap();
    fs::write(wt.join(".git"), "gitdir: real-gitdir").unwrap();
    assert_eq!(workspace::git_branch_for(wt.to_str().unwrap()), "wtbranch");
}

// ---- media clear cache ----

#[test]
fn clear_media_cache_wipes_tree() {
    let (_tmp, paths) = temp_paths();
    assert!(!media::clear_media_cache(&paths), "nothing to clear → false");
    fs::create_dir_all(paths.media_root().join("2026-07-29/s")).unwrap();
    assert!(media::clear_media_cache(&paths));
    assert!(!paths.media_root().exists());
}
