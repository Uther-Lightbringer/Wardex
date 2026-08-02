// Workspace file access and @-references (data-formats.md §11). These are
// runtime reads against the session workspace root — they change no on-disk
// format, but the ignore set, caps and error enums are a user-visible
// contract and are replicated exactly.
//
// Two DIFFERENT error vocabularies live here on purpose:
//   - readFileRange / previewFile: machine strings ("escape" | "missing" |
//     "unreadable" | "binary" | "range")
//   - savePreviewText: Chinese user-facing strings ("文件不存在" …)
// They must not be mixed (§11.3 vs §11.4).

use std::fs;
use std::io::Read;
use std::path::Path;

use serde::Serialize;

use crate::store::json::contains_case_insensitive;
use crate::store::paths::{clean_path_forward, is_absolute_windows};

/// Injection cap per file for @-reference expansion (~200KB).
pub const MAX_REF_BYTES: u64 = 200 * 1024;
/// previewFile reads at most this much (256KB).
pub const MAX_PREVIEW_BYTES: u64 = 256 * 1024;
/// savePreviewText sniffs the first 4096 bytes for NUL.
const SAVE_PROBE_BYTES: usize = 4096;

/// Keep the ignore list deliberately small: VCS metadata, dependency and
/// build-output dirs — plus anything starting with ".git".
pub fn is_ignored_name(name: &str) -> bool {
    const IGNORED: [&str; 8] = [
        ".git",
        "node_modules",
        "build",
        "dist",
        ".venv",
        "__pycache__",
        ".qt",
        ".rcc",
    ];
    IGNORED.contains(&name) || name.starts_with(".git")
}

pub fn is_image_ext(ext_lower: &str) -> bool {
    const EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
    EXTS.contains(&ext_lower)
}

