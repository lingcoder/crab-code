//! Process sandbox — trait-based core + per-platform backends.
//!
//! Top-level entry points:
//! - [`Sandbox`] / [`SandboxBackend`] / [`SandboxResult`] in [`traits`]
//! - [`SandboxPolicy`] / [`PathAccess`] / [`PathRule`] in [`policy`]
//! - [`backend::create_sandbox`] / [`backend::apply_policy`] for tool callers
//!
//! Platform selection is automatic (see [`backend::create_sandbox`]).
//! The `landlock` external crate is pulled in only on Linux via a
//! target-cfg'd dependency; no feature flag needed.

pub mod backend;
pub mod config;
pub mod doctor;
pub mod error;
pub mod policy;
pub mod traits;
pub mod violation;

pub use backend::{NoopSandbox, apply_policy, create_sandbox};
pub use policy::{PathAccess, PathRule, SandboxPolicy};
pub use traits::{Sandbox, SandboxBackend, SandboxResult};

#[cfg(target_os = "linux")]
pub use backend::LandlockSandbox;

#[cfg(target_os = "windows")]
pub use backend::WindowsJobSandbox;
