//! Process sandbox — transform-style core + per-platform backends.
//!
//! Callers hand `prepare_command` a policy plus the raw `(program, args, cwd)`
//! and get back a spawnable [`tokio::process::Command`] that is confined
//! according to the platform backend:
//! - **Linux** — Landlock ruleset installed via `pre_exec` (real enforcement).
//! - **macOS** — argv wrapped with `sandbox-exec` + generated SBPL profile.
//! - **Windows** — no isolation (fail-open, warns).
//! - **other** — no-op passthrough.
//!
//! Sandboxing can be globally disabled at runtime with [`set_enabled`]; the
//! policy derivation ([`SandboxPolicy::for_mode`]) honors that switch.
//!
//! Top-level entry points:
//! - [`prepare_command`] — turn a policy + invocation into a [`PreparedCommand`].
//! - [`SandboxPolicy`] / [`SandboxPolicy::for_mode`] — the policy model.
//! - [`set_enabled`] / [`is_enabled`] — the global on/off switch.

use std::sync::atomic::{AtomicBool, Ordering};

pub mod backend;
pub mod config;
pub mod denial;
pub mod error;
pub mod policy;
pub mod traits;

pub use backend::{
    LandlockSandbox, NoopSandbox, SeatbeltSandbox, WindowsSandbox, create_sandbox, prepare_command,
};
pub use config::SandboxMode;
pub use denial::{SANDBOX_DENIAL_HINT, is_likely_sandbox_denied};
pub use error::SandboxError;
pub use policy::{DerivedSandbox, PathAccess, PathRule, SandboxPolicy, WritableRoot};
pub use traits::{PreparedCommand, Sandbox, SandboxBackend};

/// Global sandbox switch. Defaults to enabled; the CLI/config flips it off when
/// the user passes `--sandbox off`.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable command sandboxing process-wide.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Apply a [`SandboxMode`] to the global switch.
pub fn set_mode(mode: SandboxMode) {
    set_enabled(mode.enabled());
}

/// Whether command sandboxing is currently enabled process-wide.
#[must_use]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_toggle_roundtrips() {
        // Default is enabled.
        assert!(is_enabled());
        set_enabled(false);
        assert!(!is_enabled());
        set_mode(SandboxMode::Auto);
        assert!(is_enabled());
        // Restore default for other tests in this binary.
        set_enabled(true);
    }
}
