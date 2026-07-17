//! Layer 1 multi-agent infrastructure — the base plumbing that every
//! multi-agent usage (Swarm / Coordinator Mode) builds on.
//!
//! Domain-pure primitives (bus, mailbox, roster, `task_list`, `task_lock`,
//! retry, backend) live in the `crab-team` crate. This module holds
//! the engine-coupled orchestration layer: worker, `worker_pool`,
//! coordinator.

pub mod coordinator;
pub mod spawn;
pub mod worker;
pub mod worker_pool;

// Re-export domain primitives from crab-team so existing callers
// (e.g. `crate::teams::MessageBus`) keep compiling without path changes.
pub use crab_team::*;

pub use coordinator::TeamCoordinator;
pub use worker::{AgentWorker, WorkerConfig, WorkerResult};
pub use worker_pool::{AgentHandle, WorkerPool};
