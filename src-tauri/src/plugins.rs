// Plugin registry (插件化改造 P0): everything the user can hot-manage lives
// under <data root>/wardex-plugins/ with a single registry.json as the source
// of truth for enabled flags.
//
// Two plugin kinds:
//   tool — a pi extension entry file (main.ts) injected via --extension at
//          spawn; "生效" restarts live runtimes so they pick up new files.
//   ui   — an optional panel.html rendered in a sandboxed iframe inside the
//          chat side dock (WarDock), plus optional tool entry.
//
// Built-ins: the repo-shipped pi-extensions (wardex-reminders.ts /
// wardex-codegraph.ts / wardex-plugins manager) surface in the same registry
// so the settings page manages one uniform list. Built-in files cannot be
// deleted, only toggled.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chat::pi::locate_extensions_dir;
use crate::store::Paths;

/// User-visible plugin descriptor (camelCase over the wire for the Vue side).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// "tool" | "ui" | "tool+ui"
    #[serde(default)]
    pub kind: String,
    pub enabled: bool,
    /// Repo/bundled extension — cannot be deleted from the settings page.
    pub builtin: bool,
    /// Absolute path of the pi extension entry (.ts), empty when none.
    #[serde(default)]
    pub entry: String,
    /// Absolute path of the UI panel html, empty when none.
    #[serde(default)]
    pub ui: String,
    /// Plugin directory (user plugins only; builtins point at pi-extensions).
    #[serde(default)]
    pub dir: String,
    /// UI panel surface: "drawer" (default) | "dialog" | "both".
    #[serde(default)]
    pub surface: String,
    /// Data scope declared by the plugin: "session" (in-memory, dies with
    /// the session) | "project" (<project>/.wardex/) | "global"
    /// (data-root-wide). Default "project".
    #[serde(default)]
    pub data_scope: String,
}

impl PluginInfo {
    fn has_tool(&self) -> bool {
        !self.entry.is_empty()
    }
}

/// <data root>/wardex-plugins/
pub fn plugins_root(paths: &Paths) -> PathBuf {
    paths.root().join("wardex-plugins")
}

fn registry_path(paths: &Paths) -> PathBuf {
    plugins_root(paths).join("registry.json")
}

/// id → enabled overrides persisted across scans. Unknown ids are dropped on
/// save; new discoveries default to enabled = true.
type Registry = std::collections::BTreeMap<String, bool>;

fn load_registry(paths: &Paths) -> Registry {
    let mut map = Registry::new();
    let Ok(raw) = fs::read_to_string(registry_path(paths)) else {
        return map;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return map;
    };
    if let Some(obj) = v.get("enabled").and_then(|e| e.as_object()) {
        for (k, val) in obj {
            map.insert(k.clone(), val.as_bool().unwrap_or(true));
        }
    }
    map
}

fn save_registry(paths: &Paths, reg: &Registry) -> Result<(), String> {
    let root = plugins_root(paths);
    fs::create_dir_all(&root).map_err(|e| format!("创建插件目录失败: {e}"))?;
    let enabled: serde_json::Map<String, serde_json::Value> =
        reg.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let body = json!({ "version": 1, "enabled": enabled });
    let tmp = registry_path(paths).with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap_or_default())
        .map_err(|e| format!("写插件注册表失败: {e}"))?;
    fs::rename(&tmp, registry_path(paths)).map_err(|e| format!("提交插件注册表失败: {e}"))?;
    Ok(())
}

