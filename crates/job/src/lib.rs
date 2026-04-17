//! `crab-job` — unified scheduling primitives for the whole workspace.
//!
//! Replaces hand-rolled `tokio::time::interval` and `sleep_until` scatter
//! across `crab-mcp` (heartbeat), `crab-agent` (proactive timers),
//! `crab-remote` (server-scheduled triggers), and user-facing cron jobs.
//! One API, one view — the TUI can render "pending jobs", the web UI can
//! show a jobs panel, and the CLI can offer `crab jobs list / cancel`.
//!
//! ## Capabilities (planned; scaffolded here)
//!
//! - **One-shot** — "run task once after delay / at instant".
//! - **Interval** — "run every N seconds" (e.g. MCP heartbeat).
//! - **Cron** — "every day at 09:00" via the [`croner`] crate's expressions.
//! - **Persistence** — cron jobs survive process restart; heartbeats are
//!   in-memory only. Backend trait decides per-kind policy.
//!
//! ## Module layout (scaffold; full impl in Phase α)
//!
//! ```text
//! crab-job/
//! ├── id.rs         JobId / JobHandle    ← this commit
//! ├── spec.rs       JobSpec enum (one-shot | interval | cron)
//! ├── scheduler.rs  JobScheduler + JobHandler trait — Phase α
//! └── storage/      persistence backends (memory / json-file) — Phase α
//! ```

pub mod id;

pub use id::{JobId, JobKind};
