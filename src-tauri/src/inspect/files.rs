// Workspace directory listing for the file-tree panel (features/chat.md
// §6.4). One level per call — the tree lazily expands directories, so each
// expand is one cheap read_dir. Same ignore set as the @-picker / flat list
// (store::workspace::is_ignored_name); directories first, case-insensitive
// name sort (QDir::Name | IgnoreCase).

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::store::paths::{clean_path_forward, is_absolute_windows};
use crate::store::workspace::is_ignored_name;

#[derive(Debug, Clone, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
}

/// List one directory level under the workspace root. `rel` is the
/// workspace-relative directory ("" for the root). Rejects absolute paths
/// and escapes the same way readFileRange does.
pub fn list_workspace_dir(root: &str, rel: &str) -> Result<Vec<DirEntry>, String> {
    if root.is_empty() {
        return Ok(Vec::new());
    }
    let root_abs = clean_path_forward(&Path::new(root).to_string_lossy());
    let joined = if rel.is_empty() {
        root_abs.clone()
    } else {
        format!("{root_abs}/{rel}")
    };
    let abs = clean_path_forward(&joined);
    let absolute_input =
        is_absolute_windows(rel) || rel.starts_with('/') || rel.starts_with('\\');
    let under_root = abs == root_abs
        || abs
            .to_lowercase()
            .starts_with(&format!("{}/", root_abs.to_lowercase()));
    if absolute_input || !under_root {
        return Err("路径超出工作区".to_string());
    }
    let dir = Path::new(&abs);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Err("无法读取目录".to_string());
    };
    let mut out: Vec<DirEntry> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_ignored_name(&name) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(DirEntry { name, dir: is_dir });
    }
    out.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_dirs_first_and_ignores_noise() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().into_owned();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join("README.md"), "hi").unwrap();
        let out = list_workspace_dir(&root, "").unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].dir && out[0].name == "src");
        assert!(!out[1].dir && out[1].name == "README.md");
    }

    #[test]
    fn rejects_escape_and_absolute() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().into_owned();
        assert!(list_workspace_dir(&root, "..").is_err());
        assert!(list_workspace_dir(&root, "C:/Windows").is_err());
        assert!(list_workspace_dir(&root, "/etc").is_err());
    }

    #[test]
    fn empty_root_and_missing_dir_are_empty() {
        assert!(list_workspace_dir("", "").unwrap().is_empty());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().into_owned();
        assert!(list_workspace_dir(&root, "nope").unwrap().is_empty());
    }
}
