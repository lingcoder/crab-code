//! Tool allow-lists for Coordinator Mode.
//!
//! The Coordinator role has no hands-on execution tools — it can only
//! delegate via `Agent`, talk via `SendMessage`, and stop running tasks via
//! `TaskStop`. Workers spawned by a Coordinator additionally cannot nest
//! their own team creation or message other workers directly.
//!
//! These constants mirror CCB's `ASYNC_AGENT_ALLOWED_TOOLS` /
//! `INTERNAL_WORKER_TOOLS` lists but stay a static `&[&str]` slice for
//! compile-time visibility.

use crab_tools::builtin::agent::AGENT_TOOL_NAME;
use crab_tools::builtin::task::TASK_STOP_TOOL_NAME;
use crab_tools::builtin::team::{
    SEND_MESSAGE_TOOL_NAME, TEAM_CREATE_TOOL_NAME, TEAM_DELETE_TOOL_NAME,
};

/// Tools a Coordinator may invoke. The Coordinator's registry is reduced
/// to exactly these names before the session starts.
pub const COORDINATOR_TOOLS: &[&str] =
    &[AGENT_TOOL_NAME, SEND_MESSAGE_TOOL_NAME, TASK_STOP_TOOL_NAME];

/// Tools a Worker (spawned via the Coordinator's `Agent` tool) is *not*
/// allowed to use. This prevents nested Coordinator Mode setups and
/// peer-to-peer messaging that would bypass Coordinator oversight.
pub const WORKER_DENIED_TOOLS: &[&str] = &[
    TEAM_CREATE_TOOL_NAME,
    TEAM_DELETE_TOOL_NAME,
    SEND_MESSAGE_TOOL_NAME,
];

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
    fn worker_denied_tools_blocks_team_mgmt_and_messaging() {
        assert!(WORKER_DENIED_TOOLS.contains(&"TeamCreate"));
        assert!(WORKER_DENIED_TOOLS.contains(&"TeamDelete"));
        assert!(WORKER_DENIED_TOOLS.contains(&"SendMessage"));
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
