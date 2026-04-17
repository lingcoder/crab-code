//! Vim-style editing support for the TUI input box.
//!
//! Provides modal editing with Normal, Insert, Visual, and Command modes.
//! The [`VimHandler`] (in [`handler`]) wraps an `InputBox` and intercepts
//! key events to implement vim-like navigation and mode transitions.

pub mod handler;
pub mod mode;
pub mod motion;
pub mod operator;
pub mod register;
pub mod text_object;
pub mod transition;

pub use handler::{VimAction, VimHandler};
