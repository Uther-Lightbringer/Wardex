// Git history for the version-control panel (features/chat.md §6.3). The
// branch badge itself reads .git/HEAD directly (store::workspace::git_branch);
// this module only covers the read-only commit list via `git log --format`.
//
// The old app ran QProcess git; here the spawn gets a hard 4s ceiling
// (performance.md §6: every external interaction has a timeout) so a wedged
// git executable can never block a UI command.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;

/// Default cap of commits pulled per refresh (panels.md §1.3 budget).
pub const GIT_LOG_MAX: usize = 200;
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

/// `git -C <dir> log` with the format above. Errors come back as a short
/// Chinese-safe message (panel shows it in red); "not a git repo" is one of
/// them — the panel normally hides itself on an empty branch before calling.
pub fn git_log(dir: &str, max: usize) -> Result<Vec<GitCommit>, String> {
    if dir.is_empty() {
        return Ok(Vec::new());
    }
    let max = max.clamp(1, GIT_LOG_MAX);
    let child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("log")
        .arg(format!("-n{max}"))
        .arg("--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s")
        .arg("--date=format:%Y-%m-%d")
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
        Err(_) => return Err("git log 超时".to_string()),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or("git log 失败").trim();
        return Err(first.to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().filter_map(parse_log_line).collect())
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
}
