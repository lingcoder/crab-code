//! Buddy companion widget — an ASCII-art mascot that lives in the TUI.
//!
//! The buddy is generated deterministically from the session identifier,
//! giving each session a unique companion with its own species, eyes,
//! hat, and personality.
//!
//! # Submodules
//!
//! - [`buddy`]        — the [`Buddy`] widget state + `Widget` impl
//! - [`sprite`]       — seed-based PRNG sprite generation
//! - [`personality`]  — personality traits derived from the sprite
//! - [`notification`] — speech-bubble notifications from the buddy

#[allow(clippy::module_inception)]
pub mod buddy;
pub mod companion;
pub mod notification;
pub mod personality;
pub mod prompt;
pub mod render;
pub mod sprite;

pub use buddy::Buddy;
