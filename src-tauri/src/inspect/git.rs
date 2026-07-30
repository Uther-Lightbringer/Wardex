// Git history + read-only diffs for the version-control panel
// (features/chat.md §6.3, panels.md §1.3). The branch badge itself reads
// .git/HEAD directly (store::workspace::git_branch); this module covers the
// commit list, the working-tree change list and unified-diff views.
//
// The old app ran QProcess git; here every spawn gets a hard 4s ceiling
// (performance.md §6: every external interaction has a timeout) so a wedged
// git executable can never block a UI command. git2 is deliberately NOT a
// dependency — the CLI keeps this module consistent with git_log and avoids
// a large native crate for read-only views.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;

/// Default cap of commits pulled per refresh (panels.md §1.3 budget).
pub const GIT_LOG_MAX: usize = 200;
/// R4 payload budget: one diff view never ships more than 64KB of line text.
pub const DIFF_MAX_BYTES: usize = 64 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Default, Serialize)]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// Parse one `--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s` output line.
pub fn parse_log_line(line: &str) -> Option<GitCommit> {
    let mut parts = line.splitn(5, '\x1f');
    let hash = parts.next()?.trim();
    if hash.is_empty() {
        return None;
    }
    Some(GitCommit {
        hash: hash.to_string(),
        short: parts.next().unwrap_or_default().to_string(),
        author: parts.next().unwrap_or_default().to_string(),
        date: parts.next().unwrap_or_default().to_string(),
        subject: parts.next().unwrap_or_default().to_string(),
    })
}

/// Spawn `git -C <dir> <args>` with the hard 4s ceiling; stdout on success,
/// first stderr line otherwise.
fn run_git(dir: &str, args: &[&str]) -> Result<String, String> {
    let child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 git: {e}"))?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("git 执行失败: {e}")),
        Err(_) => return Err("git 执行超时".to_string()),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or("git 执行失败").trim();
        return Err(first.to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `git -C <dir> log` with the format above. Errors come back as a short
/// Chinese-safe message (panel shows it in red); "not a git repo" is one of
/// them — the panel normally hides itself on an empty branch before calling.
pub fn git_log(dir: &str, max: usize) -> Result<Vec<GitCommit>, String> {
    if dir.is_empty() {
        return Ok(Vec::new());
    }
    let max = max.clamp(1, GIT_LOG_MAX);
    let stdout = run_git(
        dir,
        &[
            "log",
            &format!("-n{max}"),
            "--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s",
            "--date=format:%Y-%m-%d",
        ],
    )?;
    Ok(stdout.lines().filter_map(parse_log_line).collect())
}

// ---- working-tree change list (git status) ----

/// One changed path of `git status --porcelain=v1 -z`. `index` is the staged
/// column, `worktree` the unstaged one (' ' = clean, '?' = untracked);
/// `orig_path` is only set on renames.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct GitStatusEntry {
    pub path: String,
    pub index: String,
    pub worktree: String,
    pub orig_path: String,
}

/// Parse `--porcelain=v1 -z` bytes: entries are "XY <path>\0"; a rename
/// ("R" in the index column) carries a second NUL-separated field with the
/// original path.
pub fn parse_status_z(bytes: &[u8]) -> Vec<GitStatusEntry> {
    let mut out = Vec::new();
    let mut fields = bytes.split(|b| *b == 0);
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue; // "" (trailing NUL) or malformed
        }
        let index = (field[0] as char).to_string();
        let worktree = (field[1] as char).to_string();
        let path = String::from_utf8_lossy(&field[3..]).into_owned();
        if path.is_empty() {
            continue;
        }
        let mut orig_path = String::new();
        if index == "R" {
            // The original path is the NEXT NUL-separated field.
            if let Some(orig) = fields.next() {
                orig_path = String::from_utf8_lossy(orig).into_owned();
            }
        }
        out.push(GitStatusEntry {
            path,
            index,
            worktree,
            orig_path,
        });
    }
    out
}