/// Read one user plugin directory (must contain plugin.json).
fn read_user_plugin(dir: &Path) -> Option<PluginInfo> {
    let manifest_path = dir.join("plugin.json");
    let raw = fs::read_to_string(&manifest_path).ok()?;
    let m: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let id = dir.file_name()?.to_string_lossy().into_owned();
    let name = m
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();
    let version = m
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    let rel_entry = m.get("entry").and_then(|v| v.as_str()).unwrap_or("");
    let rel_ui = m.get("ui").and_then(|v| v.as_str()).unwrap_or("");
    // Drawer (side dock tab) / dialog (floating window) / both.
    let surface = match m.get("surface").and_then(|v| v.as_str()) {
        Some("dialog") => "dialog",
        Some("both") => "both",
        _ => "drawer",
    }
    .to_string();
    let data_scope = match m.get("data").and_then(|d| d.get("scope")).and_then(|v| v.as_str()) {
        Some("global") => "global",
        Some("session") => "session",
        _ => "project", // default
    }
    .to_string();
    let entry = if rel_entry.trim().is_empty() {
        String::new()
    } else {
        dir.join(rel_entry.trim()).to_string_lossy().into_owned()
    };
    let ui = if rel_ui.trim().is_empty() {
        String::new()
    } else {
        dir.join(rel_ui.trim()).to_string_lossy().into_owned()
    };
    let kind = match (!entry.is_empty(), !ui.is_empty()) {
        (true, true) => "tool+ui",
        (false, true) => "ui",
        _ => "tool",
    };
    Some(PluginInfo {
        id,
        name,
        version,
        kind: kind.to_string(),
        enabled: true,
        builtin: false,
        entry,
        ui,
        dir: dir.to_string_lossy().into_owned(),
        surface,
        data_scope,
    })
}

/// Full scan: builtins (repo pi-extensions) ∪ user plugin dirs, merged with
/// persisted enabled flags. Deterministic order: builtins first, then user
/// plugins sorted by name.
pub fn scan(paths: &Paths) -> Vec<PluginInfo> {
    let reg = load_registry(paths);
    let mut out: Vec<PluginInfo> = Vec::new();

    // ---- builtins from locate_extensions_dir() ----
    if let Some(dir) = locate_extensions_dir() {
        let push_builtin = |out: &mut Vec<PluginInfo>, file: &str, id: &str, name: &str| {
            let p = dir.join(file);
            if p.is_file() {
                out.push(PluginInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    kind: "tool".to_string(),
                    enabled: true,
                    builtin: true,
                    entry: p.to_string_lossy().into_owned(),
                    ui: String::new(),
                    dir: dir.to_string_lossy().into_owned(),
                    surface: String::new(),
                    data_scope: String::new(),
                });
            }
        };
        push_builtin(&mut out, "wardex-reminders.ts", "reminders", "会话提醒");
        push_builtin(&mut out, "wardex-codegraph.ts", "codegraph", "Codegraph 代码索引");
        push_builtin(&mut out, "wardex-plugins.ts", "plugins", "插件管理员");
    }

    // Native UI panels shipped with the app (待办/后台任务/数据库) surface in
    // the SAME registry as default-installed system plugins: toggleable,
    // undeletable. They have no extension entry / panel.html — the frontend
    // maps these ids to its compiled Vue components (panels/registry.ts).
    let push_native = |out: &mut Vec<PluginInfo>, id: &str, name: &str| {
        out.push(PluginInfo {
            id: id.to_string(),
            name: name.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            kind: "ui".to_string(),
            enabled: true,
            builtin: true,
            entry: String::new(),
            ui: String::new(),
            dir: String::new(),
            surface: String::new(),
            data_scope: String::new(),
        });
    };
    push_native(&mut out, "tasks", "后台任务");
    push_native(&mut out, "todos", "待办");
    push_native(&mut out, "db", "数据库");

    // ---- user plugins ----
    let root = plugins_root(paths);
    if let Ok(entries) = fs::read_dir(&root) {
        let mut users: Vec<PluginInfo> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter_map(|d| read_user_plugin(&d))
            .collect();
        users.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        out.extend(users);
    }

    // ---- merge persisted enabled flags ----
    for p in &mut out {
        if let Some(v) = reg.get(&p.id) {
            p.enabled = *v;
        }
    }
    out
}

/// Persist the enabled flag for one plugin id.
pub fn set_enabled(paths: &Paths, id: &str, enabled: bool) -> Result<(), String> {
    if id == "plugins" && !enabled {
        return Err("插件管理员是内置能力，不能停用".to_string());
    }
    let mut reg = load_registry(paths);
    reg.insert(id.to_string(), enabled);
    save_registry(paths, &reg)
}

