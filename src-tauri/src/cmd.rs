// Terminal-command runner (Composer `!` prefix).
// One run per session; a run owns a cmd.exe child (CREATE_NO_WINDOW, so no
// console flashes), streams merged stdout/stderr to the frontend as 50ms
// flushes (R1-style chunk events), and persists ONE chat row per run:
//   - created at start (kind=="command", role "user", status "streaming",
//     content = the command) via store::sessions::append_message,
//   - output chunks go into the row's trailing text segment in memory
//     (store::sessions::append_command_output, head-capped at 64KB),
//   - at exit the final snapshot (status done/error/interrupted + exitCode)
//     is REWRITTEN to disk once (finalize_command_row) and pushed as
//     chat://bubbleSet.
//
// Events:
//   term://output {sessionId, runId, rowId, text}   (50ms merged chunks)
//   term://exit   {sessionId, runId, rowId, code, killed, truncated}
//
// The output is NEVER sent to the agent (privacy + tokens): `!` is a purely
// local action. stdin is closed, so interactive commands (git commit without
// -m, node REPL) fail fast instead of hanging.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::chat::runtime::{lock_ok, row_json};
use crate::store::sessions::{MessageRow, CMD_OUTPUT_MAX};
use crate::store::StoreRegistry;

/// Hard ceiling of concurrent runs across all sessions (each may keep
/// grandchildren alive; the ACP process cap sets the precedent).
const MAX_TOTAL_RUNS: usize = 8;
/// Chunk merge flush cadence (mirrors the ACP merger; term://output never
/// fires more often than this).
const FLUSH_MS: u64 = 50;
/// Output tail budget of the [Wardex 终端] context block injected into the
/// next user prompt (token control; the chat row itself keeps the full 64KB).
const CMD_CTX_MAX: usize = 16 * 1024;

struct RunState {
    session_id: String,
    /// Child process id — kill_command nukes the whole tree via taskkill /T
    /// (a bare Child::kill would orphan cmd /c grandchildren).
    pid: u32,
    killed: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub struct CommandRunner {
    runs: Arc<Mutex<HashMap<String, RunState>>>,
}

impl CommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when a run is live for the session — session_messages uses this
    /// to NOT mark 'streaming' command rows as interrupted on reload.
    pub fn has_active_run(&self, session_id: &str) -> bool {
        self.runs
            .lock()
            .map(|r| r.values().any(|x| x.session_id == session_id))
            .unwrap_or(false)
    }