/// Working-tree change list (unstaged + staged + untracked) for the panel's
/// 更改 view. Empty outside a git repo is reported as an error like git_log.
pub fn git_status(dir: &str) -> Result<Vec<GitStatusEntry>, String> {
    if dir.is_empty() {
        return Ok(Vec::new());
    }
    let stdout = run_git(dir, &["status", "--porcelain=v1", "-z"])?;
    Ok(parse_status_z(stdout.as_bytes()))
}

// ---- unified diffs ----

/// One line of a rendered diff. `kind`: "meta" (diff --git / index / --- /
/// +++ headers), "hunk" (@@ header), "add", "del", "ctx", "eof" (\ No
/// newline). Line numbers are 1-based; absent on meta/hunk/eof lines and on
/// the side the line does not belong to.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DiffLine {
    pub kind: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub text: String,
}

/// Per-file section of a diff payload (a commit diff may touch many files).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub binary: bool,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct GitDiff {
    pub files: Vec<FileDiff>,
    /// R4: the payload hit the 64KB budget and was cut short.
    pub truncated: bool,
}

/// Parse unified-diff text into per-file sections with 1-based line numbers.
/// Unknown lines are kept as "meta" so nothing the user might need is dropped.
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;
    let mut in_hunks = false;

    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            // Path comes from the b/ side; renames show the new name here.
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, p)| p.to_string())
                .unwrap_or_else(|| rest.to_string());
            files.push(FileDiff {
                path,
                binary: false,
                lines: vec![DiffLine {
                    kind: "meta".to_string(),
                    old_lineno: None,
                    new_lineno: None,
                    text: raw.to_string(),
                }],
            });
            in_hunks = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue; // preamble before the first diff --git (shouldn't happen)
        };
        if raw.starts_with("Binary files ") {
            file.binary = true;
            file.lines.push(DiffLine {
                kind: "meta".to_string(),
                old_lineno: None,
                new_lineno: None,
                text: raw.to_string(),
            });
            continue;
        }
        if let Some(hunk) = raw.strip_prefix("@@ ") {
            // "@@ -a[,b] +c[,d] @@ [section]" — counts default to 1.
            if let Some((o, n)) = parse_hunk_header(hunk) {
                old_no = o;
                new_no = n;
            }
            in_hunks = true;
            file.lines.push(DiffLine {
                kind: "hunk".to_string(),
                old_lineno: None,
                new_lineno: None,
                text: raw.to_string(),
            });
            continue;
        }
        if !in_hunks {
            // index / --- / +++ / old mode / ... header lines.
            file.lines.push(DiffLine {
                kind: "meta".to_string(),
                old_lineno: None,
                new_lineno: None,
                text: raw.to_string(),
            });
            continue;
        }
        let (kind, text) = match raw.chars().next() {
            Some('+') => ("add", &raw[1..]),
            Some('-') => ("del", &raw[1..]),
            Some(' ') => ("ctx", &raw[1..]),
            Some('\\') => ("eof", raw),
            _ => ("ctx", raw), // tolerate empty context lines
        };
        let (old_lineno, new_lineno) = match kind {
            "add" => {
                let n = new_no;
                new_no += 1;
                (None, Some(n))
            }
            "del" => {
                let o = old_no;
                old_no += 1;
                (Some(o), None)
            }
            "ctx" => {
                let (o, n) = (old_no, new_no);
                old_no += 1;
                new_no += 1;
                (Some(o), Some(n))
            }
            _ => (None, None),
        };
        file.lines.push(DiffLine {
            kind: kind.to_string(),
            old_lineno,
            new_lineno,
            text: text.to_string(),
        });
    }
    files
}

