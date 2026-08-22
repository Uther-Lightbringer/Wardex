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
                });
            }
        };
        push_builtin(&mut out, "wardex-reminders.ts", "reminders", "会话提醒");
        push_builtin(&mut out, "wardex-codegraph.ts", "codegraph", "Codegraph 代码索引");
        push_builtin(&mut out, "wardex-plugins.ts", "plugins", "插件管理员");
    }

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
/// the per-session codegraph toggle. The plugin-manager builtin ("plugins")
/// is FORCE-INCLUDED so the model can always manage plugins.
pub fn extension_files(paths: &Paths, use_codegraph: bool) -> Vec<PathBuf> {
    scan(paths)
        .into_iter()
        .filter(|p| p.has_tool())
        .filter(|p| p.id == "plugins" || (p.enabled && (p.id != "codegraph" || use_codegraph)))
        .map(|p| PathBuf::from(p.entry))
        .collect()
}

/// Ensure the user plugin root exists (first run) and return it.
pub fn ensure_root(paths: &Paths) -> PathBuf {
    let root = plugins_root(paths);
    let _ = fs::create_dir_all(&root);
    root
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