    /// Spawn `cmd /d /s /c <command>` in `work_dir`, append the command row
    /// and start streaming. Returns the run id.
    pub async fn run(
        &self,
        app: AppHandle,
        stores: Arc<Mutex<StoreRegistry>>,
        session_id: &str,
        command: &str,
        work_dir: &str,
    ) -> Result<String, String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("命令为空".to_string());
        }
        if !std::path::Path::new(work_dir).is_dir() {
            return Err("项目目录不存在，请先绑定项目".to_string());
        }
        {
            let runs = self.runs.lock().map_err(err_msg)?;
            if runs.values().any(|r| r.session_id == session_id) {
                return Err("该会话已有命令在运行，请先取消或等待结束".to_string());
            }
            if runs.len() >= MAX_TOTAL_RUNS {
                return Err(format!("运行中的命令过多（上限 {MAX_TOTAL_RUNS}），请先取消一些"));
            }
        }

        let mut spawn = Command::new("cmd");
        spawn.args(["/d", "/s", "/c"])
            .arg(command)
            .current_dir(work_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            spawn.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = spawn.spawn().map_err(|e| format!("无法启动命令: {e}"))?;
        let stdout = child.stdout.take().ok_or_else(|| "stdout 不可用".to_string())?;
        let stderr = child.stderr.take().ok_or_else(|| "stderr 不可用".to_string())?;

        // Append the command row before registering the run: the row id is
        // what every stream event carries (messageAppended lands first).
        let (append, row) = {
            let mut stores = lock_ok(&stores);
            stores.sessions.ensure_open(session_id);
            let res = stores
                .sessions
                .append_message(session_id, "user", command, "", "streaming", &[], "command")
                .map_err(err_msg);
            let row = match &res {
                Ok(true) => stores
                    .sessions
                    .messages(session_id)
                    .and_then(|m| m.last())
                    .cloned(),
                _ => None,
            };
            (res, row)
        };
        if let Err(e) = append {
            child.kill().await.ok();
            return Err(e);
        }
        let Some(row) = row else {
            child.kill().await.ok();
            return Err("会话不可用".to_string());
        };
        let row_id = row.id.clone();
        let _ = app.emit("chat://messageAppended", row_json(session_id, &row));
        let _ = app.emit("store://sessions", json!({}));

        let run_id = uuid::Uuid::new_v4().hyphenated().to_string();
        let killed = Arc::new(AtomicBool::new(false));
        let pid = child.id().ok_or_else(|| "子进程无 pid".to_string())?;
        {
            let mut runs = self.runs.lock().map_err(err_msg)?;
            runs.insert(
                run_id.clone(),
                RunState {
                    session_id: session_id.to_string(),
                    pid,
                    killed: killed.clone(),
                },
            );
        }

        let runs = self.runs.clone();
        let session_owned = session_id.to_string();
        let run_owned = run_id.clone();
        tokio::spawn(async move {
            stream_until_exit(
                app,
                stores,
                runs,
                session_owned,
                run_owned,
                row_id,
                child,
                stdout,
                stderr,
                killed,
            )
            .await;
        });
        Ok(run_id)
    }

    /// Kill a run: taskkill /T on the tree (cmd /c grandchildren outlive a
    /// bare Child::kill). The stream task observes the exit and finalizes
    /// the row as "interrupted".
    pub async fn kill(&self, run_id: &str) -> Result<(), String> {
        let (pid, killed) = {
            let runs = self.runs.lock().map_err(err_msg)?;
            let Some(rs) = runs.get(run_id) else {
                return Err("命令已结束".to_string());
            };
            (rs.pid, rs.killed.clone())
        };
        killed.store(true, Ordering::SeqCst);
        let mut tk = Command::new("taskkill");
        tk.args(["/F", "/T", "/PID"])
            .arg(pid.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            tk.creation_flags(0x08000000);
        }
        if let Ok(mut c) = tk.spawn() {
            let _ = c.wait().await;
        }
        Ok(())
    }
}

