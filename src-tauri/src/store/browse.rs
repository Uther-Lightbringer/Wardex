// Folder browser backend for the 打开项目 dialog (FolderBrowserModel
// equivalent in the old app): drive enumeration, one-level directory listing
// (directories only), and inline folder creation. Windows-only by design
// (design-principles.md §5) — drive letters are probed A..Z.

use serde::Serialize;

/// One row of the folder list. `path` uses native separators and carries no
/// trailing backslash (drive roots keep theirs, e.g. "C:\").
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
}

/// Existing drive roots, A..Z order ("C:\" style). Falls back to ["C:\"]
/// when the probe finds nothing (sandboxed/odd environments), so the dialog
/// always has one dropdown entry.
pub fn drives() -> Vec<String> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if std::path::Path::new(&root).is_dir() {
            out.push(root);
        }
    }
    if out.is_empty() {
        out.push("C:\\".to_string());
    }
    out
}

/// Join a child name onto a directory with exactly one backslash. Forward
/// slashes are normalized to native separators; a drive root "C:\" loses
/// then regains its separator, so "C:" + "\" + name.
pub fn join(dir: &str, name: &str) -> String {
    let normalized = dir.trim().replace('/', "\\");
    let d = normalized.trim_end_matches('\\');
    format!("{}\\{}", d, name)
}

/// Parent directory of `dir` ("C:\a\b" → "C:\a", "C:\a" → "C:\"); None when
/// already at a drive root or the input is not drive-absolute.
pub fn parent_of(dir: &str) -> Option<String> {
    let d = dir.trim().trim_end_matches(['/', '\\']);
    let bytes = d.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let rest = &d[2..]; // strip "C:"
    if rest.is_empty() {
        return None; // already at the drive root
    }
    match rest.rfind('\\') {
        None => Some(format!("{}:\\", &d[..1])),
        Some(0) => Some(format!("{}:\\", &d[..1])),
        Some(i) => Some(format!("{}:{}", &d[..1], &rest[..i])),
    }
}

/// One level of subdirectories, sorted by name (case-insensitive). Anything
/// unreadable (missing dir, denied) yields an empty list — the dialog shows
// its （空目录） row, same tolerance as the old model.
pub fn list_dirs(dir: &str) -> Vec<FolderEntry> {
    let mut out: Vec<FolderEntry> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            out.push(FolderEntry {
                path: join(dir, &name),
                name,
            });
        }
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

/// Inline 新建文件夹: reject empty names and path-illegal characters
/// (the old dialog's rule), then create_dir (fails if it already exists).
pub fn create_dir(dir: &str, name: &str) -> Result<FolderEntry, String> {
    const ERR: &str = "创建失败：名称无效或已存在";
    let n = name.trim();
    if n.is_empty() || n.chars().any(|c| "\\/:*?\"<>|".contains(c)) {
        return Err(ERR.to_string());
    }
    let path = join(dir, n);
    if std::path::Path::new(&path).exists() {
        return Err(ERR.to_string());
    }
    std::fs::create_dir(&path).map_err(|_| ERR.to_string())?;
    Ok(FolderEntry {
        name: n.to_string(),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_handles_roots_and_trailing_separators() {
        assert_eq!(join("C:\\", "a"), "C:\\a");
        assert_eq!(join("C:\\work", "a"), "C:\\work\\a");
        assert_eq!(join("C:/work/", "a"), "C:\\work\\a");
    }

    #[test]
    fn parent_of_walks_up_to_the_root() {
        assert_eq!(parent_of("C:\\a\\b"), Some("C:\\a".to_string()));
        assert_eq!(parent_of("C:\\a"), Some("C:\\".to_string()));
        assert_eq!(parent_of("C:\\"), None);
        assert_eq!(parent_of("relative\\path"), None);
    }

    #[test]
    fn create_dir_rejects_illegal_names() {
        for bad in ["", "  ", "a/b", "a\\b", "a:b", "a?b", "a*b", "a\"b", "a<b", "a>b", "a|b"] {
            assert!(create_dir("C:\\definitely-not-here-wardex", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn create_and_list_roundtrip() {
        let base = std::env::temp_dir().join(format!(
            "wardex-browse-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let dir = base.to_string_lossy().to_string();

        let made = create_dir(&dir, "子目录").unwrap();
        assert_eq!(made.name, "子目录");
        assert!(std::path::Path::new(&made.path).is_dir());
        // Duplicate name is refused.
        assert!(create_dir(&dir, "子目录").is_err());
        // The new folder shows up in the listing.
        let names: Vec<String> = list_dirs(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["子目录"]);

        let _ = std::fs::remove_dir_all(&base);
    }
}
