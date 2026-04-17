//! Layer 1 — Teammate spawner backends. Creates / drives / tears down a
//! teammate process or in-proc task, independent of topology (Swarm uses the
//! same backends as Coordinator Mode).
//!
//! - [`spawner`] — [`SwarmBackend`] trait + [`InProcessBackend`] + [`TmuxBackend`]
//! - [`tmux`] — [`PaneInfo`] + [`PaneManager`] tmux CLI wrapper (used by `TmuxBackend`)
//! - [`teammate`] — [`Teammate`] / [`TeammateConfig`] / [`TeammateState`] value types
//! - [`init_script`] — [`generate_init_script`] bash script generator for teammate env

pub mod init_script;
pub mod spawner;
pub mod teammate;
pub mod tmux;

pub use init_script::generate_init_script;
pub use spawner::{InProcessBackend, SwarmBackend, TmuxBackend};
pub use teammate::{Teammate, TeammateConfig, TeammateState};
pub use tmux::{PaneInfo, PaneManager};
