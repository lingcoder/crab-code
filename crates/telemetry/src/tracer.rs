use std::path::Path;

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Build the default `EnvFilter`: INFO unless `RUST_LOG` is set.
fn default_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialize the tracing subscriber with stderr output.
///
/// - Default level: `INFO` (overridden by `RUST_LOG`)
/// - Format: compact, with target and timestamps
///
/// Returns `Err` if a global subscriber has already been set.
pub fn init() -> Result<(), tracing_subscriber::util::TryInitError> {
    let fmt_layer = fmt::layer().with_target(true).compact();

    tracing_subscriber::registry()
        .with(default_filter())
        .with(fmt_layer)
        .try_init()
}

/// Initialize the tracing subscriber with both stderr and rolling file output.
///
/// Log files are written to `log_dir` with daily rotation and a `crab-code` prefix.
///
/// Returns `Err` if a global subscriber has already been set.
pub fn init_with_file(log_dir: &Path) -> Result<(), tracing_subscriber::util::TryInitError> {
    let file_appender = tracing_appender::rolling::daily(log_dir, "crab-code.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Keep the guard alive for the process lifetime by leaking it.
    // Without this, the background writer thread would be dropped immediately.
    std::mem::forget(guard);

    let stderr_layer = fmt::layer().with_target(true).compact();

    let file_layer = fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(non_blocking);

    tracing_subscriber::registry()
        .with(default_filter())
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_does_not_panic() {
        // The global subscriber may already be set by another test in this process,
        // so we only assert that init() does not panic — the Err case is acceptable.
        let _ = init();
    }

    #[test]
    fn init_with_file_does_not_panic() {
        let dir = std::env::temp_dir();
        let _ = init_with_file(&dir);
    }

    #[test]
    fn default_filter_respects_env() {
        // Just verify it constructs without panic.
        let _filter = default_filter();
    }
}
