//! Computer-use tool — desktop automation via screenshot, input, and window management.
//!
//! Provides a unified [`ComputerUseTool`] that dispatches to platform-
//! specific backends for taking screenshots, simulating keyboard/mouse
//! input, and enumerating desktop windows. Platform integration is not
//! yet available; all actions return informational messages indicating
//! the feature requires a native backend.

pub mod input;
pub mod platform;
pub mod screenshot;
pub mod tool;
pub mod window;

pub use tool::{COMPUTER_USE_TOOL_NAME, ComputerUseTool};
