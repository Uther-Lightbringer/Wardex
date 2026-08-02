//! Real-CLI protocol smoke test against the locally installed agent CLIs.
//!
//! Ignored by default (requires the CLIs and spawns real processes); run with:
//!   cargo test --test acp_smoke -- --ignored --nocapture
//!
//! Covers only the handshake half of the protocol (initialize + session/new):
//! no `session/prompt` is sent, so the test never consumes API quota.
//!
//! Note: with node .cmd shims (claude/codex) the test may linger in teardown
//! after printing OK — killing the cmd.exe wrapper leaves the node grandchild
//! holding the stderr pipe. The assertions have passed by then; run tests
//! individually (`-- --ignored --exact <name>`) if the wait bothers you.

use tokio::sync::mpsc;
use wardex_lib::acp::{AcpClient, AcpEvent, SpawnConfig, StartParams};
use wardex_lib::provider;

fn kimi_cli_path() -> Option<String> {
    let home = std::env::var("USERPROFILE").ok()?;
    let p = format!(r"{home}\.kimi-code\bin\kimi.exe");
    std::path::Path::new(&p).exists().then_some(p)
}

#[tokio::test]
#[ignore = "requires local kimi CLI"]
async fn kimi_acp_handshake_smoke() {
    let Some(cli) = kimi_cli_path() else {
        eprintln!("kimi.exe not found, skipping");
        return;
    };
    handshake("kimi", &cli).await;
}

#[tokio::test]
#[ignore = "requires local claude-code-acp CLI"]
async fn claude_acp_handshake_smoke() {
    handshake("claude", "claude-code-acp").await;
}

#[tokio::test]
#[ignore = "requires local codex-acp CLI"]
async fn codex_acp_handshake_smoke() {
    handshake("codex", "codex-acp").await;
}

#[tokio::test]
#[ignore = "requires local opencode CLI"]
async fn opencode_acp_handshake_smoke() {
    handshake("opencode", "opencode").await;
}

/// spawn → initialize → session/new. Asserts the handshake completes without
/// StartFailed and the process speaks ACP (is_initialized). claude/codex are
/// npm .cmd shims, so this also exercises the cmd.exe /c wrapping path.
/// session/new may legitimately fail when the CLI has no credentials; that is
/// reported but not asserted (no prompt is ever sent, zero quota cost).
async fn handshake(provider_id: &str, cli: &str) {
    let spec = provider::spec(provider_id).expect("provider in REGISTRY");
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_string_lossy().to_string();

    let config = SpawnConfig {
        cli_path: cli.to_string(),
        args: spec.acp_args.iter().map(|s| s.to_string()).collect(),
        env: provider::env_overrides(spec, "", "", true),
        cwd: cwd.clone(),
    };
    let start = StartParams {
        cwd,
        preferred_mode: String::new(),
        resume_session_id: String::new(),
        mcp_servers: vec![],
    };

    let (tx, mut rx) = mpsc::channel::<AcpEvent>(64);
    let mut client = AcpClient::spawn(config, start, tx)
        .await
        .expect("spawn + initialize must succeed");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut ready = false;
    while tokio::time::Instant::now() < deadline && !ready {
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.recv_once()).await {
            Ok(Ok(false)) => break, // EOF: CLI exited (e.g. session/new w/o auth)
            Ok(Ok(true)) => {}
            Ok(Err(e)) => panic!("recv error: {e}"),
            Err(_) => break, // idle timeout: handshake done, nothing more coming
        }
        while let Ok(ev) = rx.try_recv() {
            eprintln!("event: {ev:?}");
            match ev {
                AcpEvent::Started { .. } => ready = true,
                AcpEvent::StartFailed { error } => panic!("start failed: {error}"),
                _ => {}
            }
        }
    }

    assert!(client.is_initialized(), "{provider_id}: initialize must complete");
    if ready {
        let session_id = client.session_id().to_string();
        assert!(!session_id.is_empty(), "session/new must return a sessionId");
        eprintln!("OK: {provider_id} handshake + session/new, sessionId = {session_id}");
    } else {
        eprintln!(
            "OK: {provider_id} initialize handshake completed; \
             session/new not established (likely missing credentials), accepted"
        );
    }
}