pub fn is_binary_ext(ext_lower: &str) -> bool {
    const EXTS: [&str; 21] = [
        "ico", "zip", "7z", "rar", "gz", "exe", "dll", "pdf", "mp3", "mp4", "wav", "ogg", "blp",
        "mpq", "mdx", "glb", "bin", "dat", "db", "so", "dylib",
    ];
    EXTS.contains(&ext_lower)
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// UTF-8 with the legacy fallback: if the lossy decode produced U+FFFD the
/// file is probably GBK etc. — retry with GBK (old: QString::fromLocal8Bit
/// on a Chinese-Windows system).
fn decode_text(bytes: &[u8]) -> String {
    let utf8 = String::from_utf8_lossy(bytes);
    if utf8.contains('\u{FFFD}') {
        let (decoded, _, _) = encoding_rs::GBK.decode(bytes);
        decoded.into_owned()
    } else {
        utf8.into_owned()
    }
}

// ---------------------------------------------------------------------------
// workspaceFileList (§11.2): flat recursive relative-path list for the @
// picker. DFS with a manual stack so ignored dirs are never descended.
// ---------------------------------------------------------------------------

pub fn workspace_file_list(root: &Path, filter: &str, max_results: usize) -> Vec<String> {
    let mut out = Vec::new();
    let f = filter.trim();
    if root.as_os_str().is_empty() || max_results == 0 || !root.is_dir() {
        return out;
    }
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= max_results {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        // Per-level case-insensitive name sort (QDir::Name | IgnoreCase).
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_ignored_name(&name) {
                continue;
            }
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path);
                continue;
            }
            let ext = ext_of(&path);
            // The picker is for text references — binaries/images are skipped
            // at expansion time anyway, so don't offer them.
            if is_binary_ext(&ext) || is_image_ext(&ext) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if f.is_empty() || contains_case_insensitive(&rel, f) {
                out.push(rel);
                if out.len() >= max_results {
                    break;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// readFileRange (§11.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FileLine {
    pub n: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadFileRangeOk {
    pub ok: bool,
    pub lines: Vec<FileLine>,
    #[serde(rename = "totalLines")]
    pub total_lines: i64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadFileRangeErr {
    pub ok: bool,
    /// "escape" | "missing" | "unreadable" | "binary" | "range"
    pub error: String,
    /// Present only for the "range" error (old code inserted them first).
    #[serde(rename = "totalLines", skip_serializing_if = "Option::is_none")]
    pub total_lines: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

pub type ReadFileRangeResult = Result<ReadFileRangeOk, ReadFileRangeErr>;

fn range_err(error: &str) -> ReadFileRangeErr {
    ReadFileRangeErr {
        ok: false,
        error: error.to_string(),
        total_lines: None,
        truncated: None,
    }
}

/// Line-range read of a workspace file for @-reference expansion at send
/// time. `from <= 0` → whole file (still line-numbered); `to <= 0` → the
/// single line `from`; 1-based line numbers.
pub fn read_file_range(root: &str, rel_path: &str, from: i64, to: i64) -> ReadFileRangeResult {
    if root.is_empty() {
        return Err(range_err("missing"));
    }
    let root_abs = clean_path_forward(&Path::new(root).to_string_lossy());
    let joined = format!("{root_abs}/{rel_path}");
    let abs = clean_path_forward(&joined);
    // Containment check on the cleaned absolute path (case-insensitive:
    // Windows). Symlink tricks are out of scope — the composer token is
    // plain text typed by the local user.
    let absolute_input = is_absolute_windows(rel_path)
        || rel_path.starts_with('/')
        || rel_path.starts_with('\\');
    let under_root = abs == root_abs
        || abs
            .to_lowercase()
            .starts_with(&format!("{}/", root_abs.to_lowercase()));
    if absolute_input || !under_root {
        return Err(range_err("escape"));
    }
    let path = Path::new(&abs);
    if !path.is_file() {
        return Err(range_err("missing"));
    }
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let head = match read_head(path, MAX_REF_BYTES) {
        Some(h) => h,
        None => return Err(range_err("unreadable")),
    };
    let ext = ext_of(path);
    if is_binary_ext(&ext) || is_image_ext(&ext) || head.contains(&0) {
        return Err(range_err("binary"));
    }
    let text = decode_text(&head);
    let mut all: Vec<&str> = text.split('\n').collect();
    let truncated = size > MAX_REF_BYTES;
    if truncated && !all.is_empty() {
        all.pop(); // last line may be cut mid-way
    }
    let total = all.len() as i64;

    let mut lo = from;
    let mut hi = to;
    if lo <= 0 {
        // whole file
        lo = 1;
        hi = total;
    } else {
        if hi <= 0 {
            hi = lo; // "@file:10" → that single line
        }
        if hi < lo {
            hi = lo;
        }
        if lo > total {
            return Err(ReadFileRangeErr {
                ok: false,
                error: "range".to_string(),
                total_lines: Some(total),
                truncated: Some(truncated),
            });
        }
        hi = hi.min(total);
    }
    let mut lines = Vec::with_capacity((hi - lo + 1).max(0) as usize);
    for n in lo..=hi {
        let raw = all.get((n - 1) as usize).copied().unwrap_or_default();
        let t = raw.strip_suffix('\r').unwrap_or(raw);
        lines.push(FileLine {
            n,
            text: t.to_string(),
        });
    }
    Ok(ReadFileRangeOk {
        ok: true,
        lines,
        total_lines: total,
        truncated,
    })
}

fn read_head(path: &Path, max: u64) -> Option<Vec<u8>> {
    let f = fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf).ok()?;
    Some(buf)
}

// ---------------------------------------------------------------------------
// codeSearch: project-wide search (Ctrl+F overlay on the chat page).
// Modes: content-only, filename-only or both (filename rows carry line 0).
// Plain queries are case-insensitive substrings; `regex` mode treats the
// query as a case-insensitive regex. Honoring the same ignore list and
// binary/ext skip rules as the rest of the workspace module; an explicit
// extension filter narrows the walk further. Capped on scanned bytes,
// matched files, hits per file and total hits so a large project can never
// stall the UI.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CodeHit {
    /// 1-based line number; **0 = filename-only match** (no content hit).
    pub line: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeMatch {
    pub file: String,
    pub hits: Vec<CodeHit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Match against file contents only.
    Content,
    /// Match against relative file paths only.
    Filename,
    /// Filename matches first, then content matches.
    Both,
}

pub struct CodeSearchOptions<'a> {
    pub mode: SearchMode,
    /// Restrict scanned files to these extensions (no leading dot, lowercase,
    /// e.g. "java"). Empty = every text file.
    pub exts: &'a [String],
    /// Treat the query as a case-insensitive regex.
    pub regex: bool,
}

/// Files matched at most (the hit cap usually binds first).
pub const CODE_SEARCH_MAX_FILES: usize = 60;
/// Hits per file so one file can't flood the results.
pub const CODE_SEARCH_PER_FILE_HITS: usize = 8;
/// Total bytes scanned across files before giving up (responsiveness).
const CODE_SEARCH_BUDGET: u64 = 64 * 1024 * 1024;
/// Longest line text forwarded to the UI (minified files).
const CODE_SEARCH_LINE_CAP: usize = 300;

enum Matcher {
    Plain(String),
    Regex(regex::Regex),
}

impl Matcher {
    fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Plain(ql) => text.to_lowercase().contains(ql),
            Matcher::Regex(r) => r.is_match(text),
        }
    }
}

pub fn search_code(
    root: &Path,
    query: &str,
    opts: &CodeSearchOptions<'_>,
) -> Result<Vec<CodeMatch>, String> {
    let q = query.trim();
    if q.is_empty() || !root.is_dir() {
        return Ok(Vec::new());
    }
    let matcher = if opts.regex {
        Matcher::Regex(
            regex::RegexBuilder::new(q)
                .case_insensitive(true)
                .build()
                .map_err(|e| format!("正则表达式无效：{e}"))?,
        )
    } else {
        Matcher::Plain(q.to_lowercase())
    };
    let mut out: Vec<CodeMatch> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let mut scanned: u64 = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= CODE_SEARCH_MAX_FILES {
                return Ok(out);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_ignored_name(&name) {
                continue;
            }
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path);
                continue;
            }
            let ext = ext_of(&path);
            if is_binary_ext(&ext) || is_image_ext(&ext) {
                continue;
            }
            if !opts.exts.is_empty() && !opts.exts.contains(&ext) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let mut hits: Vec<CodeHit> = Vec::new();
            if opts.mode != SearchMode::Content && matcher.is_match(&rel_str) {
                hits.push(CodeHit { line: 0, text: String::new() });
            }
            if opts.mode != SearchMode::Filename && hits.len() < CODE_SEARCH_PER_FILE_HITS {
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                if meta.len() > CODE_SEARCH_BUDGET {
                    continue;
                }
                scanned += meta.len();
                if scanned > CODE_SEARCH_BUDGET {
                    return Ok(out);
                }
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                if bytes.contains(&0) {
                    continue;
                }
                let text = decode_text(&bytes);
                for (i, raw) in text.split('\n').enumerate() {
                    if hits.len() >= CODE_SEARCH_PER_FILE_HITS {
                        break;
                    }
                    if matcher.is_match(raw) {
                        let t = raw.strip_suffix('\r').unwrap_or(raw);
                        hits.push(CodeHit {
                            line: (i + 1) as i64,
                            text: t.chars().take(CODE_SEARCH_LINE_CAP).collect(),
                        });
                    }
                }
            }
            if !hits.is_empty() {
                out.push(CodeMatch { file: rel_str, hits });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// searchJavaInterfaces (Ctrl+\ overlay, V1 heuristic — no AST): scan .java
// files for `interface` declarations with a regex. Comments/strings can
// produce false positives (accepted for V1; a tree-sitter index is the
// planned replacement). Empty query lists every declaration; otherwise the
// interface NAME must contain the query (case-insensitive).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceHit {
    pub file: String,
    pub line: i64,
    pub name: String,
    pub text: String,
}

/// Declarations returned at most.
pub const INTERFACE_MAX: usize = 60;
/// Total bytes scanned across files before giving up.
const INTERFACE_BUDGET: u64 = 64 * 1024 * 1024;

pub fn search_java_interfaces(root: &Path, query: &str) -> Vec<InterfaceHit> {
    let q = query.trim().to_lowercase();
    if !root.is_dir() {
        return Vec::new();
    }
    let re = regex::Regex::new(r"(?i)\binterface\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
    let mut out: Vec<InterfaceHit> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let mut scanned: u64 = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= INTERFACE_MAX {
                return out;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_ignored_name(&name) {
                continue;
            }
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path);
                continue;
            }
            if ext_of(&path) != "java" {
                continue;
            }
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.len() > INTERFACE_BUDGET {
                continue;
            }
            scanned += meta.len();
            if scanned > INTERFACE_BUDGET {
                return out;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let text = decode_text(&bytes);
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            for (i, raw) in text.split('\n').enumerate() {
                if out.len() >= INTERFACE_MAX {
                    return out;
                }
                let t = raw.strip_suffix('\r').unwrap_or(raw);
                if let Some(caps) = re.captures(t) {
                    let iname = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
                    if q.is_empty() || iname.to_lowercase().contains(&q) {
                        out.push(InterfaceHit {
                            file: rel_str.clone(),
                            line: (i + 1) as i64,
                            name: iname,
                            text: t.to_string(),
                        });
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// previewFile / savePreviewText (§11.4). Input is an ABSOLUTE path from the
// workspace tree — no root containment check (old behavior).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PreviewOutcome {
    pub ok: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

impl PreviewOutcome {
    fn fail(size: u64, reason: &str) -> Self {
        Self {
            ok: false,
            size,
            reason: Some(reason.to_string()),
            image: None,
            text: None,
            truncated: None,
        }
    }
}

pub fn preview_file(path: &str) -> PreviewOutcome {
    let p = Path::new(path);
    if !p.is_file() {
        return PreviewOutcome::fail(0, "missing");
    }
    let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let ext = ext_of(p);
    // Images preview in-app (the front end loads the file directly) — no
    // text read.
    if is_image_ext(&ext) {
        return PreviewOutcome {
            ok: true,
            size,
            reason: None,
            image: Some(true),
            text: None,
            truncated: None,
        };
    }
    let Some(head) = read_head(p, MAX_PREVIEW_BYTES) else {
        return PreviewOutcome::fail(size, "unreadable");
    };
    if is_binary_ext(&ext) || head.contains(&0) {
        return PreviewOutcome::fail(size, "binary");
    }
    PreviewOutcome {
        ok: true,
        size,
        reason: None,
        image: Some(false),
        text: Some(decode_text(&head)),
        truncated: Some(size > MAX_PREVIEW_BYTES),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SavePreviewOutcome {
    pub ok: bool,
    /// Chinese user-facing string on failure (§11.4) — NOT the machine
    /// enum of readFileRange.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SavePreviewOutcome {
    fn fail(error: &str) -> Self {
        Self {
            ok: false,
            error: Some(error.to_string()),
        }
    }

    fn success() -> Self {
        Self { ok: true, error: None }
    }
}

/// Whole-file UTF-8 write-back for the editable text preview. Refuses
/// missing and binary/image files (never write text over a binary file).
pub fn save_preview_text(path: &str, content: &str) -> SavePreviewOutcome {
    let p = Path::new(path);
    if !p.is_file() {
        return SavePreviewOutcome::fail("文件不存在");
    }
    let ext = ext_of(p);
    if is_binary_ext(&ext) || is_image_ext(&ext) {
        return SavePreviewOutcome::fail("二进制文件不可编辑");
    }
    match read_head(p, SAVE_PROBE_BYTES as u64) {
        Some(probe) => {
            if probe.contains(&0) {
                return SavePreviewOutcome::fail("二进制文件不可编辑");
            }
        }
        None => return SavePreviewOutcome::fail("无法读取原文件"),
    }
    // Deliberately a plain truncate write (old behavior); user workspace
    // files are not app-owned JSON so the atomic tmp+rename dance (with its
    // remove-first window) is not applied here.
    match fs::write(p, content.as_bytes()) {
        Ok(()) => SavePreviewOutcome::success(),
        Err(e) => SavePreviewOutcome::fail(&e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// gitBranchFor (§11.5): read .git/HEAD directly — no process spawn.
// ---------------------------------------------------------------------------

pub fn git_branch_for(dir: &str) -> String {
    if dir.is_empty() {
        return String::new();
    }
    let git_path = Path::new(dir).join(".git");
    let head_path = if git_path.is_dir() {
        git_path.join("HEAD")
    } else if git_path.is_file() {
        // Worktree/submodule gitfile: ".git" contains "gitdir: <path>".
        let Ok(raw) = fs::read(&git_path) else {
            return String::new();
        };
        let line = String::from_utf8_lossy(&raw).trim().to_string();
        let Some(rest) = line.strip_prefix("gitdir:") else {
            return String::new();
        };
        let gd = rest.trim();
        let gd_path = Path::new(gd);
        let gd_abs = if gd_path.is_absolute() {
            gd_path.to_path_buf()
        } else {
            Path::new(dir).join(gd_path)
        };
        gd_abs.join("HEAD")
    } else {
        return String::new();
    };
    let Ok(raw) = fs::read(&head_path) else {
        return String::new();
    };
    let head = String::from_utf8_lossy(&raw).trim().to_string();
    if let Some(b) = head.strip_prefix("ref: refs/heads/") {
        return b.to_string();
    }
    if let Some(r) = head.strip_prefix("ref: ") {
        return r.to_string();
    }
    // detached HEAD → short SHA
    head.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &std::path::Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("src/App.java"), "package a;\ninterface Demo { void run(); }\n").unwrap();
        fs::write(root.join("src/Impl.java"), "class Impl implements Demo {}\n").unwrap();
        fs::write(root.join("src/notes.md"), "# notes\n").unwrap();
        fs::write(root.join("node_modules/pkg/Dep.java"), "interface Hidden {}\n").unwrap();
        fs::write(root.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
    }

    fn opts<'a>(mode: SearchMode) -> CodeSearchOptions<'a> {
        CodeSearchOptions { mode, exts: &[], regex: false }
    }

    #[test]
    fn search_finds_lines_case_insensitively_and_skips_ignored() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let res = search_code(dir.path(), "interface", &opts(SearchMode::Content)).unwrap();
        assert_eq!(res.len(), 1, "only App.java has 'interface'; node_modules/binaries skipped");
        assert!(res.iter().all(|m| !m.file.contains("node_modules")));
        assert_eq!(res[0].file, "src/App.java");
        assert_eq!(res[0].hits[0].line, 2);
    }

    #[test]
    fn search_empty_query_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        assert!(search_code(dir.path(), "  ", &opts(SearchMode::Content)).unwrap().is_empty());
    }

    #[test]
    fn search_reports_relative_paths_and_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let res = search_code(dir.path(), "run", &opts(SearchMode::Content)).unwrap();
        assert_eq!(res[0].file, "src/App.java");
        assert_eq!(res[0].hits[0].line, 2);
        assert_eq!(res[0].hits[0].text, "interface Demo { void run(); }");
    }

    #[test]
    fn filename_mode_matches_paths_with_line_zero() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let res = search_code(dir.path(), "impl", &opts(SearchMode::Filename)).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].file, "src/Impl.java");
        assert_eq!(res[0].hits[0].line, 0);
    }

    #[test]
    fn both_mode_matches_name_first_then_content() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let res = search_code(dir.path(), "App", &opts(SearchMode::Both)).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].hits[0].line, 0, "filename match carries line 0");
        assert_eq!(res[0].hits.len(), 1, "'App' appears in no content line");
    }

    #[test]
    fn ext_filter_narrows_scan() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let exts = vec!["java".to_string()];
        let o = CodeSearchOptions { mode: SearchMode::Content, exts: &exts, regex: false };
        let res = search_code(dir.path(), "notes", &o).unwrap();
        assert!(res.is_empty(), ".md excluded by ext filter");
        let o2 = CodeSearchOptions { mode: SearchMode::Content, exts: &exts, regex: false };
        let res = search_code(dir.path(), "interface", &o2).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].file, "src/App.java");
    }

    #[test]
    fn regex_mode_matches_and_rejects_invalid() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let o = CodeSearchOptions { mode: SearchMode::Content, exts: &[], regex: true };
        let res = search_code(dir.path(), r"void\s+run\(\)", &o).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].file, "src/App.java");
        assert!(search_code(dir.path(), "(", &o).is_err(), "unbalanced '(' is an invalid regex");
    }

    #[test]
    fn interfaces_found_by_name_or_listed_all() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let res = search_java_interfaces(dir.path(), "");
        assert_eq!(res.len(), 1, "only App.java declares an interface; node_modules skipped");
        assert!(res.iter().all(|h| h.file != "node_modules/pkg/Dep.java"), "ignored dirs skipped");
        let by_name = search_java_interfaces(dir.path(), "dem");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].name, "Demo");
        assert_eq!(by_name[0].line, 2);
        assert!(search_java_interfaces(dir.path(), "zzz").is_empty());
    }
}
