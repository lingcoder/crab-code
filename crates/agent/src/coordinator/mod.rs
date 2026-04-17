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
//! Phase 2 scaffold — only [`permission_sync`] is populated today; the
//! `gating` / `tool_acl` / `prompt` modules land in Phase 3.
//!
//! See `docs/architecture.md` § Multi-Agent Three-Layer Architecture.

pub mod permission_sync;

pub use permission_sync::{PermissionDecisionEvent, PermissionSyncManager};
