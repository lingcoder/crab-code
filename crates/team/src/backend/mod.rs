//! Teammate spawner backend. Creates / drives / tears down teammate agent
//! instances, aligned with Claude Code's in-process teams model.
//!
//! - [`spawner`] — [`TeammateBackend`] trait + [`InProcessBackend`]
//! - [`teammate`] — [`TeammateConfig`] spawn parameters

pub mod spawner;
pub mod teammate;

pub use spawner::{InProcessBackend, MAIN_AGENT, TeammateBackend, TeammateRunCtx, TeammateRunner};
pub use teammate::{DEFAULT_CONTEXT_WINDOW, TeammateConfig};
