//! Platform-specific sandbox backends + [`create_sandbox`] + [`apply_policy`].
//!
//! Auto-selection precedence:
//! - Linux: `landlock` (if kernel ≥ 5.13) → `noop`
//! - Windows: `windows` (Job Object) → `noop`
//! - macOS / other: `noop`
//!
//! Each OS-specific backend file is cfg-gated to its target OS; the
//! top-level [`create_sandbox`] picks the best one compiled for the
//! current platform.

pub mod factory;
pub mod noop;

#[cfg(target_os = "linux")]
pub mod landlock;

#[cfg(target_os = "windows")]
pub mod windows;

pub use factory::{apply_policy, create_sandbox};
pub use noop::NoopSandbox;

#[cfg(target_os = "linux")]
pub use landlock::LandlockSandbox;

#[cfg(target_os = "windows")]
pub use windows::WindowsJobSandbox;
