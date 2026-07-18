//! Process sandbox — trait-based core + per-platform backend placeholders.
//!
//! The trait/policy layer is real; every backend is an empty placeholder
//! that reports `applied: false` until enforcement lands (deferred).
//!
//! Top-level entry points:
//! - [`Sandbox`] / [`SandboxBackend`] / [`SandboxResult`] in [`traits`]
//! - [`SandboxPolicy`] / [`PathAccess`] / [`PathRule`] in [`policy`]
//! - [`backend::create_sandbox`] / [`backend::apply_policy`] for tool callers

pub mod backend;
pub mod config;
pub mod doctor;
pub mod error;
pub mod policy;
pub mod traits;
pub mod violation;

pub use backend::{
    LandlockSandbox, NoopSandbox, SeatbeltSandbox, WindowsSandbox, apply_policy, create_sandbox,
};
pub use policy::{PathAccess, PathRule, SandboxPolicy};
pub use traits::{Sandbox, SandboxBackend, SandboxResult};
