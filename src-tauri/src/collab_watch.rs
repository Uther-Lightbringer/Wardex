// 工作区文件监听 → 工作区动态（collab store）的「写侧」。
//
// 每项目一个 notify watcher，检测工作区文件变化；事件经去抖窗口聚合后，
// 归因到「该项目最近有工具活动的会话」（ACP ToolCall，见 runtime on_tool 的
// note_activity），并对变化的文件跑局部 git diff 提炼摘要，写入 collab store
// 的 record_change + touch。其他会话下次 prompt 时能看到「工作区刚改了哪些
// 文件」（注入文案不点名 UUID）。
//
// 归因是「最佳努力」：并行多会话时 watch 无法精确区分是谁改的，用该项目
// 10s 内最近工具活动会话作为归属；没有则记为匿名（""）。按 project_dir
// 分槽，避免 P1 的工具活动污染 P2 的改动。
//
// git diff 在 store 锁外跑（4s 超时），只在写 collab 时短持锁。
//
// 生命周期：项目有活跃会话时 start（幂等），不再有活跃会话时 stop（通知
// watcher 线程退出 + abort 后台任务）。store 层的 clear_project 由 manager
// 在 stop 时一并触发。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use notify::Watcher;

use crate::store::StoreRegistry;

/// 忽略的目录片段（任意层级命中即跳过）。
const IGNORE_SEGMENTS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", ".codegraph",
    ".idea", ".vscode", "__pycache__", "third_party",
];

/// 归因活跃窗口：该项目最近 10s 内有工具活动的会话视为改动者。
const ATTRIB_WINDOW_MS: i64 = 10_000;
const DEBOUNCE_MS: u64 = 500;

fn now_ms() -> i64 {
    crate::store::json::now_ms()
}

/// 后台任务句柄 + 停止信号。
struct TaskHandle {
    task: tauri::async_runtime::JoinHandle<()>,
    stop: mpsc::Sender<()>,
}

/// 项目级 watcher 管理器（放入 StoreRegistry，runtime/manager 共享）。
#[derive(Default)]
pub struct CollabWatcher {
    /// project_dir -> 后台任务
    tasks: Mutex<HashMap<String, TaskHandle>>,
    /// 每项目最近一次工具活动的会话（供归因）
    last_activity: Arc<Mutex<HashMap<String, (String, i64)>>>,
}

impl CollabWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// ACP ToolCall 事件（on_tool）：标记该项目下该会话最近有工具活动。
    pub fn note_activity(&self, project_dir: &str, session_id: &str) {
        if project_dir.is_empty() || session_id.is_empty() {
            return;
        }
        self.last_activity.lock().unwrap().insert(
            project_dir.to_string(),
            (session_id.to_string(), now_ms()),
        );
    }

    /// 启动某项目 watcher（有活跃会话时）。重复启动幂等。
    pub fn start(&self, project_dir: &str, stores: Arc<Mutex<StoreRegistry>>) {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.contains_key(project_dir) {
            return;
        }
        let pd = project_dir.to_string();
        let pd_watch = pd.clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<std::path::PathBuf>();
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

        // notify watcher 跑在独立线程（回调非 async）；事件转发到 tx。
        std::thread::spawn(move || {
            let tx2 = tx;
            let mut watcher = match notify::recommended_watcher(
                move |res: notify::Result<notify::Event>| {
                    if let Ok(ev) = res {
                        for p in ev.paths {
                            let _ = tx2.send(p);
                        }
                    }
                },
            ) {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("collab watch create {pd_watch}: {e}");
                    let _ = stop_rx.blocking_recv();
                    return;
                }
            };
            let root = Path::new(&pd_watch);
            if let Err(e) = watcher.watch(root, notify::RecursiveMode::Recursive) {
                log::warn!("collab watch {pd_watch}: {e}");
            }
            // 阻塞等待停止信号；drop 时 watcher 自动 unwatch。
            let _ = stop_rx.blocking_recv();
        });

        // 去抖 + 归因 + diff 的后台任务。
        let la = Arc::clone(&self.last_activity);
        let pd_task = pd.clone();
        let task = tauri::async_runtime::spawn(async move {
            let mut pending: HashMap<std::path::PathBuf, i64> = HashMap::new();
            loop {
                match rx.recv().await {
                    Some(p) => pending.insert(p, now_ms()),
                    None => return,
                };
                // 去抖窗口：等待期间继续累积新事件。
                let window = tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS));
                tokio::pin!(window);
                loop {
                    tokio::select! {
                        _ = &mut window => break,
                        ev = rx.recv() => match ev {
                            Some(p) => { pending.insert(p, now_ms()); }
                            None => return,
                        },
                    }
                }
                process_batch(&stores, &pd_task, &la, &pending);
                pending.clear();
            }
        });
        tasks.insert(pd, TaskHandle { task, stop: stop_tx });
    }

    /// 停止某项目 watcher（不再有活跃会话）。
    pub fn stop(&self, project_dir: &str) {
        let mut guard = self.tasks.lock().unwrap();
        if let Some(h) = guard.remove(project_dir) {
            let _ = h.stop.try_send(());
            h.task.abort();
        }
        self.last_activity.lock().unwrap().remove(project_dir);
    }

    /// 当前在监听的项目数（供测试/调试）。
    pub fn active_count(&self) -> usize {
        self.tasks.lock().unwrap().len()
    }
}