/// "−a[,b] +c[,d] @@" tail of a hunk header → (old_start, new_start).
fn parse_hunk_header(h: &str) -> Option<(u32, u32)> {
    let h = h.strip_prefix('-')?;
    let (old_part, rest) = h.split_once(' ')?;
    let new_part = rest.strip_prefix('+')?.split(' ').next()?;
    let old_start = old_part.split(',').next()?.parse().ok()?;
    let new_start = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// R4 budget: keep whole lines until the accumulated line text would exceed
/// `budget` bytes, then cut and flag. Files past the cut are dropped whole.
pub fn truncate_diff(files: &mut Vec<FileDiff>, budget: usize) -> bool {
    let mut used = 0usize;
    for fi in 0..files.len() {
        for li in 0..files[fi].lines.len() {
            let cost = files[fi].lines[li].text.len() + 1;
            if used + cost > budget {
                files[fi].lines.truncate(li);
                files.truncate(fi + 1);
                return true;
            }
            used += cost;
        }
    }
    false
}

fn build_diff(mut files: Vec<FileDiff>) -> GitDiff {
    let truncated = truncate_diff(&mut files, DIFF_MAX_BYTES);
    GitDiff { files, truncated }
}

/// Synthetic all-added diff for an untracked file: git diff has no
/// representation for it (no /dev/null trickery on Windows), so the file is
/// read directly; NUL bytes in the head mark it binary. Capped by R4 later.
fn untracked_diff(dir: &str, path: &str) -> FileDiff {
    let full = std::path::Path::new(dir).join(path);
    let mut file = FileDiff {
        path: path.to_string(),
        binary: false,
        lines: Vec::new(),
    };
    let Ok(bytes) = std::fs::read(&full) else {
        file.lines.push(DiffLine {
            kind: "meta".to_string(),
            old_lineno: None,
            new_lineno: None,
            text: "（无法读取文件）".to_string(),
        });
        return file;
    };
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        file.binary = true;
        file.lines.push(DiffLine {
            kind: "meta".to_string(),
            old_lineno: None,
            new_lineno: None,
            text: "（二进制文件）".to_string(),
        });
        return file;
    }
    let text = String::from_utf8_lossy(&bytes);
    for (i, line) in text.lines().enumerate() {
        file.lines.push(DiffLine {
            kind: "add".to_string(),
            old_lineno: None,
            new_lineno: Some(i as u32 + 1),
            text: line.to_string(),
        });
    }
    file
}

/// Diff of one path. `staged` → index vs HEAD (`git diff --cached`);
/// otherwise worktree vs HEAD (`git diff HEAD`, covering staged+unstaged,
/// with a `git diff` fallback for repos without any commit yet). Untracked
/// files get the synthetic all-added view.
pub fn git_diff_file(dir: &str, path: &str, staged: bool) -> Result<GitDiff, String> {
    if dir.is_empty() || path.is_empty() {
        return Ok(GitDiff::default());
    }
    if staged {
        let out = run_git(dir, &["diff", "--cached", "--", path])?;
        return Ok(build_diff(parse_unified_diff(&out)));
    }
    // HEAD may not exist yet (fresh repo): fall back to the plain worktree
    // diff which never needs a commit.
    let out = run_git(dir, &["diff", "HEAD", "--", path])
        .or_else(|_| run_git(dir, &["diff", "--", path]))?;
    Ok(build_diff(parse_unified_diff(&out)))
}

/// Diff of one path when the change list says it is untracked.
pub fn git_diff_untracked(dir: &str, path: &str) -> GitDiff {
    build_diff(vec![untracked_diff(dir, path)])
}

/// Command entry for the panel: `mode` is "worktree" (default), "staged"
/// or "untracked" (the frontend classifies from the status columns).
pub fn git_diff_file_mode(dir: &str, path: &str, mode: &str) -> Result<GitDiff, String> {
    match mode {
        "staged" => git_diff_file(dir, path, true),
        "untracked" => Ok(git_diff_untracked(dir, path)),
        _ => git_diff_file(dir, path, false),
    }
}

