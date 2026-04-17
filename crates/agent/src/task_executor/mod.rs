//! Layer 1 — Task execution polymorphism.
//!
//! CCB has 7 concrete `*Task` classes (`LocalAgentTask` /
//! `InProcessTeammateTask` / `RemoteAgentTask` / `BashBackgroundTask` / …).
//! Crab models this as a single [`TaskKind`] discriminator plus concrete
//! modules. Only [`local_agent`] is implemented today; others
//! (`InProcess` / `Remote` / `BashBackground`) land as separate sibling
//! files when real use cases appear.
//!
//! See `docs/architecture.md` § Multi-Agent Three-Layer Architecture.

pub mod local_agent;

pub use local_agent::{AgentWorker, Worker, WorkerConfig, WorkerResult};

/// Kind of task an executor runs. Mirrors CCB's task-type discriminator but
/// stays open-ended — a new variant requires a matching impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// In-process sub-agent running the full query loop (the current default).
    LocalAgent,
    /// In-process teammate running a lighter script loop. Not yet implemented.
    InProcess,
    /// Remote teammate driven through `crab-remote`. Not yet implemented.
    Remote,
    /// Background bash process with streamed output. Not yet implemented.
    BashBackground,
}
