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

    eprintln!("session-mcp starting (store: {})", store_root.display());

    if let Err(err) = run_mcp_server(store_root).await {
        eprintln!("Fatal error: {err}");
        std::process::exit(1);
    }
}

fn resolve_store_root() -> PathBuf {
    if let Ok(path) = std::env::var("SESSION_STORE_ROOT") {
        return PathBuf::from(path);
    }
    match memory_kernel::workspace::resolve_consumer_store_root(
        None, None, None, ".session",
    ) {
        Ok(store_root) => store_root,
        Err(error) => {
            eprintln!("Fatal error: {error}");
            std::process::exit(1);
        },
    }
}

