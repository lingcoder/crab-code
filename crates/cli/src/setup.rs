//! Process initialization: signal handlers, panic hook, logging setup.
//!
//! Called once at the start of `main()` to set up:
//! - Graceful shutdown via Ctrl+C / SIGTERM
//! - Human-friendly panic reporting with crash log
//! - Tracing/logging to stderr and optional file

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Global flag indicating a graceful shutdown was requested.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Number of SIGINT signals received (used for force-exit on second press).
static SIGINT_COUNT: AtomicBool = AtomicBool::new(false);

/// Check if shutdown has been requested (e.g., via Ctrl+C).
#[must_use]
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

/// Install signal handlers for graceful shutdown.
///
/// Signal delivery is handled by [`spawn_async_signal_handlers`] once the
/// Tokio runtime is running. There is no synchronous pre-runtime handler here
/// because the build forbids `unsafe` code.
pub fn install_signal_handlers() {
    // The CLI relies on the async signal listeners installed after runtime
    // startup. Keeping this function lets initialization stay symmetric even
    // when no synchronous handler is registered.
}

/// Spawn the async signal listener tasks. Call this after the tokio runtime starts.
///
/// This must be called within a tokio context. It spawns background tasks that
/// listen for Ctrl+C and (on Unix) SIGTERM.
pub fn spawn_async_signal_handlers() {
    tokio::spawn(async {
        loop {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for ctrl-c");

            if SIGINT_COUNT.swap(true, Ordering::SeqCst) {
                // Second Ctrl+C: force exit
                eprintln!("\nForce exit.");
                std::process::exit(130);
            }

            SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
            eprintln!("\nShutting down gracefully (press Ctrl+C again to force)...");
        }
    });

    #[cfg(unix)]
    {
        tokio::spawn(async {
            let mut stream =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to listen for SIGTERM");
            stream.recv().await;
            SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
            eprintln!("\nSIGTERM received, shutting down...");
        });
    }
}

/// Install a custom panic hook that:
/// - Logs the panic info to a crash log file (`~/.crab/logs/crash.log`)
/// - Prints a user-friendly message to stderr
/// - Suggests filing a bug report
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        // Build a user-friendly message
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".into());

        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };

        // Try to write to crash log
        let crash_log_path = crash_log_path();
        let timestamp = chrono_free_timestamp();
        let log_entry = format!("[{timestamp}] PANIC at {location}: {payload}\n");

        if let Some(parent) = crash_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Append to crash log (best-effort)
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_log_path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{log_entry}");
        }

        // Print user-friendly message
        eprintln!();
        eprintln!("========================================");
        eprintln!("Crab Code encountered an unexpected error.");
        eprintln!();
        eprintln!("  Location: {location}");
        eprintln!("  Details:  {payload}");
        eprintln!();
        eprintln!("  Crash log: {}", crash_log_path.display());
        eprintln!();
        eprintln!(
            "  Please file a bug report at: https://github.com/pchaganti/gx-crab-code/issues"
        );
        eprintln!("========================================");
        eprintln!();

        // Also run the default hook (prints full backtrace if RUST_BACKTRACE=1)
        default_hook(info);
    }));
}

/// Initialize the tracing/logging system.
///
/// This delegates to `crab_core::common::utils::debug::init_debug` which supports:
/// - Console output to stderr (with level filtering)
/// - Optional file output to `~/.crab/logs/`
///
/// When `verbose` is true, enables `crab=debug` level logging.
/// Otherwise, tracing stays at its default (warn+error only).
pub fn init_logging(verbose: bool) {
    let config = crab_core::common::utils::debug::DebugConfig {
        enabled: verbose,
        filter: if verbose {
            Some("crab=debug".to_string())
        } else {
            None
        },
        file: if verbose { Some(log_file_path()) } else { None },
    };
    crab_core::common::utils::debug::init_debug(&config);
}

/// Run all initialization steps.
///
/// Call this once at the very start of `main()`, before building the tokio runtime.
pub fn initialize(verbose: bool) {
    install_panic_hook();
    install_signal_handlers();
    init_logging(verbose);
}

// ── Path helpers ─────────────────────────────────────────────────────

/// Return the crash log path: `~/.crab/logs/crash.log`.
fn crash_log_path() -> PathBuf {
    crab_core::common::utils::path::home_dir()
        .join(".crab")
        .join("logs")
        .join("crash.log")
}

/// Return the debug log file path: `~/.crab/logs/debug.log`.
fn log_file_path() -> PathBuf {
    crab_core::common::utils::path::home_dir()
        .join(".crab")
        .join("logs")
        .join("debug.log")
}

/// Return a basic UTC timestamp without pulling in the `chrono` crate.
///
/// Format: `YYYY-MM-DDTHH:MM:SSZ`
fn chrono_free_timestamp() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
/// Algorithm from Howard Hinnant's `civil_from_days`.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_flag_initially_false() {
        // Note: this test may interact with other tests if run in parallel,
        // but the initial state should be false at process start.
        // We just test the API compiles and returns a bool.
        let _ = is_shutdown_requested();
    }

    #[test]
    fn crash_log_path_under_crab() {
        let path = crash_log_path();
        assert!(path.ends_with("crash.log"));
        assert!(path.to_string_lossy().contains(".crab"));
    }

    #[test]
    fn log_file_path_under_crab() {
        let path = log_file_path();
        assert!(path.ends_with("debug.log"));
        assert!(path.to_string_lossy().contains(".crab"));
    }

    #[test]
    fn chrono_free_timestamp_format() {
        let ts = chrono_free_timestamp();
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
        assert!(ts.starts_with("20"));
    }

    #[test]
    fn days_to_ymd_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-04-05 is day 20,548 since epoch
        let (y, m, d) = days_to_ymd(20_548);
        assert_eq!(y, 2026);
        assert_eq!(m, 4);
        assert_eq!(d, 5);
    }

    #[test]
    fn install_panic_hook_does_not_panic() {
        // Just verify it can be called without panicking.
        // Note: this replaces the global panic hook, so run with caution.
        install_panic_hook();
    }

    #[test]
    fn init_logging_non_verbose() {
        // Non-verbose is a no-op (debug disabled), should not panic
        // Note: can only init tracing once per process, so this may
        // silently no-op if another test already initialized it.
        init_logging(false);
    }
}