/// Drop one user plugin: remove its registry row AND its directory.
/// Builtins are rejected (settings page hides the button too).
pub fn delete_plugin(paths: &Paths, id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() || id.contains('\\') || id.contains('/') || id.contains("..") {
        return Err(format!("非法插件 id: {id}"));
    }
    let dir = plugins_root(paths).join(id);
    if dir.is_dir() {
        fs::remove_dir_all(&dir).map_err(|e| format!("删除插件目录失败: {e}"))?;
    } else {
        return Err(format!("插件不存在或不是可删除的用户插件: {id}"));
    }
    let mut reg = load_registry(paths);
    reg.remove(id);
    save_registry(paths, &reg)
}

/// Absolute pi-extension entry files for a spawn, honouring enabled flags and
/// the per-session codegraph toggle.
///
/// The plugin-manager builtin ("plugins") is deliberately NOT included:
/// plugin authoring lives in the dedicated workshop session (插件工坊,
/// see `workshop_extension_file`) — main-chat sessions stay clean of it.
pub fn extension_files(paths: &Paths, use_codegraph: bool) -> Vec<PathBuf> {
    scan(paths)
        .into_iter()
        .filter(|p| p.has_tool() && p.id != "plugins")
        .filter(|p| p.enabled && (p.id != "codegraph" || use_codegraph))
        .map(|p| PathBuf::from(p.entry))
        .collect()
}

/// The plugin manager's extension file for WORKSHOP sessions — the only
/// extension they load (plus WARDEX_WORKSHOP=1 env so it can inject the
/// authoring system prompt via before_agent_start).
pub fn workshop_extension_file(_paths: &Paths) -> Option<PathBuf> {
    locate_extensions_dir()
        .map(|d| d.join("wardex-plugins.ts"))
        .filter(|p| p.is_file())
}

/// Ensure the user plugin tree exists (first run) and return it.
pub fn ensure_root(paths: &Paths) -> PathBuf {
    let root = plugins_root(paths);
    let _ = fs::create_dir_all(&root);
    root
}

/// Marker file holding the millis timestamp of the last 「生效」.
fn last_apply_path(paths: &Paths) -> PathBuf {
    plugins_root(paths).join(".last_apply")
}

fn last_apply_ms(paths: &Paths) -> Option<u128> {
    fs::read_to_string(last_apply_path(paths))
        .ok()
        .and_then(|s| s.trim().parse::<u128>().ok())
}

/// Stamp after a successful apply so pending_changes() resets.
pub fn mark_applied(paths: &Paths) {
    let _ = fs::create_dir_all(plugins_root(paths));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = fs::write(last_apply_path(paths), now.to_string());
}

/// Max size of one plugin runtime log before rotation (<id>.log → <id>.old).
const PLUGIN_LOG_MAX_BYTES: u64 = 64 * 1024;

