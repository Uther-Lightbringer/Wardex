//! Pi plugin (embedded agent) smoke test against the real compiled pi binary.
//!
//! Ignored by default (requires the compiled pi binary — see bundle-pi.mjs);
//! run with:
//!   cargo test --test pi_smoke -- --ignored --nocapture
//!
//! Sends get_state only — no prompt, so no API quota is consumed.

use tokio::sync::mpsc;
use wardex_lib::acp::AcpEvent;
use wardex_lib::chat::driver::{ClientDriver, PiLaunch};
use wardex_lib::chat::pi;

#[tokio::test]
#[ignore = "requires node + built pi plugin"]
async fn pi_rpc_state_smoke() {
    // Node presence: skip quietly when unavailable (CI machines).
    let plugin = match pi::locate_plugin_dir("") {
        Ok(dist) if dist.join(pi::pi_binary_name()).is_file() => dist,
        Ok(_) => {
            eprintln!("pi binary missing, skipping");
            return;
        }
        Err(e) => {
            eprintln!("pi plugin not found ({e}), skipping");
            return;
        }
    };

    let tmp = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::channel::<AcpEvent>(64);
    let mut driver = pi::PiDriver::spawn(
        PiLaunch {
            binary: plugin.join(pi::pi_binary_name()),
            provider_key: String::new(),
            model: String::new(),
            session_dir: String::new(),
            session_id: String::new(),
            effort_options: Vec::new(),
            default_effort: String::new(),
            env: Vec::new(),
            cwd: tmp.path().to_string_lossy().into_owned(),
            extensions: Vec::new(),
        },
        tx,
    )
    .await
    .expect("pi spawn must succeed");

    // Started arrives immediately (no handshake).
    let started = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("Started timeout")
        .expect("channel closed");
    assert!(matches!(started, AcpEvent::Started { .. }), "expected Started, got {started:?}");

    // Ask for state; the RPC response flows through as an ignored event, but
    // the process must stay alive and healthy (no ProcessExited).
    // NOTE: wait a moment for pi's stdin listener to attach — the command
    // must not race the startup sequence.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    driver
        .send_command_for_test("{\"id\":\"t1\",\"type\":\"get_state\"}")
        .await
        .expect("send get_state");
    // get_state produces exactly one response line; a single successful
    // recv_once proves the process is alive and the protocol is responsive.
    let ok = match tokio::time::timeout(std::time::Duration::from_secs(15), driver.recv_once())
        .await
    {
        Ok(r) => r.expect("recv error"),
        Err(_) => {
            eprintln!("recv timeout; stderr tail: {:?}", driver.stderr_tail());
            panic!("recv timeout");
        }
    };
    assert!(ok, "pi process exited during state smoke");
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AcpEvent::ProcessExited { code } => {
                panic!("pi process exited unexpectedly (code {code})");
            }
            _ => {}
        }
    }
    eprintln!("OK: pi rpc-entry alive, protocol responsive");
}
