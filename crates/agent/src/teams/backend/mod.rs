//! Layer 1 — Teammate spawner backend. Creates / drives / tears down a
//! teammate task, aligned with Claude Code's in-process teams model.
//!
//! - [`spawner`] — [`SwarmBackend`] trait + [`InProcessBackend`]
//! - [`teammate`] — [`Teammate`] / [`TeammateConfig`] / [`TeammateState`] value types

pub mod spawner;
pub mod teammate;

pub use spawner::{InProcessBackend, SwarmBackend};
pub use teammate::{Teammate, TeammateConfig, TeammateState};
