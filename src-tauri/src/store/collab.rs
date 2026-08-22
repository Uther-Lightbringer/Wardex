// 工作区动态（monitor 多步兵并行时的态势，不是会话互访）。
//
// 问题：同项目多个会话同时改文件，下一个动手的 agent 不知道工作区刚发生了
// 什么。机制：记录「文件改动 + 尽力归因 + git 摘要」，下次 prompt 时把「自
// 上次以来别人的改动」附上，让它避让或先 git diff。
//
// 设计要点：
//   - 全局 `changes`（文件改动）。`session_id` 是监控层的尽力归因，未知为 ""；
//     注入文案不点名 UUID（不准就不装准），监控页仍可显示供人看。
//   - 每会话 `cursor`：上次注入的 `last_id` + 该会话已涉足的 files/dirs。
//   - 注入：跳过自己的改动。尚无涉足范围（冷启动）→ 展开最近几条；有范围 →
//     相关的展开、无关的压成一行。然后 `last_id` 前移（含已跳过的），避免重复。
//   - 落盘 collab.json；纯数据层，写侧在 collab_watch.rs。
//
// 清理：会话销毁 `clear_session`；项目无活跃会话 `clear_project`。不设 TTL。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Paths;

/// 一条工作区文件改动记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabChange {
    /// 全局递增序号，注入游标用它。
    pub id: u64,
    pub project_dir: String,
    /// 归因到的会话 id；"" = 未能归因（仅时间窗粗归因）。
    pub session_id: String,
    /// 相对项目根的文件路径（统一用 `/` 分隔，便于比较）。
    pub path: String,
    /// 改动摘要（git diff 的 + 行提炼；非 git 为空）。
    pub summary: String,
    /// 毫秒时间戳。
    pub ts: i64,
}

/// 每个会话的注入游标 + 涉足范围。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollabCursor {
    /// 上次注入到的改动序号。
    pub last_id: u64,
    /// 该会话已涉足的文件（相对路径，/ 分隔）。
    pub files: Vec<String>,
    /// 该会话已涉足的目录前缀（/ 分隔，命中子路径）。
    pub dirs: Vec<String>,
}

#[derive(Debug)]
pub struct CollabStore {
    path: PathBuf,
    changes: Vec<CollabChange>,
    cursors: HashMap<String, CollabCursor>,
    next_id: u64,
}

const MAX_CHANGES_PER_PROJECT: usize = 200;
/// 冷启动（尚无涉足范围）最多展开这么多条近期改动。
const COLD_START_LIMIT: usize = 8;