/// Diff of one commit vs its parent (`git show --format=`). Merges render
/// empty (combined diff suppressed) — acceptable for a read-only viewer.
pub fn git_diff_commit(dir: &str, hash: &str) -> Result<GitDiff, String> {
    if dir.is_empty() {
        return Ok(GitDiff::default());
    }
    // The hash is interpolated as a bare argv word; lock it to hex so no
    // option/revision injection is possible.
    if hash.len() < 4 || hash.len() > 40 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("非法提交哈希".to_string());
    }
    let out = run_git(dir, &["show", "--format=", "--no-color", hash])?;
    Ok(build_diff(parse_unified_diff(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_format_line() {
        let c = parse_log_line("abc123\x1fabc1\x1fAlice\x1f2026-07-29\x1ffix bug").unwrap();
        assert_eq!(c.hash, "abc123");
        assert_eq!(c.short, "abc1");
        assert_eq!(c.author, "Alice");
        assert_eq!(c.date, "2026-07-29");
        assert_eq!(c.subject, "fix bug");
        // Subjects may contain the separator only if forged; splitn(5) keeps
        // everything after the 4th separator in the subject.
        let c = parse_log_line("h\x1fs\x1fa\x1fd\x1fsub\x1fject").unwrap();
        assert_eq!(c.subject, "sub\x1fject");
        assert!(parse_log_line("").is_none());
        assert!(parse_log_line("\x1fs\x1fa\x1fd\x1fs").is_none());
    }

    #[test]
    fn empty_dir_is_empty_list() {
        assert!(git_log("", 50).unwrap().is_empty());
    }

    #[test]
    fn parses_status_z_including_renames_and_untracked() {
        // "M  staged" / " M worktree" / "?? new" / "R  new\0old"
        let z = b"M  staged.txt\0 M dirty.txt\0?? new file.txt\0R  renamed.txt\0old.txt\0";
        let out = parse_status_z(z);
        assert_eq!(
            out,
            vec![
                GitStatusEntry {
                    path: "staged.txt".into(),
                    index: "M".into(),
                    worktree: " ".into(),
                    orig_path: String::new(),
                },
                GitStatusEntry {
                    path: "dirty.txt".into(),
                    index: " ".into(),
                    worktree: "M".into(),
                    orig_path: String::new(),
                },
                GitStatusEntry {
                    path: "new file.txt".into(),
                    index: "?".into(),
                    worktree: "?".into(),
                    orig_path: String::new(),
                },
                GitStatusEntry {
                    path: "renamed.txt".into(),
                    index: "R".into(),
                    worktree: " ".into(),
                    orig_path: "old.txt".into(),
                },
            ]
        );
        assert!(parse_status_z(b"").is_empty());
        assert!(parse_status_z(b"\0").is_empty());
    }

    #[test]
    fn parses_unified_diff_with_line_numbers() {
        let text = "\
diff --git a/a.txt b/a.txt
index 111..222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,3 +1,4 @@
 ctx
-del
+add1
+add2
 tail
\\ No newline at end of file
diff --git a/b.bin b/b.bin
Binary files a/b.bin and b/b.bin differ
";
        let files = parse_unified_diff(text);
        assert_eq!(files.len(), 2);
        let f = &files[0];
        assert_eq!(f.path, "a.txt");
        assert!(!f.binary);
        // meta: diff --git / index / --- / +++ = 4, then the hunk header.
        assert_eq!(f.lines[0].kind, "meta");
        assert_eq!(f.lines[4].kind, "hunk");
        assert_eq!(
            f.lines[5],
            DiffLine {
                kind: "ctx".into(),
                old_lineno: Some(1),
                new_lineno: Some(1),
                text: "ctx".into()
            }
        );
        assert_eq!(
            f.lines[6],
            DiffLine {
                kind: "del".into(),
                old_lineno: Some(2),
                new_lineno: None,
                text: "del".into()
            }
        );
        assert_eq!(
            f.lines[7],
            DiffLine {
                kind: "add".into(),
                old_lineno: None,
                new_lineno: Some(2),
                text: "add1".into()
            }
        );
        assert_eq!(f.lines[8].new_lineno, Some(3));
        assert_eq!(
            f.lines[9],
            DiffLine {
                kind: "ctx".into(),
                old_lineno: Some(3),
                new_lineno: Some(4),
                text: "tail".into()
            }
        );
        assert_eq!(f.lines[10].kind, "eof");
        assert!(files[1].binary, "Binary files marker flips the flag");
    }

    #[test]
    fn truncate_diff_cuts_at_budget_and_flags() {
        let mk = |n: usize| FileDiff {
            path: format!("f{n}.txt"),
            binary: false,
            lines: (0..4)
                .map(|i| DiffLine {
                    kind: "add".into(),
                    old_lineno: None,
                    new_lineno: Some(i as u32),
                    text: "0123456789".into(), // 10 bytes + 1 = 11 per line
                })
                .collect(),
        };
        let mut files = vec![mk(1), mk(2)];
        // 8 lines * 11 = 88 bytes total; budget 30 -> cut mid file 1.
        assert!(truncate_diff(&mut files, 30));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].lines.len(), 2, "two 11-byte lines fit in 30");

        let mut files = vec![mk(1)];
        assert!(!truncate_diff(&mut files, 1024), "nothing cut -> not flagged");
        assert_eq!(files[0].lines.len(), 4);
    }

    // ---- end-to-end against a real temporary repo (needs git on PATH) ----

    /// tempdir repo with one committed file, one dirty, one staged, one
    /// untracked; returns (dir, head_hash).
    fn fixture_repo() -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .expect("git spawn");
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(tmp.path().join("a.txt"), "l1\nl2\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(tmp.path().join("a.txt"), "l1\nl2 changed\nl3\n").unwrap();
        std::fs::write(tmp.path().join("staged.txt"), "s1\n").unwrap();
        git(&["add", "staged.txt"]);
        std::fs::write(tmp.path().join("new.txt"), "n1\nn2\n").unwrap();
        let head = run_git(&dir, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        (tmp, head)
    }

    #[test]
    fn status_diffs_and_commit_diff_end_to_end() {
        let (tmp, head) = fixture_repo();
        let dir = tmp.path().to_string_lossy().into_owned();

        let status = git_status(&dir).unwrap();
        let by_path = |p: &str| status.iter().find(|e| e.path == p).unwrap().clone();
        assert_eq!(by_path("a.txt").worktree, "M");
        assert_eq!(by_path("staged.txt").index, "A");
        assert_eq!(by_path("new.txt").index, "?");

        // Worktree diff vs HEAD covers the dirty file.
        let d = git_diff_file(&dir, "a.txt", false).unwrap();
        assert_eq!(d.files.len(), 1);
        assert!(d.files[0]
            .lines
            .iter()
            .any(|l| l.kind == "add" && l.text == "l2 changed"));
        assert!(!d.truncated);
        // Worktree diff of the staged new file (HEAD mode) shows it as added.
        let d = git_diff_file(&dir, "staged.txt", false).unwrap();
        assert!(d.files[0].lines.iter().any(|l| l.kind == "add" && l.text == "s1"));
        // Staged mode: only the index vs HEAD.
        let d = git_diff_file(&dir, "staged.txt", true).unwrap();
        assert!(d.files[0].lines.iter().any(|l| l.kind == "add"));
        let d = git_diff_file(&dir, "a.txt", true).unwrap();
        assert!(d.files.is_empty(), "a.txt has no staged changes");
        // Untracked file: synthetic all-added view.
        let d = git_diff_untracked(&dir, "new.txt");
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].lines.len(), 2);
        assert!(d.files[0].lines.iter().all(|l| l.kind == "add"));

        // Commit diff vs parent.
        let d = git_diff_commit(&dir, &head).unwrap();
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path, "a.txt");
        assert!(d.files[0].lines.iter().any(|l| l.kind == "add" && l.text == "l2"));
        // Injection-shaped hashes are rejected before touching git.
        assert!(git_diff_commit(&dir, "--help").is_err());
        assert!(git_diff_commit(&dir, "HEAD~1").is_err());
        assert!(git_diff_commit(&dir, "abc").is_err(), "too short");
    }
}
