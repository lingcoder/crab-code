//! Multi-agent team primitives for Crab Code.
//!
//! Domain-pure building blocks shared by all multi-agent execution modes:
//! message bus, mailbox routing, team roster, task lists, retry policies,
//! and the teammate backend abstraction.

pub mod backend;
pub mod bus;
pub mod mailbox;
pub mod retry;
pub mod roster;
pub mod task_list;
pub mod task_lock;

pub use backend::{InProcessBackend, Teammate, TeammateBackend, TeammateConfig, TeammateState};
pub use bus::{AgentMessage, AgentStatus, Envelope, MessageBus, event_channel};
pub use mailbox::MessageRouter;
pub use retry::{BackoffStrategy, RetryDecision, RetryPolicy, RetryTracker};
pub use roster::{Capability, Team, TeamMember, TeamMode};
pub use task_list::{SharedTaskList, Task, TaskList, TaskStatus, shared_task_list};
pub use task_lock::{claim_task, load_from_file as load_task_list_from_file, with_locked};
