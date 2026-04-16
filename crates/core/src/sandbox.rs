//! Sandbox backend and violation types shared with TUI and `core::Event`.
//!
//! The `Sandbox` trait and `SandboxPolicy` live in `crab-sandbox`; this
//! module only carries the enum label used in events and the minimal
//! violation payload for display.

use serde::{Deserialize, Serialize};

/// Which sandbox backend produced a given event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxBackend {
    Noop,
    Seatbelt,
    Landlock,
    Wsl,
}

/// Minimal violation payload suitable for UI display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViolationInfo {
    /// Operation that was denied, e.g. "write", "exec", "connect".
    pub op: String,
    /// Target path / host / resource the operation was attempted against.
    pub target: String,
    /// Human-readable reason the backend rejected the operation.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_serde_roundtrip() {
        let b = SandboxBackend::Landlock;
        let json = serde_json::to_string(&b).unwrap();
        let back: SandboxBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn violation_serde_roundtrip() {
        let v = ViolationInfo {
            op: "write".into(),
            target: "/etc/passwd".into(),
            reason: "path outside workdir allowlist".into(),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: ViolationInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}
