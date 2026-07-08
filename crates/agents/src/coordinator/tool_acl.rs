//! Tool allow-lists for Coordinator Mode.
//!
//! The Coordinator role has no hands-on execution tools — it can only
//! delegate via `Agent`, talk via `SendMessage`, and stop running tasks via
//! `TaskStop`. Workers spawned by a Coordinator cannot message other workers
//! directly; all cross-worker coordination flows through the Coordinator.
//!
//! These constants stay a static `&[&str]` slice for compile-time
//! visibility.

use crab_tools::builtin::agent::AGENT_TOOL_NAME;
use crab_tools::builtin::send_message::SEND_MESSAGE_TOOL_NAME;
use crab_tools::builtin::task::TASK_STOP_TOOL_NAME;

/// Tools a Coordinator may invoke. The Coordinator's registry is reduced
/// to exactly these names before the session starts.
pub const COORDINATOR_TOOLS: &[&str] =
    &[AGENT_TOOL_NAME, SEND_MESSAGE_TOOL_NAME, TASK_STOP_TOOL_NAME];

/// Tools a Worker (spawned via the Coordinator's `Agent` tool) is *not*
/// allowed to use. Peer-to-peer messaging would bypass Coordinator
/// oversight.
pub const WORKER_DENIED_TOOLS: &[&str] = &[SEND_MESSAGE_TOOL_NAME];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_tools_are_exactly_three() {
        assert_eq!(COORDINATOR_TOOLS.len(), 3);
        assert!(COORDINATOR_TOOLS.contains(&"Agent"));
        assert!(COORDINATOR_TOOLS.contains(&"SendMessage"));
        assert!(COORDINATOR_TOOLS.contains(&"TaskStop"));
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
