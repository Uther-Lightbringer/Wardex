#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // MCP reminder subprocess mode (chat/runtime.rs injects this as a
    // built-in mcpServers entry): serve the stdio protocol and exit, never
    // starting the GUI app.
    if std::env::args().any(|a| a == "--mcp-reminder") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        if let Err(e) = rt.block_on(wardex_lib::mcp_reminder::run()) {
            eprintln!("wardex-reminder: {e}");
            std::process::exit(1);
        }
        return;
    }
    // MCP database schema server (chat/runtime.rs injects this as a built-in
    // mcpServers entry when the session's project has DB connections): serve
    // metadata-only tools and exit, never starting the GUI app.
    if std::env::args().any(|a| a == "--mcp-db") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        if let Err(e) = rt.block_on(wardex_lib::db::mcp::run()) {
            eprintln!("wardex-db: {e}");
            std::process::exit(1);
        }
        return;
    }
    wardex_lib::run()
}
