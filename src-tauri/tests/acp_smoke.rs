//! Real-CLI protocol smoke test against the locally installed `kimi` binary.
//!
//! Ignored by default (requires the CLI and spawns real processes); run with:
//!   cargo test --test acp_smoke -- --ignored --nocapture
//!
//! Covers only the handshake half of the protocol (initialize + session/new):
//! no `session/prompt` is sent, so the test never consumes API quota.

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
    let spec = provider::spec("kimi").expect("kimi in REGISTRY");
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_string_lossy().to_string();

    let config = SpawnConfig {
        cli_path: cli,
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

    // Drive the read loop until the session is ready (session/new answered)
    // or a failure event surfaces. Handshake should take well under 30s.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut ready = false;
    while tokio::time::Instant::now() < deadline && !ready {
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.recv_once()).await {
            Ok(Ok(false)) => panic!("EOF before session ready"),
            Ok(Ok(true)) => {}
            Ok(Err(e)) => panic!("recv error: {e}"),
            Err(_) => panic!("timeout waiting for session ready"),
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

    assert!(ready, "session did not become ready in time");
    let session_id = client.session_id().to_string();
    assert!(!session_id.is_empty(), "session/new must return a sessionId");
    eprintln!("OK: kimi acp handshake, sessionId = {session_id}");
}
