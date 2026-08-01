// Data-root location and path layout (data-formats.md §1).
//
// Dev/release discrimination is deliberately NOT the old exe-name rule:
// `cfg!(debug_assertions)` selects `%AppData%/WarDex-tauri-dev` for dev
// builds (isolated from both legacy data sets) and `%AppData%/WarDex` for
// release builds (which directly reuse the user's existing data, zero
// migration). All discrimination lives in `root()` — nowhere else.
//
// Stores receive a `Paths` (cheaply clonable) instead of calling `root()`
// themselves, so unit tests run against an isolated temp root without any
// global state.

use std::path::{Path, PathBuf};

use crate::store::media;

/// Dev → `%AppData%/WarDex-tauri-dev`, release → `%AppData%/WarDex`.
/// Falls back to the system temp dir if the roaming profile is unavailable
/// (should not happen on Windows; keeps the app alive rather than panicking).
pub fn root() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    base.join(if cfg!(debug_assertions) {
        "WarDex-tauri-dev"
    } else {
        "WarDex"
    })
}

#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Production paths from the dev/release discrimination above.
    pub fn production() -> Self {
        Self::new(root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    pub fn agents_index_path(&self) -> PathBuf {
        self.agents_dir().join("index.json")
    }

    pub fn agent_file_path(&self, agent_id: &str) -> PathBuf {
        self.agents_dir().join(format!("{agent_id}.json"))
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(session_id)
    }

    pub fn session_meta_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("meta.json")
    }

    pub fn session_messages_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("messages.jsonl")
    }

    pub fn session_workspace_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("workspace")
    }

    pub fn media_root(&self) -> PathBuf {
        self.root.join("media")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn crashes_dir(&self) -> PathBuf {
        self.root.join("crashes")
    }

    pub fn projects_path(&self) -> PathBuf {
        self.root.join("projects.json")
    }

    pub fn user_prefs_path(&self) -> PathBuf {
        self.root.join("user_prefs.json")
    }

    pub fn user_avatar_path(&self) -> PathBuf {
        self.root.join("user_avatar.png")
    }

    pub fn todos_path(&self) -> PathBuf {
        self.root.join("todos.json")
    }

    pub fn reminders_path(&self) -> PathBuf {
        self.root.join("reminders.json")
    }

    pub fn usage_path(&self) -> PathBuf {
        self.root.join("usage.json")
    }

    pub fn prompts_path(&self) -> PathBuf {
        self.root.join("prompts.json")
    }

    /// Startup layout (data-formats.md §1.2): create every directory and run
    /// the 14-day media prune once. Individual failures are logged to stderr
    /// but never abort startup — every reader tolerates a missing file.
    pub fn ensure_layout(&self) {
        for dir in [
            self.agents_dir(),
            self.sessions_dir(),
            self.media_root(),
            self.logs_dir(),
            self.crashes_dir(),
        ] {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("WarDex: mkpath {} failed: {e}", dir.display());
            }
        }
        let today = chrono::Local::now().date_naive();
        if let Err(e) = media::prune_media(self, media::DEFAULT_MAX_AGE_DAYS, today) {
            eprintln!("WarDex: media prune failed: {e}");
        }
    }
}

/// canonicalDir() (data-formats.md §6.1): absolute, lexically cleaned
/// (`.`/`..` resolved, duplicate separators folded), forward slashes, no
/// trailing slash except drive roots ("C:/"). Symlinks are NOT resolved
/// (Qt's QDir::cleanPath doesn't either). Returns "" for empty input.
pub fn canonical_dir(dir: &str) -> String {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Make absolute against the process cwd when relative (Qt
    // QDir(dir).absolutePath() semantics).
    let p = Path::new(trimmed);
    let abs = if is_absolute_windows(trimmed) {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(p),
            Err(_) => p.to_path_buf(),
        }
    };
    clean_path_forward(&abs.to_string_lossy())
}

/// Windows-aware absolute-path test (Qt QDir::isAbsolutePath on Windows):
/// drive-letter paths ("C:/…"), UNC ("\\…", "//…"), and root-relative
/// ("\…", "/…" is drive-relative for Qt, but for our containment checks we
/// refuse it too — safer, and the old code's escape guard treats any
/// non-under-root resolution as an escape anyway).
pub fn is_absolute_windows(p: &str) -> bool {
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return true;
    }
    p.starts_with("\\\\") || p.starts_with("//") || Path::new(p).is_absolute()
}

/// Lexical path cleanup producing Qt QDir::cleanPath-style output:
/// forward slashes, no `.`/`..`, no duplicate separators, no trailing slash
/// (drive roots keep theirs: "C:/"; UNC roots keep "//server/share").
pub fn clean_path_forward(input: &str) -> String {
    let s = input.replace('\\', "/");
    let unc = s.starts_with("//");
    let mut prefix = String::new();
    let mut rest = s.as_str();
    if unc {
        rest = &rest[2..];
        prefix.push_str("//");
    }
    // Drive letter ("C:/…").
    let bytes = rest.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        prefix.push_str(&rest[..2]);
        rest = &rest[2..];
    }
    let rooted = rest.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                // Pop only within the path; ".." at the root is dropped
                // (QDir::cleanPath behavior for absolute paths).
                if !parts.is_empty() {
                    parts.pop();
                }
            }
            c => parts.push(c),
        }
    }
    let mut out = prefix;
    if rooted || (!out.is_empty() && !out.ends_with('/')) {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if out.is_empty() {
        // Everything collapsed away (e.g. "C:/..") — keep the root.
        return String::new();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_cases() {
        assert_eq!(clean_path_forward("C:/a//b/./c/../d/"), "C:/a/b/d");
        assert_eq!(clean_path_forward("C:\\a\\b"), "C:/a/b");
        assert_eq!(clean_path_forward("C:/"), "C:/");
        assert_eq!(clean_path_forward("C:/../x"), "C:/x");
        assert_eq!(clean_path_forward("C:/.."), "C:/");
        assert_eq!(clean_path_forward("//server/share//a/"), "//server/share/a");
    }

    #[test]
    fn absolute_windows() {
        assert!(is_absolute_windows("C:/a"));
        assert!(is_absolute_windows("c:\\a"));
        assert!(is_absolute_windows("\\\\server\\share"));
        assert!(!is_absolute_windows("a/b"));
        assert!(!is_absolute_windows("./a"));
    }

    #[test]
    fn canonical_absolute() {
        // Absolute inputs are cleaned, independent of cwd.
        assert_eq!(canonical_dir("C:/workspace/WarDex/"), "C:/workspace/WarDex");
        assert_eq!(canonical_dir("c:\\workspace\\.\\WarDex"), "c:/workspace/WarDex");
        assert_eq!(canonical_dir(""), "");
    }
}