fn err_msg(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Drain stdout+stderr into a merge buffer, flush every 50ms (head-capped at
/// CMD_OUTPUT_MAX forwarded bytes), then finalize the row at exit. The pipes
/// are ALWAYS drained (even past the cap) so a full pipe can never deadlock
/// the child.
async fn stream_until_exit(
    app: AppHandle,
    stores: Arc<Mutex<StoreRegistry>>,
    runs: Arc<Mutex<HashMap<String, RunState>>>,
    session_id: String,
    run_id: String,
    row_id: String,
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    killed: Arc<AtomicBool>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(512);
    let tx_out = tx.clone();
    let tx_err = tx.clone();
    // recv() returns None only when EVERY sender is dropped — the original
    // `tx` must go too, or the loop can never observe both pipes' EOF and
    // the row stays "streaming" forever.
    drop(tx);
    let out_reader = tokio::spawn(async move { read_loop(&tx_out, stdout).await });
    let err_reader = tokio::spawn(async move { read_loop(&tx_err, stderr).await });

    let mut pending = String::new();
    let mut emitted = 0usize;
    let mut truncated = false;
    let mut ticker = tokio::time::interval(Duration::from_millis(FLUSH_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Exit when both pipes hit EOF (rx None) OR the child itself is gone —
    // a daemonized grandchild may keep a pipe open past the child's exit.
    loop {
        tokio::select! {
            chunk = rx.recv() => match chunk {
                Some(t) => pending.push_str(&t),
                None => break,
            },
            _ = ticker.tick() => {}
        }
        flush_pending(
            &app,
            &stores,
            &session_id,
            &run_id,
            &row_id,
            &mut pending,
            &mut emitted,
            &mut truncated,
        );
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
    }
    // Bounded drain of whatever the readers were mid-sending, then flush.
    while let Ok(Some(t)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        pending.push_str(&t);
    }
    flush_pending(
        &app,
        &stores,
        &session_id,
        &run_id,
        &row_id,
        &mut pending,
        &mut emitted,
        &mut truncated,
    );
    // Readers exit on pipe EOF; a detached grandchild can keep a pipe open
    // forever — never wait on them unbounded.
    let _ = tokio::time::timeout(Duration::from_secs(1), out_reader).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), err_reader).await;

    let code = child.wait().await.ok().and_then(|s| s.code());
    let is_killed = killed.load(Ordering::SeqCst);

    let status = if is_killed {
        "interrupted"
    } else if code == Some(0) {
        "done"
    } else {
        "error"
    };
    let exit_code = if is_killed { None } else { code };
    let row = {
        let mut s = lock_ok(&stores);
        s.sessions.ensure_open(&session_id);
        if let Err(e) = s
            .sessions
            .finalize_command_row(&session_id, &row_id, status, exit_code, truncated)
        {
            log::warn!("cmd[{run_id}] finalize row failed: {e}");
        }
        s.sessions
            .messages(&session_id)
            .and_then(|m| m.iter().find(|r| r.id == row_id))
            .cloned()
    };
    if let Some(row) = row {
        // Hand the executed command + result to the agent: queue a context
        // block that the runtime prepends to the NEXT user prompt (drained
        // once, so it is never repeated).
        let block = cmd_context_block(&row, status, exit_code);
        if !block.is_empty() {
            let mut s = lock_ok(&stores);
            s.sessions.ensure_open(&session_id);
            s.sessions.push_cmd_context(&session_id, block);
        }
        let _ = app.emit("chat://bubbleSet", row_json(&session_id, &row));
    }
    let _ = app.emit("store://sessions", json!({}));
    if let Ok(mut runs) = runs.lock() {
        runs.remove(&run_id);
    }
    let _ = app.emit(
        "term://exit",
        json!({
            "sessionId": session_id,
            "runId": run_id,
            "rowId": row_id,
            "code": code,
            "killed": is_killed,
            "truncated": truncated,
        }),
    );
}

/// Push a pipe reader's bytes into the merge channel as lossy UTF-8 chunks.
async fn read_loop(
    tx: &tokio::sync::mpsc::Sender<String>,
    mut stream: impl tokio::io::AsyncRead + Unpin,
) {
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                if tx.send(text).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Flush the merge buffer as ONE term://output event, capped at the head
/// budget; overflow beyond it is dropped and flagged (R4). The flushed text
/// is ALSO appended to the backend row's in-memory segments
/// (append_command_output): the row the frontend holds must never be the
/// only copy — at finalize the bubbleSet row (read back from the store)
/// REPLACES the frontend row, so a row without segments would erase the
/// output the user already saw.
fn flush_pending(
    app: &AppHandle,
    stores: &Arc<Mutex<StoreRegistry>>,
    session_id: &str,
    run_id: &str,
    row_id: &str,
    pending: &mut String,
    emitted: &mut usize,
    truncated: &mut bool,
) {
    if pending.is_empty() {
        return;
    }
    let room = CMD_OUTPUT_MAX.saturating_sub(*emitted);
    let (take, rest) = if pending.len() > room {
        let mut cut = room;
        while !pending.is_char_boundary(cut) {
            cut -= 1;
        }
        *truncated = true;
        (pending[..cut].to_string(), pending[cut..].to_string())
    } else {
        (std::mem::take(pending), String::new())
    };
    if take.is_empty() {
        pending.clear();
        return;
    }
    *emitted += take.len();
    *pending = rest;
    {
        let mut s = lock_ok(stores);
        s.sessions.ensure_open(session_id);
        s.sessions.append_command_output(session_id, row_id, &take);
    }
    let _ = app.emit(
        "term://output",
        json!({
            "sessionId": session_id,
            "runId": run_id,
            "rowId": row_id,
            "text": take,
        }),
    );
}

/// Build the "[Wardex 终端]" block queued for the next user prompt: the
/// command, its outcome and the output TAIL (capped at CMD_CTX_MAX — the
/// chat row itself keeps the full head-capped 64KB).
fn cmd_context_block(row: &MessageRow, status: &str, exit_code: Option<i32>) -> String {
    let outcome = if status == "interrupted" {
        "已被用户中断".to_string()
    } else if let Some(code) = exit_code {
        if code == 0 {
            "成功".to_string()
        } else {
            format!("失败（退出码 {code}）")
        }
    } else {
        "结束".to_string()
    };
    let full: String = row
        .segments
        .iter()
        .filter_map(|s| s.get("text"))
        .filter_map(Value::as_str)
        .collect();
    let mut out = full;
    if out.len() > CMD_CTX_MAX {
        let mut cut = out.len() - CMD_CTX_MAX;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = format!("…（输出过长，仅保留末尾 {CMD_CTX_MAX} 字节）\n{}", &out[cut..]);
    }
    format!(
        "[Wardex 终端] 用户执行了命令：\n$ {}\n执行{outcome}，输出如下：\n```text\n{}\n```",
        row.content, out
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[test]
    fn cmd_context_block_formats_outcome_and_command() {
        let mut row = MessageRow::default();
        row.content = "git status".to_string();
        row.segments.push(Value::Object(Map::from_iter([
            ("kind".to_string(), Value::String("text".to_string())),
            ("text".to_string(), Value::String("M a.txt\n".to_string())),
        ])));

        let done = cmd_context_block(&row, "done", Some(0));
        assert!(done.contains("[Wardex 终端] 用户执行了命令："));
        assert!(done.contains("$ git status"));
        assert!(done.contains("执行成功"));
        assert!(done.contains("M a.txt"));
        assert!(done.contains("```text"));

        let killed = cmd_context_block(&row, "interrupted", None);
        assert!(killed.contains("已被用户中断"));
        assert!(!killed.contains("退出码"));

        let failed = cmd_context_block(&row, "error", Some(2));
        assert!(failed.contains("失败（退出码 2）"));
    }

    #[test]
    fn cmd_context_block_caps_output_tail() {
        let mut row = MessageRow::default();
        row.content = "cat".to_string();
        // head marker + filler over the cap + tail marker: the head must be
        // dropped, the tail must survive (and the cap note added).
        let long = format!("HEAD_MARKER{}TAIL_MARKER", "x".repeat(CMD_CTX_MAX + 50));
        row.segments.push(Value::Object(Map::from_iter([
            ("kind".to_string(), Value::String("text".to_string())),
            ("text".to_string(), Value::String(long)),
        ])));
        let block = cmd_context_block(&row, "done", Some(0));
        assert!(block.contains("仅保留末尾"));
        assert!(block.contains("TAIL_MARKER"));
        assert!(!block.contains("HEAD_MARKER"));
    }

    #[test]
    fn flush_caps_at_head_and_keeps_rest_for_next_flush() {
        let mut pending = "abcdef".to_string();
        let mut emitted = 0usize;
        let mut truncated = false;
        // room = 3 → take "abc", keep "def"
        flush_pending_into(&mut pending, &mut emitted, &mut truncated, 3);
        assert_eq!(pending, "def");
        assert_eq!(emitted, 3);
        assert!(truncated);
    }

    #[test]
    fn flush_keeps_whole_when_under_budget() {
        let mut pending = "abc".to_string();
        let mut emitted = 0usize;
        let mut truncated = false;
        flush_pending_into(&mut pending, &mut emitted, &mut truncated, CMD_OUTPUT_MAX);
        assert!(pending.is_empty());
        assert_eq!(emitted, 3);
        assert!(!truncated);
    }

    #[test]
    fn flush_drops_everything_when_budget_exhausted() {
        let mut pending = "abc".to_string();
        let mut emitted = CMD_OUTPUT_MAX;
        let mut truncated = false;
        flush_pending_into(&mut pending, &mut emitted, &mut truncated, CMD_OUTPUT_MAX);
        assert!(pending.is_empty());
        assert!(truncated);
    }

    #[test]
    fn flush_cut_never_splits_multibyte_chars() {
        let mut pending = "中文".to_string(); // 中=E4B8AD(0-2) 文=E69687(3-5)
        let mut emitted = 0usize;
        let mut truncated = false;
        flush_pending_into(&mut pending, &mut emitted, &mut truncated, 4); // cut at 4 → walk back to 3
        assert_eq!(pending, "文");
        assert_eq!(emitted, 3);
        assert!(truncated);
    }

    /// Test-only variant without the AppHandle emit.
    fn flush_pending_into(
        pending: &mut String,
        emitted: &mut usize,
        truncated: &mut bool,
        budget: usize,
    ) {
        if pending.is_empty() {
            return;
        }
        let room = budget.saturating_sub(*emitted);
        let (take, rest) = if pending.len() > room {
            let mut cut = room;
            while !pending.is_char_boundary(cut) {
                cut -= 1;
            }
            *truncated = true;
            (pending[..cut].to_string(), pending[cut..].to_string())
        } else {
            (std::mem::take(pending), String::new())
        };
        if take.is_empty() {
            pending.clear();
            return;
        }
        *emitted += take.len();
        *pending = rest;
    }
}
