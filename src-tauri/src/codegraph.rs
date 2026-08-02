// Codegraph (https://github.com/optave/ops-codegraph-tool) integration for
// the Ctrl+\ interface lookup (V2). The CLI is an external npm tool
// (`npm install -g @optave/codegraph`, needs Node >= 22.6); Wardex shells
// out to it via cmd.exe because the npm global bin ships .cmd shims that
// cannot be CreateProcess'd directly. Installation is probed ONCE and cached
// in the user prefs; the resolved path + per-project build status live here.
//
// Contract (verified against codegraph 3.16.0):
//   codegraph build <dir>      -> writes <dir>/.codegraph/graph.db (incremental)
//   codegraph query <name> -k interface -j -n 60 -d <db>
//                              -> {"name":..., "results":[{name,kind,file,line,endLine,role}]}
//   codegraph plot -d <db>     -> writes a temp HTML + opens the browser, exits

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::probe;

const CMD_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildState {
    Idle,
    Building,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildStatus {
    pub state: BuildState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BuildStatus {
    fn idle() -> Self {
        Self { state: BuildState::Idle, error: None }
    }
    fn building() -> Self {
        Self { state: BuildState::Building, error: None }
    }
    fn done() -> Self {
        Self { state: BuildState::Done, error: None }
    }
    fn error(e: String) -> Self {
        Self { state: BuildState::Error, error: Some(e) }
    }
}

#[derive(Default)]
struct Inner {
    /// None = not probed this session; Some(None) = not found.
    path: Mutex<Option<Option<PathBuf>>>,
    /// project_dir -> build status.
    builds: Mutex<HashMap<String, BuildStatus>>,
}

#[derive(Clone, Default)]
pub struct CodegraphRunner {
    inner: Arc<Inner>,
}

impl CodegraphRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the CLI once per session (cached; `invalidate` re-probes after
    /// the user installs it).
    pub fn resolve(&self) -> Option<PathBuf> {
        let mut cached = self.inner.path.lock().unwrap();
        if let Some(found) = &*cached {
            return found.clone();
        }
        let found = probe::which_on_expanded_path("codegraph");
        *cached = Some(found.clone());
        found
    }

    pub fn invalidate(&self) {
        *self.inner.path.lock().unwrap() = None;
    }

    pub fn build_status(&self, project_dir: &str) -> BuildStatus {
        self.inner
            .builds
            .lock()
            .unwrap()
            .get(project_dir)
            .cloned()
            .unwrap_or_else(BuildStatus::idle)
    }

    pub fn index_exists(project_dir: &str) -> bool {
        std::path::Path::new(project_dir)
            .join(".codegraph")
            .join("graph.db")
            .is_file()
    }

    /// Status payload for the Ctrl+\ overlay (installed flag is set by the
    /// command layer from the prefs-cached probe).
    pub fn status(&self, project_dir: &str) -> serde_json::Value {
        json!({
            "path": self.resolve().map(|p| p.to_string_lossy().into_owned()),
            "build": self.build_status(project_dir),
            "indexExists": Self::index_exists(project_dir),
        })
    }

    fn set_build(&self, project_dir: &str, s: BuildStatus) {
        self.inner.builds.lock().unwrap().insert(project_dir.to_string(), s);
    }

    /// Start `codegraph build <dir>` in the background (spawned task, survives
    /// the command return). Emits codegraph://build on completion so a focused
    /// overlay can react without polling.
    pub fn start_build(&self, app: AppHandle, project_dir: String) {
        {
            let mut builds = self.inner.builds.lock().unwrap();
            if builds.get(&project_dir).map(|s| s.state) == Some(BuildState::Building) {
                return; // already building
            }
            builds.insert(project_dir.clone(), BuildStatus::building());
        }
        let Some(path) = self.resolve() else {
            self.set_build(&project_dir, BuildStatus::error("codegraph 未安装".to_string()));
            return;
        };
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let args = format!("{} build \"{}\"", path.to_string_lossy(), project_dir);
            let status = match run_cli_capture(&args).await {
                Ok(_) => BuildStatus::done(),
                Err(e) => BuildStatus::error(e),
            };
            let ok = matches!(status.state, BuildState::Done);
            this.set_build(&project_dir, status);
            let _ = app.emit(
                "codegraph://build",
                json!({ "projectDir": project_dir, "ok": ok }),
            );
        });
    }

    /// Query interfaces by name (partial, case-insensitive). The empty query
    /// is rejected by the CLI, so it maps to a dedicated error the frontend
    /// turns into a hint.
    pub async fn query_interfaces(
        &self,
        project_dir: &str,
        query: &str,
    ) -> Result<Vec<crate::store::workspace::InterfaceHit>, String> {
        let path = self.resolve().ok_or_else(|| "codegraph 未安装".to_string())?;
        if !Self::index_exists(project_dir) {
            return Err("尚未构建索引".to_string());
        }
        let q = query.trim();
        if q.is_empty() {
            return Err("empty".to_string());
        }
        let db = std::path::Path::new(project_dir).join(".codegraph").join("graph.db");
        let args = format!(
            "{} query {} --kind interface --json -n 60 -d \"{}\"",
            path.to_string_lossy(),
            quote_arg(q),
            db.to_string_lossy(),
        );
        let stdout = run_cli_capture(&args).await.map_err(|e| format!("codegraph 查询失败：{e}"))?;
        #[derive(serde::Deserialize)]
        struct Raw {
            results: Vec<RawHit>,
        }
        #[derive(serde::Deserialize)]
        struct RawHit {
            name: String,
            file: String,
            line: i64,
        }
        let raw: Raw = serde_json::from_str(&stdout).map_err(|e| format!("codegraph 返回无法解析：{e}"))?;
        Ok(raw
            .results
            .into_iter()
            .map(|h| crate::store::workspace::InterfaceHit {
                file: h.file,
                line: h.line,
                name: h.name,
                text: String::new(),
            })
            .collect())
    }

    /// Fire-and-forget `codegraph plot -d <db>`: it writes a temp HTML,
    /// opens the default browser and exits on its own.
    pub fn plot(&self, project_dir: &str) -> Result<(), String> {
        let path = self.resolve().ok_or_else(|| "codegraph 未安装".to_string())?;
        if !Self::index_exists(project_dir) {
            return Err("尚未构建索引".to_string());
        }
        let db = std::path::Path::new(project_dir).join(".codegraph").join("graph.db");
        let args = format!(
            "{} plot -d \"{}\"",
            path.to_string_lossy(),
            db.to_string_lossy()
        );
        tauri::async_runtime::spawn(async move {
            let mut c = tokio::process::Command::new("cmd.exe");
            c.args(["/d", "/s", "/c"])
                .arg(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                c.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            if let Ok(mut ch) = c.spawn() {
                let _ = ch.wait().await;
            }
        });
        Ok(())
    }
}

/// Wrap a shell argument in double quotes, escaping embedded quotes.
fn quote_arg(s: &str) -> String {
    let escaped = s.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Run a cmd.exe-wrapped codegraph command; returns stdout on success,
/// stderr (fallback stdout) on failure. 600s ceiling so a hung build can
/// never wedge the app.
async fn run_cli_capture(args: &str) -> Result<String, String> {
    let mut c = tokio::process::Command::new("cmd.exe");
    c.args(["/d", "/s", "/c"])
        .arg(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        c.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let child = c.spawn().map_err(|e| format!("无法启动 codegraph：{e}"))?;
    let out = match tokio::time::timeout(CMD_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("codegraph 执行失败：{e}")),
        Err(_) => return Err("codegraph 执行超时（10 分钟）".to_string()),
    };
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let err = stderr.trim();
        if !err.is_empty() {
            Err(err.to_string())
        } else {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let e = stdout.trim();
            if !e.is_empty() {
                Err(e.to_string())
            } else {
                Err("codegraph 命令执行失败".to_string())
            }
        }
    }
}
