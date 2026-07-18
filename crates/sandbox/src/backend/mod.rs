//! Platform-specific sandbox backends + [`create_sandbox`] + [`apply_policy`].
//!
//! Platform mapping (all placeholders today; enforcement deferred):
//! - Linux: `landlock`
//! - macOS: `seatbelt`
//! - Windows: `windows` (restricted token / `AppContainer`)
//! - other: `noop`

pub mod factory;
pub mod landlock;
pub mod noop;
pub mod seatbelt;
pub mod windows;

pub use factory::{apply_policy, create_sandbox};
pub use landlock::LandlockSandbox;
pub use noop::NoopSandbox;
pub use seatbelt::SeatbeltSandbox;
pub use windows::WindowsSandbox;
