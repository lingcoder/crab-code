//! [`SandboxError`] — typed failures from sandbox preparation.

use std::fmt;

/// Errors raised while preparing a sandboxed command.
#[derive(Debug)]
pub enum SandboxError {
    /// The policy required isolation but the platform backend is unavailable
    /// and its contract is fail-closed (Linux/macOS).
    Unavailable(String),
    /// The kernel/OS accepted the request but did not actually enforce it.
    NotEnforced(String),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(msg) => write!(f, "sandbox unavailable: {msg}"),
            Self::NotEnforced(msg) => write!(f, "sandbox not enforced: {msg}"),
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<SandboxError> for crab_core::Error {
    fn from(err: SandboxError) -> Self {
        Self::Other(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_conversion() {
        let err = SandboxError::Unavailable("no sandbox-exec".into());
        assert!(err.to_string().contains("unavailable"));
        let core: crab_core::Error = err.into();
        assert!(core.to_string().contains("no sandbox-exec"));
    }
}
