//! Layer 2b — Coordinator Mode.
//!
//! Star-topology overlay on top of Layer 1 [`crate::teams`]: a designated
//! Coordinator agent is stripped of hands-on tools (only `Agent` /
//! `SendMessage` / `TaskStop`), Workers run with an allow-list, and the
//! Coordinator gets an anti-pattern prompt overlay ("understand before
//! delegating").
//!
//! This module is opt-in via `CRAB_COORDINATOR_MODE=1` (see
//! `SessionConfig::coordinator_mode`). The Layer 1 pool ([`crate::teams::WorkerPool`])
//! runs unconditional base infrastructure; Coordinator Mode is additive.
//!
//! Wiring: [`crate::session::AgentSession::new`] calls [`CoordinatorMode::from_config`];
//! if it returns `Some(c)`, `c.apply(&mut registry, &mut system_prompt)` is
//! invoked before the session is handed out.
//!
//! See `docs/architecture.md` § Multi-Agent Three-Layer Architecture.

#[allow(clippy::module_inception)]
mod coordinator;
pub mod gating;
pub mod permission_sync;
pub mod prompt;
pub mod tool_acl;

pub use coordinator::CoordinatorMode;
pub use permission_sync::{PermissionDecisionEvent, PermissionSyncManager};
