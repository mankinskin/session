use std::path::PathBuf;

use session::mcp::run_mcp_server;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("session_mcp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let store_root = resolve_store_root();
    let workspace_slug = resolve_workspace_slug();

    eprintln!(
        "session-mcp starting (store: {}, workspace: {workspace_slug})",
        store_root.display()
    );

    if let Err(err) = run_mcp_server(store_root, workspace_slug).await {
        eprintln!("Fatal error: {err}");
        std::process::exit(1);
    }
}

fn resolve_store_root() -> PathBuf {
    if let Ok(path) = std::env::var("SESSION_STORE_ROOT") {
        return PathBuf::from(path);
    }
    memory_kernel::workspace::resolve_requested_store_root(
        None, None, None, ".session",
    )
}

fn resolve_workspace_slug() -> String {
    std::env::var("SESSION_WORKSPACE_SLUG")
        .unwrap_or_else(|_| "default".to_string())
}
