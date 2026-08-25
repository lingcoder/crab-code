//! Multi-agent team primitives for Crab Code.
//!
//! Domain-pure building blocks shared by all multi-agent execution modes:
//! message bus, mailbox routing, team roster, retry policies, and the
//! teammate backend abstraction.
//!
//! The work-queue model itself lives in `crab_core::task`; this crate only
//! adds the cross-process claiming protocol on top of it ([`task_lock`]).

pub mod backend;
pub mod bus;
pub mod mailbox;
pub mod retry;
pub mod roster;
pub mod task_lock;

pub use backend::{DEFAULT_CONTEXT_WINDOW, InProcessBackend, TeammateBackend, TeammateConfig};
pub use bus::{AgentMessage, AgentStatus, Envelope, MessageBus, event_channel};
pub use mailbox::MessageRouter;
pub use retry::{BackoffStrategy, RetryDecision, RetryPolicy, RetryTracker};
pub use roster::{Capability, Lifetime, Team, TeamMode, Teammate, TeammateState};
pub use task_lock::{claim_task, load_from_file as load_task_list_from_file, with_locked};
