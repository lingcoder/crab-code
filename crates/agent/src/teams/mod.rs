//! Layer 1 multi-agent infrastructure — the base plumbing that every
//! multi-agent usage (Swarm / Coordinator Mode) builds on.
//!
//! - [`mailbox`] — [`MessageRouter`] per-agent inbox routing
//! - [`bus`] — lower-level `MessageBus` + [`AgentMessage`] / [`Envelope`] types
//! - [`task_list`] — shared [`TaskList`] with pending/claimed/completed tasks
//! - [`roster`] — [`Team`] / [`TeamMember`] / [`TeamMode`] roster types
//! - [`retry`] — [`RetryPolicy`] and [`RetryTracker`] for failed tasks
//! - [`worker_pool`] — [`WorkerPool`] + [`AgentHandle`] for spawn/collect/cancel lifecycle
//!
//! This module is unconditional base infrastructure (no env/settings gate);
//! Coordinator Mode (Layer 2b) is the only gated overlay. See
//! `docs/architecture.md` § Multi-Agent Three-Layer Architecture.

pub mod backend;
pub mod bus;
pub mod mailbox;
pub mod retry;
pub mod roster;
pub mod task_list;
pub mod worker_pool;

pub use backend::{
    InProcessBackend, PaneInfo, PaneManager, SwarmBackend, Teammate, TeammateConfig, TeammateState,
    TmuxBackend, generate_init_script,
};
pub use bus::{AgentMessage, AgentStatus, Envelope, MessageBus, event_channel};
pub use mailbox::MessageRouter;
pub use retry::{BackoffStrategy, RetryDecision, RetryPolicy, RetryTracker};
pub use roster::{Capability, Team, TeamMember, TeamMode};
pub use task_list::{SharedTaskList, Task, TaskList, TaskStatus, shared_task_list};
pub use worker_pool::{AgentHandle, WorkerPool};
