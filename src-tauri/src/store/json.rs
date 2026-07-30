// Shared JSON persistence helpers used by every store domain (docs:
// data-formats.md §0). Centralizes the disk-format quirks so adding a new
// store domain never re-implements them:
//
// - Tolerant reads: a missing/corrupt file is *not* an error — the old Qt
//   code treated QJsonDocument parse failure as an empty object/array
//   (crash-truncated files must load as defaults).
// - Atomic writes: tmp file + rename (performance.md §4). The old version
//   truncated in place; the rewrite deliberately upgrades full-file writes
//   to atomic ones — crash safety without changing the file format.
// - 4-space indented output for single JSON files (Qt QJsonDocument::Indented)
//   and compact single lines for JSONL.
// - Timestamps are Unix ms numbers parsed via f64 (Qt read them with
//   toDouble(), so historical files may carry float-shaped values), and
//   written back as integer-shaped numbers.
// - Case-insensitive matching helpers on the ORIGINAL string so snippet
//   offsets stay valid (no case folding, see data-formats.md §10).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum JsonError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("serialize error on {path}: {source}")]
    Serde {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub type JsonResult<T> = Result<T, JsonError>;

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> JsonError + '_ {
    move |source| JsonError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Tolerant read of a whole JSON file. Missing file, unreadable file, or
/// corrupt JSON all yield `None` (the caller then applies its defaults).
/// Never fails — matches the old QJsonDocument::fromJson behavior.
pub fn read_value(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Tolerant read of a JSON object file; absent/corrupt/non-object → empty map.
pub fn read_object(path: &Path) -> Map<String, Value> {
    match read_value(path) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// Serialize `value` with Qt `QJsonDocument::Indented`-style 4-space indent.
/// serde_json's Map is BTreeMap-backed (no `preserve_order` feature), so keys
/// come out alphabetically sorted like QJsonObject — new files diff cleanly
/// against old ones.
pub fn to_indented_string<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    value.serialize(&mut ser)?;
    // Qt terminates the file with no trailing newline; keep it that way.
    String::from_utf8(buf).map_err(|e| serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}

/// Atomic write of a full JSON file (indented): write `<name>.tmp` next to
/// the target, then rename over it. Crash mid-write leaves the old file
/// intact; there is no half-written target (performance.md §4).
pub fn write_value_atomic<T: Serialize>(path: &Path, value: &T) -> JsonResult<()> {
    let text = to_indented_string(value).map_err(|source| JsonError::Serde {
        path: path.to_path_buf(),
        source,
    })?;
    write_text_atomic(path, text.as_bytes())
}

/// Atomic write of raw bytes (tmp + rename). Used for messages.jsonl full
/// rewrites and any single-file JSON.
pub fn write_text_atomic(path: &Path, bytes: &[u8]) -> JsonResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let tmp = tmp_path_for(path);
    fs::write(&tmp, bytes).map_err(io_err(&tmp))?;
    // std::fs::rename does not replace an existing target on Windows; remove
    // first (ignore NotFound). The tiny window between remove and rename is
    // acceptable here — readers are tolerant of a missing file.
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(path)(e)),
    }
    fs::rename(&tmp, path).map_err(io_err(path))
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wardex".to_string());
    path.with_file_name(format!("{file_name}.tmp-{}", std::process::id()))
}

/// Append one compact JSON line + '\n' to a JSONL file (the ONLY non-atomic
/// write path — data-formats.md §4.4: appended rows carry no `segments` key
/// and a crash may leave a torn last line, which loaders must skip).
pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> JsonResult<()> {
    use std::io::Write;
    let line = serde_json::to_string(value).map_err(|source| JsonError::Serde {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(io_err(path))?;
    f.write_all(line.as_bytes()).map_err(io_err(path))?;
    f.write_all(b"\n").map_err(io_err(path))
}

/// Current time as Unix milliseconds (integer form, like
/// QDateTime::currentMSecsSinceEpoch()).
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Serde deserializer for ms timestamps: accepts integer OR float JSON
/// numbers (Qt wrote integer form but read via toDouble()). Normalizes to i64.
pub fn de_ms_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let v = f64::deserialize(deserializer)?;
    Ok(v as i64)
}

/// QString::left(n) equivalent (char-based; Qt counted UTF-16 units, which
/// only differs for non-BMP characters — not a compatibility concern).
pub fn left_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Qt `left(n) + "…" if longer` pattern used for title/summary snippets.
pub fn ellipsize(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", left_chars(s, n))
    } else {
        s.to_string()
    }
}

/// Case-insensitive substring search on the ORIGINAL string, returning the
/// byte range of the match in the original (no case folding — data-formats.md
/// §10: folding can change string length and break snippet offsets).
/// Comparison is per-char lowercase equality; a needle of N chars can only
/// match an N-char window (so "ss" never matches "ß" — acceptable, see §10).
pub fn find_case_insensitive(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return Some((0, 0));
    }
    let n_chars = needle.chars().count();
    let total = haystack.chars().count();
    if n_chars > total {
        return None;
    }
    for (idx, (byte_start, _)) in haystack.char_indices().enumerate() {
        if total - idx < n_chars {
            break; // window would be shorter than the needle
        }
        let window = haystack[byte_start..].chars().take(n_chars);
        let matched = window
            .zip(needle.chars())
            .all(|(h, n)| h.to_lowercase().eq(n.to_lowercase()));
        if matched {
            let byte_end = haystack[byte_start..]
                .char_indices()
                .nth(n_chars)
                .map(|(i, _)| byte_start + i)
                .unwrap_or(haystack.len());
            return Some((byte_start, byte_end));
        }
    }
    None
}

pub fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    find_case_insensitive(haystack, needle).is_some()
}

/// Snippet with ±`context` chars around a match (data-formats.md §10), with
/// "…" markers where the snippet was cut off.
pub fn snippet_around(content: &str, match_start: usize, match_end: usize, context: usize) -> String {
    debug_assert!(match_start <= match_end && match_end <= content.len());
    // Walk back `context` chars from the match start.
    let mut from = match_start;
    for _ in 0..context {
        match content[..from].chars().next_back() {
            Some(c) => from -= c.len_utf8(),
            None => break,
        }
    }
    let mut to = match_end;
    for _ in 0..context {
        match content[to..].chars().next() {
            Some(c) => to += c.len_utf8(),
            None => break,
        }
    }
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.push_str(&content[from..to]);
    if to < content.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ci_on_original_string() {
        assert_eq!(find_case_insensitive("Hello World", "world"), Some((6, 11)));
        assert_eq!(find_case_insensitive("hELLo", "ell"), Some((1, 4)));
        assert_eq!(find_case_insensitive("中文测试ABC", "abc"), Some((12, 15)));
        assert_eq!(find_case_insensitive("abc", "z"), None);
    }

    #[test]
    fn snippet_marks_truncation() {
        let content = "abcdefghijklmnopqrstuvwxyz";
        // match "klm" at byte 10..13
        let s = snippet_around(content, 10, 13, 3);
        assert_eq!(s, "…hijklmnop…");
        let s = snippet_around(content, 0, 3, 5);
        assert_eq!(s, "abcdefgh…");
    }

    #[test]
    fn ellipsize_chars() {
        assert_eq!(ellipsize("abc", 5), "abc");
        assert_eq!(ellipsize("abcdef", 3), "abc…");
    }
}
