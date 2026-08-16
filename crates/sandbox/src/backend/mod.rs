//! Platform-specific sandbox backends + [`create_sandbox`] + [`prepare_command`].
//!
//! Platform mapping:
//! - Linux: `landlock` (real enforcement via `pre_exec`)
//! - macOS: `seatbelt` (real enforcement via `sandbox-exec`)
//! - Windows: `windows` (no isolation — fail-open with a warning)
//! - other: `noop` (passthrough)

pub mod factory;
pub mod landlock;
pub mod noop;
pub mod seatbelt;
pub mod windows;

pub use factory::{create_sandbox, prepare_command};
pub use landlock::LandlockSandbox;
pub use noop::NoopSandbox;
pub use seatbelt::SeatbeltSandbox;
pub use windows::WindowsSandbox;
