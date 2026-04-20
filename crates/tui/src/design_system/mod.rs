//! Reusable TUI primitives that compose into higher-level views.
//!
//! The goal is that every modal, palette, or panel in Crab is built from
//! the widgets in this module. Widgets here do not own application state
//! — callers feed in data + a focus flag, and the widget renders a
//! consistent visual language driven by [`crate::theme::Theme`].

pub mod button;
pub mod dialog;
pub mod pane;
pub mod scrollbox;
pub mod tabs;

pub use button::{Button, ButtonState};
pub use dialog::{Dialog, DialogAction};
pub use pane::Pane;
pub use scrollbox::{ScrollBox, ScrollBoxState};
pub use tabs::Tabs;
