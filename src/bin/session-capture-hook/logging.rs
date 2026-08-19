use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter,
    fmt,
    layer::SubscriberExt as _,
    util::SubscriberInitExt as _,
};

/// Initialize file-only tracing so hook stdout/stderr stay reserved for the
/// Copilot hook contract (`{}` payload on stdout, human diagnostics on
/// stderr). Returns a guard that must be held for the process lifetime or
/// buffered log lines are dropped on exit.
///
/// Defaults to the OS temp directory (not the repository checkout) so the
/// hook never mutates a session store's `.session` tree as a side effect of
/// logging; override with `SESSION_HOOK_LOG_DIR`.
pub(super) fn init_file_logging() -> WorkerGuard {
    let log_dir = std::env::var("SESSION_HOOK_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("session-capture-hook"));
    let _ = std::fs::create_dir_all(&log_dir);

    let appender =
        tracing_appender::rolling::never(&log_dir, "session-capture-hook.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("SESSION_HOOK_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false);

    // Ignore double-init (e.g. under repeated test harness invocations).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .try_init();

    guard
}