impl CollabStore {
    pub fn load(paths: &Paths) -> Self {
        let path = paths.collab_path();
        let file: Option<CollabFile> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let file = file.unwrap_or_default();
        let next_id = file
            .changes
            .iter()
            .map(|c| c.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Self {
            path,
            changes: file.changes,
            cursors: file.cursors,
            next_id,
        }
    }

    /// 上报一条改动（由监控层调用）。summary 由调用方提炼。
    pub fn record_change(&mut self, project_dir: &str, session_id: &str, path: &str, summary: &str) {
        self.trim_project(&project_dir);
        self.changes.push(CollabChange {
            id: self.next_id,
            project_dir: project_dir.to_string(),
            session_id: session_id.to_string(),
            path: path.to_string(),
            summary: summary.to_string(),
            ts: super::json::now_ms(),
        });
        self.next_id += 1;
        self.save();
    }

    /// 上报会话 B 的涉足文件/目录（监控层从工具调用/改动归因得来）。
    pub fn touch(&mut self, session_id: &str, path: &str) {
        let c = self.cursors.entry(session_id.to_string()).or_default();
        if c.files.contains(&path.to_string()) {
            return;
        }
        c.files.push(path.to_string());
        let dir = match path.rfind('/') {
            Some(i) if i > 0 => &path[..i],
            _ => "",
        };
        if !dir.is_empty() && !c.dirs.contains(&dir.to_string()) {
            c.dirs.push(dir.to_string());
        }
        self.save();
    }

    /// 该会话的上次注入游标。
    pub fn last_id(&self, session_id: &str) -> u64 {
        self.cursors.get(session_id).map(|c| c.last_id).unwrap_or(0)
    }

    /// 生成给该会话的工作区动态文本；读完后把游标前移到当前该项目最新。
    /// 跳过自己的改动。冷启动展开最近几条，之后按涉足范围过滤。
    /// 返回空串 = 没有值得注入的内容。
    pub fn inject_ctx(&mut self, session_id: &str, project_dir: &str) -> String {
        let now = super::json::now_ms();
        let project_max = self
            .changes
            .iter()
            .filter(|c| c.project_dir == project_dir)
            .map(|c| c.id)
            .max()
            .unwrap_or(0);
        let (lines, omitted, dirty) = {
            let cur = self.cursors.entry(session_id.to_string()).or_default();
            let from = cur.last_id;
            let cold = cur.files.is_empty() && cur.dirs.is_empty();
            let pending: Vec<&CollabChange> = self
                .changes
                .iter()
                .filter(|c| {
                    c.project_dir == project_dir && c.id > from && c.session_id != session_id
                })
                .collect();
            let mut lines: Vec<String> = Vec::new();
            let mut omitted = 0usize;
            if cold {
                omitted = pending.len().saturating_sub(COLD_START_LIMIT);
                for ch in pending.iter().skip(omitted) {
                    lines.push(format_change(ch, now));
                }
            } else {
                for ch in pending {
                    if Self::related(cur, &ch.path) {
                        lines.push(format_change(ch, now));
                    } else {
                        omitted += 1;
                    }
                }
            }
            let dirty = project_max > cur.last_id;
            if dirty {
                cur.last_id = project_max;
            }
            (lines, omitted, dirty)
        };
        if dirty {
            self.save();
        }
        if lines.is_empty() && omitted == 0 {
            return String::new();
        }
        let mut out = String::from("【工作区动态】其他会话近期改动，注意避让或先看 git diff。");
        if !lines.is_empty() {
            out.push('\n');
            out.push_str(&lines.join("\n"));
        }
        if omitted > 0 {
            out.push_str(&format!(
                "\n（另有 {omitted} 处与你当前区域无关的改动，需要时可 git diff 查看）"
            ));
        }
        out
    }

    fn related(cur: &CollabCursor, path: &str) -> bool {
        if cur.files.iter().any(|f| f == path) {
            return true;
        }
        cur.dirs
            .iter()
            .any(|d| path == d || path.starts_with(&format!("{d}/")))
    }

    /// 会话销毁：清该会话的游标 + 涉足范围。残留数据不再占内存。
    pub fn clear_session(&mut self, session_id: &str) {
        self.cursors.remove(session_id);
        self.save();
    }

    /// 项目不再有活跃会话：清该项目的全部改动记录。
    pub fn clear_project(&mut self, project_dir: &str) {
        let before = self.changes.len();
        self.changes.retain(|c| c.project_dir != project_dir);
        if self.changes.len() != before {
            self.save();
        }
    }

    /// 该项目改动记录条数（供上层判断是否还需保留）。
    pub fn project_len(&self, project_dir: &str) -> usize {
        self.changes.iter().filter(|c| c.project_dir == project_dir).count()
    }

    /// 该项目最近 N 条改动（新的在前），供前端展示工作区动态。
    pub fn recent(&self, project_dir: &str, limit: usize) -> Vec<CollabChange> {
        self.changes
            .iter()
            .filter(|c| c.project_dir == project_dir)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    fn trim_project(&mut self, project_dir: &str) {
        // 每个项目最多保留 MAX_CHANGES_PER_PROJECT 条，超出丢最旧。
        while self.project_len(project_dir) >= MAX_CHANGES_PER_PROJECT {
            if let Some(pos) = self
                .changes
                .iter()
                .position(|c| c.project_dir == project_dir)
            {
                self.changes.remove(pos);
            } else {
                break;
            }
        }
    }

    fn save(&self) {
        let file = CollabFile {
            changes: self.changes.clone(),
            cursors: self.cursors.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&file) {
            write_atomic(&self.path, &bytes);
        }
    }
}

fn format_change(ch: &CollabChange, now: i64) -> String {
    let mins = (now - ch.ts) / 60_000;
    let when = if mins < 1 {
        "刚刚".to_string()
    } else {
        format!("{mins} 分钟前")
    };
    let mut line = format!("- {}（{}）", ch.path, when);
    if !ch.summary.is_empty() {
        line.push_str(&format!(" → {}", ch.summary));
    }
    line
}

#[derive(Serialize, Deserialize, Default)]
struct CollabFile {
    changes: Vec<CollabChange>,
    cursors: HashMap<String, CollabCursor>,
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::paths::Paths;

    fn store(tmp: &tempfile::TempDir) -> CollabStore {
        let paths = Paths::new(tmp.path().to_path_buf());
        CollabStore::load(&paths)
    }

    #[test]
    fn inject_only_related_and_advances_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(&tmp);
        let proj = "/p";
        s.record_change(proj, "A", "config.ts", "超时 5s→10s");
        s.record_change(proj, "A", "utils/io.go", "新增 WriteBuf");
        // B 只涉足 utils/，io.go 展开；config.ts 无关（兜底）。
        s.touch("B", "utils/io.go");
        let ctx = s.inject_ctx("B", proj);
        assert!(ctx.contains("工作区动态"), "{ctx}");
        assert!(ctx.contains("utils/io.go"), "{ctx}");
        assert!(ctx.contains("WriteBuf"), "{ctx}");
        assert!(ctx.contains("无关"), "{ctx}");
        assert!(!ctx.contains("config.ts"), "{ctx}");
        assert!(!ctx.contains("会话 A"), "{ctx}");
        // 游标前移后，再次注入为空。
        assert_eq!(s.inject_ctx("B", proj), "");
    }

    #[test]
    fn inject_skips_own_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(&tmp);
        let proj = "/p";
        s.record_change(proj, "B", "utils/io.go", "B-own");
        s.touch("B", "utils/io.go");
        s.record_change(proj, "A", "utils/io.go", "A-other");
        let ctx = s.inject_ctx("B", proj);
        assert!(!ctx.contains("B-own"), "{ctx}");
        assert!(ctx.contains("A-other"), "{ctx}");
    }

