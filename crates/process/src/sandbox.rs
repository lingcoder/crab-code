//! Sandbox policy for restricting child process capabilities.
//!
//! Gated behind `feature = "sandbox"`.

/// Policy describing what a sandboxed process is allowed to do.
#[cfg(feature = "sandbox")]
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub allow_network: bool,
    pub allow_write: Vec<std::path::PathBuf>,
    pub allow_read: Vec<std::path::PathBuf>,
}

/// Apply sandbox restrictions to a child process command.
#[cfg(feature = "sandbox")]
pub fn apply(
    _policy: &SandboxPolicy,
    _cmd: &mut tokio::process::Command,
) -> crab_common::Result<()> {
    todo!()
}
