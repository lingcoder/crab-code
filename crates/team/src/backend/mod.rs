//! Layer 1 — Teammate spawner backend. Creates / drives / tears down a
//! teammate task, aligned with Claude Code's in-process teams model.
//!
//! - [`spawner`] — [`TeammateBackend`] trait + [`InProcessBackend`]
//! - [`teammate`] — [`Teammate`] / [`TeammateConfig`] / [`TeammateState`] value types

pub mod spawner;
pub mod teammate;

pub use spawner::{InProcessBackend, TeammateBackend, TeammateRunCtx, TeammateRunner};
pub use teammate::{Teammate, TeammateConfig, TeammateState};
