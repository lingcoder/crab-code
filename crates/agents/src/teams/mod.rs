//! Layer 1 multi-agent infrastructure — the base plumbing that every
//! multi-agent usage (Swarm / Coordinator Mode) builds on.
//!
//! Domain-pure primitives (roster, bus, mailbox, `task_lock`, retry, backend)
//! live in the `crab-team` crate and are referenced through `crab_team::`
//! paths directly. This module holds the engine-coupled layer: the agent loop
//! a teammate runs (`worker`), the marker-to-config translation (`spawn`), and
//! the single owner that drives them (`runner`).

pub mod permission;
pub mod runner;
pub mod spawn;
pub mod worker;

pub use permission::TeamPermissionHandler;
pub use runner::TeamRunner;
pub use worker::{AgentWorker, WorkerConfig, WorkerResult};
