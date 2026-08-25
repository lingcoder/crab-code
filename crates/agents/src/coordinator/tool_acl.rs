//! Tool allow-lists for Coordinator Mode.
//!
//! The Coordinator role has no hands-on execution tools — it delegates via
//! `Agent`, talks via `SendMessage`, stops running work via `TaskStop`, and
//! keeps the shared work queue via the `Task*` bookkeeping tools. Workers
//! spawned by a Coordinator cannot message other workers directly; all
//! cross-worker coordination flows through the Coordinator or the queue.
//!
//! These constants stay a static `&[&str]` slice for compile-time
//! visibility.

use crab_tools::builtin::agent::AGENT_TOOL_NAME;
use crab_tools::builtin::send_message::SEND_MESSAGE_TOOL_NAME;
use crab_tools::builtin::task::{
    TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_STOP_TOOL_NAME,
    TASK_UPDATE_TOOL_NAME,
};

/// Tools a Coordinator may invoke. The Coordinator's registry is reduced
/// to exactly these names before the session starts.
///
/// The queue tools are bookkeeping, not execution: a coordinator that cannot
/// record or track work items has no way to delegate through the queue.
pub const COORDINATOR_TOOLS: &[&str] = &[
    AGENT_TOOL_NAME,
    SEND_MESSAGE_TOOL_NAME,
    TASK_STOP_TOOL_NAME,
    TASK_CREATE_TOOL_NAME,
    TASK_LIST_TOOL_NAME,
    TASK_GET_TOOL_NAME,
    TASK_UPDATE_TOOL_NAME,
];

/// Tools a Worker (spawned via the Coordinator's `Agent` tool) may not use.
///
/// Peer-to-peer messaging would bypass Coordinator oversight. Workers keep
/// `TaskClaim` so they can still take work off the shared queue.
pub const WORKER_DENIED_TOOLS: &[&str] = &[SEND_MESSAGE_TOOL_NAME];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_tools_cover_delegation_and_bookkeeping() {
        assert!(COORDINATOR_TOOLS.contains(&"Agent"));
        assert!(COORDINATOR_TOOLS.contains(&"SendMessage"));
        assert!(COORDINATOR_TOOLS.contains(&"TaskStop"));
        assert!(COORDINATOR_TOOLS.contains(&"TaskCreate"));
        assert!(COORDINATOR_TOOLS.contains(&"TaskList"));
        assert!(COORDINATOR_TOOLS.contains(&"TaskGet"));
        assert!(COORDINATOR_TOOLS.contains(&"TaskUpdate"));
    }

    #[test]
    fn coordinator_has_no_hands_on_execution_tools() {
        for banned in ["Bash", "Edit", "Write", "Read", "TaskClaim"] {
            assert!(
                !COORDINATOR_TOOLS.contains(&banned),
                "{banned} must stay out of the coordinator's registry"
            );
        }
    }

    #[test]
    fn worker_denied_tools_blocks_peer_messaging() {
        assert_eq!(WORKER_DENIED_TOOLS, &["SendMessage"]);
    }

    #[test]
    fn coordinator_tools_do_not_overlap_worker_denied() {
        // A Coordinator CAN use SendMessage (it's how they communicate with
        // workers); Workers CANNOT. This is intentional asymmetry.
        let only_in_both = COORDINATOR_TOOLS
            .iter()
            .filter(|t| WORKER_DENIED_TOOLS.contains(*t))
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(only_in_both, vec!["SendMessage"]);
    }
}