/// Validate an id and resolve its runtime log path under .logs/.
/// Only ids that scan() actually knows about are accepted — the sink cannot
/// be used to scribble arbitrary files outside the plugin tree.
pub fn log_file(paths: &Paths, id: &str) -> Result<PathBuf, String> {
    let clean = id.trim();
    if clean.is_empty()
        || !clean
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || clean.starts_with('.')
    {
        return Err(format!("非法插件 id: {id}"));
    }
    if !scan(paths).iter().any(|p| p.id == clean) {
        return Err(format!("未知插件: {clean}"));
    }
    let dir = plugins_root(paths).join(".logs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("{clean}.log")))
}

/// Append frontend-captured panel console/error lines to <id>.log.
pub fn append_log(paths: &Paths, id: &str, lines: &[String]) -> Result<(), String> {
    use std::io::Write as _;
    let path = log_file(paths, id)?;
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > PLUGIN_LOG_MAX_BYTES {
        let _ = fs::rename(&path, path.with_extension("old"));
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    for line in lines {
        let _ = writeln!(f, "{line}");
    }
    Ok(())
}

/// Resolve the JSON data file for one plugin's declared scope. Session scope
/// never hits disk (frontend keeps it in memory); project scope lives under
/// <projectDir>/.wardex/plugin-data/, global under the plugin data root.
pub fn data_file(
    paths: &Paths,
    id: &str,
    scope: &str,
    project_dir: &str,
) -> Result<PathBuf, String> {
    let clean = id.trim();
    if clean.is_empty()
        || !clean
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || clean.starts_with('.')
        || !scan(paths).iter().any(|p| p.id == clean)
    {
        return Err(format!("非法或未知插件 id: {id}"));
    }
    match scope {
        "global" => {
            let dir = plugins_root(paths).join(".data");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Ok(dir.join(format!("{clean}.json")))
        }
        "project" => {
            let proj = PathBuf::from(project_dir.trim());
            if !proj.is_dir() {
                return Err("项目目录无效，无法存项目级数据".into());
            }
            let dir = proj.join(".wardex").join("plugin-data");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Ok(dir.join(format!("{clean}.json")))
        }
        _ => Err(format!("不支持的存储作用域: {scope}")),
    }
}

/// Read a plugin's whole data document (empty object when absent).
pub fn read_data(_paths: &Paths, path: &Path) -> serde_json::Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// Atomically write a plugin's whole data document.
pub fn write_data(paths: &Paths, path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    let _ = paths; // reserved (future per-scope quota/metadata)
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?)
        .map_err(|e| format!("写插件数据失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("提交插件数据失败: {e}"))
}

/// True when registry.json or any plugin file is newer than the last apply —
/// surfaced as a "有未生效的变更" hint in the UI (polling, cheap small tree).
pub fn pending_changes(paths: &Paths) -> bool {
    let Some(applied) = last_apply_ms(paths) else {
        return false; // never applied → nothing to diff against
    };
    let newer = |p: &Path| -> bool {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() > applied)
            .unwrap_or(false)
    };
    if newer(&registry_path(paths)) {
        return true;
    }
    // Any file under any user plugin dir counts (entry/ui/manifest edits by
    // hand or via the plugin-manager extension).
    let root = plugins_root(paths);
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if newer(&p) && p.file_name().map(|n| n != ".last_apply").unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> (tempfile::TempDir, Paths) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let paths = Paths::new(dir.path().join(tag));
        fs::create_dir_all(paths.root()).unwrap();
        (dir, paths)
    }

    #[test]
    fn registry_roundtrip_and_toggle() {
        let (_guard, paths) = temp_paths("reg");
        assert!(load_registry(&paths).is_empty());
        set_enabled(&paths, "myplug", false).unwrap();
        let reg = load_registry(&paths);
        assert_eq!(reg.get("myplug"), Some(&false));
        set_enabled(&paths, "myplug", true).unwrap();
        assert_eq!(load_registry(&paths).get("myplug"), Some(&true));
    }

    #[test]
    fn user_plugin_scan_and_extension_files() {
        let (_guard, paths) = temp_paths("scan");
        let plug = plugins_root(&paths).join("hello");
        fs::create_dir_all(&plug).unwrap();
        fs::write(
            plug.join("plugin.json"),
            r#"{"name":"Hello","version":"1.0","entry":"main.ts"}"#,
        )
        .unwrap();
        fs::write(plug.join("main.ts"), "export default () => {};").unwrap();

        let infos = scan(&paths);
        let hello = infos.iter().find(|p| p.id == "hello").expect("hello scanned");
        assert_eq!(hello.kind, "tool");
        assert!(hello.enabled, "new plugins default to enabled");

        // extension_files = builtins found via locate_extensions_dir + our
        // user plugin; disabling removes exactly the user plugin's entry.
        let before = extension_files(&paths, true);
        assert!(before.iter().any(|p| p.ends_with("main.ts")));
        set_enabled(&paths, "hello", false).unwrap();
        let after = extension_files(&paths, true);
        assert_eq!(before.len() - after.len(), 1);
        assert!(!after.iter().any(|p| p.ends_with("main.ts")));
    }

    #[test]
    fn delete_rejects_bad_ids_and_missing_dirs() {
        let (_guard, paths) = temp_paths("del");
        assert!(delete_plugin(&paths, "../escape").is_err());
        assert!(delete_plugin(&paths, "ghost").is_err());
        let plug = plugins_root(&paths).join("gone");
        fs::create_dir_all(&plug).unwrap();
        fs::write(plug.join("plugin.json"), "{}").unwrap();
        delete_plugin(&paths, "gone").unwrap();
        assert!(!plug.exists());
    }

    #[test]
    fn data_file_scopes_and_validation() {
        let (_guard, paths) = temp_paths("pdata");
        let plug = plugins_root(&paths).join("notes");
        fs::create_dir_all(&plug).unwrap();
        fs::write(plug.join("plugin.json"), r#"{"name":"N","entry":"main.ts"}"#).unwrap();

        // Unknown / malformed ids rejected.
        assert!(data_file(&paths, "ghost", "global", "").is_err());
        assert!(data_file(&paths, "../x", "global", "").is_err());
        assert!(data_file(&paths, "notes", "bogus", "").is_err());

        // Global scope → <root>/.data/notes.json.
        let g = data_file(&paths, "notes", "global", "").unwrap();
        assert!(g.starts_with(plugins_root(&paths).join(".data")));

        // Project scope requires a real directory and lands in .wardex.
        assert!(data_file(&paths, "notes", "project", "Z:/no/such/dir").is_err());
        let tmp_proj = std::env::temp_dir().join(format!("wardex-pd-proj-{}", std::process::id()));
        fs::create_dir_all(&tmp_proj).unwrap();
        let p = data_file(&paths, "notes", "project", &tmp_proj.to_string_lossy()).unwrap();
        assert!(p.starts_with(tmp_proj.join(".wardex").join("plugin-data")));
        let _ = fs::remove_dir_all(&tmp_proj);

        // Round-trip read/write of the whole document.
        write_data(&paths, &g, &json!({"count": 3})).unwrap();
        assert_eq!(read_data(&paths, &g), json!({"count": 3}));
    }

    #[test]
    fn plugin_log_sink_validates_and_appends() {
        let (_guard, paths) = temp_paths("plog");
        // Unknown / malformed ids are rejected — no arbitrary file writes.
        assert!(append_log(&paths, "ghost", &["x".into()]).is_err());
        assert!(append_log(&paths, "../evil", &["x".into()]).is_err());
        assert!(append_log(&paths, ".hidden", &["x".into()]).is_err());

        let plug = plugins_root(&paths).join("hello");
        fs::create_dir_all(&plug).unwrap();
        fs::write(plug.join("plugin.json"), r#"{"name":"H","entry":"main.ts"}"#).unwrap();

        append_log(&paths, "hello", &["line one".into(), "line two".into()]).unwrap();
        append_log(&paths, "hello", &["line three".into()]).unwrap();
        let content = fs::read_to_string(log_file(&paths, "hello").unwrap()).unwrap();
        assert_eq!(content, "line one\nline two\nline three\n");
    }

    #[test]
    fn pending_changes_tracks_apply_stamp() {
        let (_guard, paths) = temp_paths("pending");
        // Never applied → nothing pending.
        assert!(!pending_changes(&paths));
        let plug = plugins_root(&paths).join("late");
        fs::create_dir_all(&plug).unwrap();
        fs::write(plug.join("plugin.json"), "{}").unwrap();
        // Still nothing pending until the first apply stamps the baseline.
        assert!(!pending_changes(&paths));
        mark_applied(&paths);
        assert!(!pending_changes(&paths));
        // A newer file write (mtime > stamp) flips it on. Force an old stamp
        // to avoid filesystem mtime granularity flakes.
        fs::write(last_apply_path(&paths), "1000").unwrap();
        assert!(pending_changes(&paths));
        mark_applied(&paths);
        assert!(!pending_changes(&paths));
    }

    #[test]
    fn ui_only_plugin_kind() {
        let (_guard, paths) = temp_paths("ui");
        let plug = plugins_root(&paths).join("panelish");
        fs::create_dir_all(&plug).unwrap();
        fs::write(
            plug.join("plugin.json"),
            r#"{"name":"Panel","ui":"panel.html"}"#,
        )
        .unwrap();
        let infos = scan(&paths);
        let p = infos.iter().find(|x| x.id == "panelish").unwrap();
        assert_eq!(p.kind, "ui");
        assert!(p.entry.is_empty());
        assert!(p.ui.ends_with("panel.html"));
    }
}