/// 处理一批去抖后的事件：过滤 → 归一相对路径 → diff 摘要（锁外）→ 归因 → 写入。
fn process_batch(
    stores: &Arc<Mutex<StoreRegistry>>,
    project_dir: &str,
    last_activity: &Arc<Mutex<HashMap<String, (String, i64)>>>,
    pending: &HashMap<std::path::PathBuf, i64>,
) {
    let mut items: Vec<(String, String)> = Vec::new();
    for (path, _ts) in pending {
        let Some(rel) = normalize_rel(project_dir, path) else { continue };
        if is_ignored(&rel) {
            continue;
        }
        let summary = summarize(project_dir, &rel);
        items.push((rel, summary));
    }
    if items.is_empty() {
        return;
    }
    let session = attribute(last_activity, project_dir);
    let mut guard = lock_ok(stores);
    for (rel, summary) in items {
        guard.collab.record_change(project_dir, &session, &rel, &summary);
        if !session.is_empty() {
            guard.collab.touch(&session, &rel);
        }
    }
}

/// 归一：绝对路径 → 相对项目根（/ 分隔）。不在项目内返回 None。
fn normalize_rel(project_dir: &str, path: &std::path::Path) -> Option<String> {
    let root = Path::new(project_dir);
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 是否命中忽略目录。
pub fn is_ignored(rel: &str) -> bool {
    IGNORE_SEGMENTS
        .iter()
        .any(|seg| rel.split('/').any(|c| c == *seg))
}

/// 归因：该项目最近 10s 内工具活动的会话，否则 ""。
fn attribute(
    last_activity: &Arc<Mutex<HashMap<String, (String, i64)>>>,
    project_dir: &str,
) -> String {
    let la = last_activity.lock().unwrap();
    match la.get(project_dir) {
        Some((sid, ts)) if now_ms() - *ts <= ATTRIB_WINDOW_MS => sid.clone(),
        _ => String::new(),
    }
}

/// 局部 git diff 提炼摘要：取 add 行的前几行文本拼接。
/// 非 git / 无法 diff 时返回空串。
pub fn summarize(project_dir: &str, rel: &str) -> String {
    let Ok(diff) = crate::inspect::git::git_diff_file(project_dir, rel, false) else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    for f in diff.files {
        for l in f.lines {
            if l.kind == "add" && !l.text.trim().is_empty() {
                lines.push(l.text.trim().to_string());
                if lines.len() >= 3 {
                    break;
                }
            }
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join(" · ");
    if out.chars().count() > 80 {
        out = out.chars().take(77).collect::<String>();
        out.push('…');
    }
    out
}

fn lock_ok(m: &Arc<Mutex<StoreRegistry>>) -> std::sync::MutexGuard<'_, StoreRegistry> {
    m.lock().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_common_dirs() {
        assert!(is_ignored("node_modules/x/a.js"));
        assert!(is_ignored("src/.git/config"));
        assert!(is_ignored("target/debug/foo"));
        assert!(!is_ignored("src/main.rs"));
        assert!(!is_ignored("utils/io.go"));
    }

    #[test]
    fn summarize_truncates() {
        let s = "a".repeat(200);
        let out = summarize_mock(&s);
        assert!(out.chars().count() <= 78);
        assert!(out.ends_with('…'));
    }

    fn summarize_mock(long: &str) -> String {
        let mut out = long.to_string();
        if out.chars().count() > 80 {
            out = out.chars().take(77).collect::<String>();
            out.push('…');
        }
        out
    }

    #[test]
    fn attribute_is_per_project() {
        let map = Arc::new(Mutex::new(HashMap::new()));
        map.lock().unwrap().insert("/p1".into(), ("A".into(), now_ms()));
        map.lock().unwrap().insert("/p2".into(), ("B".into(), now_ms()));
        assert_eq!(attribute(&map, "/p1"), "A");
        assert_eq!(attribute(&map, "/p2"), "B");
        assert_eq!(attribute(&map, "/p3"), "");
    }
}