    #[test]
    fn cold_start_shows_recent_without_touch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(&tmp);
        let proj = "/p";
        s.record_change(proj, "A", "config.ts", "超时 5s→10s");
        // B 从未 touch：冷启动仍能看到近期改动。
        let ctx = s.inject_ctx("B", proj);
        assert!(ctx.contains("config.ts"), "{ctx}");
        assert!(ctx.contains("超时"), "{ctx}");
        assert_eq!(s.inject_ctx("B", proj), "");
    }

    #[test]
    fn own_only_changes_inject_empty_but_advance() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(&tmp);
        s.record_change("/p", "B", "a.txt", "x");
        s.touch("B", "a.txt");
        assert_eq!(s.inject_ctx("B", "/p"), "");
        assert!(s.last_id("B") > 0);
    }

    #[test]
    fn clear_session_and_project() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(&tmp);
        s.record_change("/p", "A", "a.txt", "");
        s.touch("B", "b.txt");
        assert_eq!(s.project_len("/p"), 1);
        s.clear_session("B");
        assert!(s.last_id("B") == 0);
        s.clear_project("/p");
        assert_eq!(s.project_len("/p"), 0);
    }

    #[test]
    fn cursor_persists_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        {
            let mut s = CollabStore::load(&paths);
            s.record_change("/p", "A", "a.txt", "x");
            s.touch("B", "a.txt");
            s.inject_ctx("B", "/p");
        }
        let s2 = CollabStore::load(&paths);
        assert_eq!(s2.last_id("B"), 1);
    }
}
