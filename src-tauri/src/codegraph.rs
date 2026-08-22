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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::Manager;
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
    /// Tail of the last build output (shown in the overlay's building/error
    /// cards so the user can see what codegraph is doing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl BuildStatus {
    fn idle() -> Self {
        Self { state: BuildState::Idle, error: None, output: None }
    }
    fn building() -> Self {
        Self { state: BuildState::Building, error: None, output: None }
    }
    fn done_with(output: &str) -> Self {
        Self {
            state: BuildState::Done,
            error: None,
            output: Some(tail(output, 1600)),
        }
    }
    fn error(e: String) -> Self {
        Self { state: BuildState::Error, error: Some(e), output: None }
    }
}

/// Keep the tail of a CLI output blob (the head is usually banner noise).
fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.trim().to_string()
    } else {
        s.chars().skip(n - max).collect::<String>().trim_start().to_string()
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

    /// Build the MCP server command to register for an ACP session
    /// (`node <cli.js> mcp -d <db>`). Returns None when codegraph isn't
    /// installed. The db path is pinned via `-d` (not cwd-dependent); a
    /// missing index is fine — tool calls then return a clear
    /// "run codegraph build" error the agent can recover from in-session.
    pub fn mcp_command(project_dir: &str) -> Option<(String, Vec<String>)> {
        let shim = probe::which_on_expanded_path("codegraph")?;
        let dir = shim.parent()?;
        let db = std::path::Path::new(project_dir)
            .join(".codegraph")
            .join("graph.db")
            .to_string_lossy()
            .into_owned();
        // Prefer launching node directly (node.exe spawns cleanly; the .cmd
        // shim does not). cli.js lives under the npm global bin's
        // node_modules/@optave/codegraph.
        let cli = dir
            .join("node_modules")
            .join("@optave")
            .join("codegraph")
            .join("dist")
            .join("cli.js");
        if cli.is_file() {
            return Some((
                "node".to_string(),
                vec![cli.to_string_lossy().into_owned(), "mcp".to_string(), "-d".to_string(), db],
            ));
        }
        // Fallback: run the shim through cmd (relies on PATH + shim).
        Some((
            "cmd".to_string(),
            vec!["/c".to_string(), "codegraph".to_string(), "mcp".to_string(), "-d".to_string(), db],
        ))
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
            log::info!("[codegraph] build start: {project_dir}");
            let status = match run_cli_capture(&path, &["build", project_dir.as_str()]).await {
                Ok(out) => {
                    log::info!(
                        "[codegraph] build ok: {project_dir} ({})",
                        out.combined().lines().count()
                    );
                    BuildStatus::done_with(&out.combined())
                }
                Err(e) => {
                    log::warn!("[codegraph] build failed {project_dir}: {e}");
                    BuildStatus::error(e)
                }
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
        let db_str = db.to_string_lossy();
        let stdout = run_cli_capture(
            &path,
            &["query", q, "--kind", "interface", "--json", "-n", "60", "-d", db_str.as_ref()],
        )
        .await
        .map_err(|e| format!("codegraph 查询失败：{e}"))?
        .stdout;
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

    /// Render the dependency graph: `codegraph plot -d <db> -o <html> --no-open`
    /// writes an HTML file, then opens it in an in-app Tauri window (asset
    /// protocol) instead of the external browser. Reuses the same window on
    /// repeat views.
    pub fn plot(&self, app: &tauri::AppHandle, project_dir: &str) -> Result<(), String> {
        let path = self.resolve().ok_or_else(|| "codegraph 未安装".to_string())?;
        if !Self::index_exists(project_dir) {
            return Err("尚未构建索引".to_string());
        }
        let db = std::path::Path::new(project_dir).join(".codegraph").join("graph.db");
        let db_str = db.to_string_lossy().into_owned();
        let html = std::env::temp_dir().join("wardex-codegraph-graph.html");
        let html_str = html.to_string_lossy().into_owned();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match run_cli_capture(
                &path,
                &["plot", "-d", db_str.as_str(), "-o", html_str.as_str(), "--no-open"],
            )
            .await
            {
                Ok(_) => open_plot_window(&app, &html),
                Err(e) => log::warn!("[codegraph] plot failed: {e}"),
            }
        });
        Ok(())
    }
}

/// Open `codegraph-graph` in an in-app Tauri window via the asset protocol.
/// On Windows the asset URL is `http://asset.localhost/<path>` (what the JS
/// `convertFileSrc` produces), with the absolute path fully percent-encoded;
/// the asset handler percent-decodes it back and serves the file. Creates the
/// window on first use; reuses + navigates it afterwards.
fn open_plot_window(app: &tauri::AppHandle, html: &std::path::Path) {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let abs = html.canonicalize().unwrap_or_else(|_| html.to_path_buf());
    let encoded = utf8_percent_encode(&abs.to_string_lossy(), NON_ALPHANUMERIC).to_string();
    let url = tauri::Url::parse(&format!("http://asset.localhost/{encoded}"))
        .expect("valid asset url");
    if let Some(win) = app.get_webview_window("codegraph-graph") {
        let _ = win.navigate(url);
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    if let Err(e) = tauri::WebviewWindowBuilder::new(
        app,
        "codegraph-graph",
        tauri::WebviewUrl::External(url),
    )
    .title("Codegraph 图谱")
    .inner_size(1000.0, 720.0)
    .build()
    {
        log::warn!("[codegraph] open window failed: {e}");
    }
}

/// Captured CLI output; the query path parses `stdout` only (JSON), the
/// build path shows `combined()` so progress lines land in the overlay.
struct CaptureOut {    stdout: String,
    stderr: String,
}

impl CaptureOut {
    fn combined(&self) -> String {
        if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else if self.stdout.trim().is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout.trim_end(), self.stderr.trim())
        }
    }
}

/// Run a codegraph CLI command by wrapping its (possibly `.cmd`/`.bat`) shim
/// in `cmd.exe`. The shim path and each argument are passed as SEPARATE argv
/// entries so CreateProcess quotes them — the same pattern the ACP transport
/// uses (acp/transport.rs). Concatenating them into a single `/c` string makes
/// cmd.exe re-parse the quotes and mangle paths (e.g. a quoted dir becomes
/// garbage and codegraph's mkdir fails), which is why builds used to fail.
/// 600s ceiling so a hung build can never wedge the app.
async fn run_cli_capture(program: &Path, args: &[&str]) -> Result<CaptureOut, String> {
    let mut c = tokio::process::Command::new("cmd.exe");
    c.args(["/d", "/s", "/c"])
        .arg(program)
        .args(args)
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
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() {
        Ok(CaptureOut { stdout, stderr })
    } else {
        let err = stderr.trim();
        if !err.is_empty() {
            Err(err.to_string())
        } else {
            let e = stdout.trim();
            if !e.is_empty() {
                Err(e.to_string())
            } else {
                Err("codegraph 命令执行失败".to_string())
            }
        }
    }
}
