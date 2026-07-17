use serde::{Deserialize, Serialize};

/// When a hook fires relative to tool or lifecycle events.
///
/// This is the canonical definition shared by configuration parsing
/// (`crab_config`) and runtime execution (`crab_hooks`). The full Claude
/// Code hooks event set is accepted (`PascalCase` wire names), so any
/// CC-style hooks configuration parses unchanged. Events whose lifecycle
/// point is not wired yet parse and register but never fire; see
/// [`Self::is_dispatched`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HookTrigger {
    /// Before a tool is invoked.
    PreToolUse,
    /// After a tool completes successfully.
    PostToolUse,
    /// After a tool fails.
    PostToolUseFailure,
    /// When user submits a prompt (before it reaches the LLM).
    UserPromptSubmit,
    /// When the query loop is about to exit (model produced no tool calls).
    /// A hook returning `Retry` continues the loop instead of stopping.
    Stop,
    /// When the query loop exits due to an error.
    StopFailure,
    /// When a notification is sent.
    Notification,
    /// When a session starts.
    SessionStart,
    /// When a session ends.
    SessionEnd,
    /// After the user accepts the trust dialog for a project for the
    /// first time. Fires once per project lifetime; useful for one-shot
    /// project setup (install hooks, materialize config, ...).
    Setup,
    /// When a watched file changes (settings.json, skills dir).
    FileChanged,
    /// Before conversation compaction starts.
    PreCompact,
    /// When conversation compaction completes.
    PostCompact,
    /// When a tool needs interactive permission. A hook may resolve the
    /// request programmatically (allow/deny) instead of prompting.
    PermissionRequest,
    /// After a permission request was denied.
    PermissionDenied,
    /// When a subagent starts.
    SubagentStart,
    /// When a subagent stops.
    SubagentStop,
    /// When a teammate goes idle.
    TeammateIdle,
    /// When a background task is created.
    TaskCreated,
    /// When a background task completes.
    TaskCompleted,
    /// When an MCP server requests user input (elicitation).
    Elicitation,
    /// Before an elicitation response is sent back to the MCP server.
    ElicitationResult,
    /// When configuration changes at runtime.
    ConfigChange,
    /// When the working directory changes.
    CwdChanged,
    /// After project instructions (`AGENTS.md` / memory) are loaded.
    InstructionsLoaded,
    /// After a git worktree is created.
    WorktreeCreate,
    /// Before a git worktree is removed.
    WorktreeRemove,
}

impl HookTrigger {
    /// The CC-protocol event name, as sent in the stdin payload's
    /// `hook_event_name` field.
    #[must_use]
    pub fn event_name(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::Notification => "Notification",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::Setup => "Setup",
            Self::FileChanged => "FileChanged",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionDenied => "PermissionDenied",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::TeammateIdle => "TeammateIdle",
            Self::TaskCreated => "TaskCreated",
            Self::TaskCompleted => "TaskCompleted",
            Self::Elicitation => "Elicitation",
            Self::ElicitationResult => "ElicitationResult",
            Self::ConfigChange => "ConfigChange",
            Self::CwdChanged => "CwdChanged",
            Self::InstructionsLoaded => "InstructionsLoaded",
            Self::WorktreeCreate => "WorktreeCreate",
            Self::WorktreeRemove => "WorktreeRemove",
        }
    }

    /// Whether the runtime currently has a dispatch point for this event.
    ///
    /// Every event parses and registers; undispatched ones simply never
    /// fire until their lifecycle point is wired. Keep this in sync when
    /// adding dispatch sites.
    #[must_use]
    pub fn is_dispatched(self) -> bool {
        matches!(
            self,
            Self::PreToolUse
                | Self::PostToolUse
                | Self::UserPromptSubmit
                | Self::Stop
                | Self::Notification
                | Self::SessionStart
                | Self::SessionEnd
                | Self::Setup
                | Self::FileChanged
                | Self::PostCompact
        )
    }

    /// All hook events, in a stable order.
    pub const ALL: [Self; 27] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::UserPromptSubmit,
        Self::Stop,
        Self::StopFailure,
        Self::Notification,
        Self::SessionStart,
        Self::SessionEnd,
        Self::Setup,
        Self::FileChanged,
        Self::PreCompact,
        Self::PostCompact,
        Self::PermissionRequest,
        Self::PermissionDenied,
        Self::SubagentStart,
        Self::SubagentStop,
        Self::TeammateIdle,
        Self::TaskCreated,
        Self::TaskCompleted,
        Self::Elicitation,
        Self::ElicitationResult,
        Self::ConfigChange,
        Self::CwdChanged,
        Self::InstructionsLoaded,
        Self::WorktreeCreate,
        Self::WorktreeRemove,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_trigger_serde_roundtrip() {
        for trigger in HookTrigger::ALL {
            let json = serde_json::to_string(&trigger).unwrap();
            assert_eq!(json, format!("\"{}\"", trigger.event_name()));
            let back: HookTrigger = serde_json::from_str(&json).unwrap();
            assert_eq!(back, trigger);
        }
    }

    #[test]
    fn all_cc_event_names_parse() {
        // The full CC HOOK_EVENTS set must round-trip through config parsing.
        let cc_events = [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "Notification",
            "UserPromptSubmit",
            "SessionStart",
            "SessionEnd",
            "Stop",
            "StopFailure",
            "SubagentStart",
            "SubagentStop",
            "PreCompact",
            "PostCompact",
            "PermissionRequest",
            "PermissionDenied",
            "Setup",
            "TeammateIdle",
            "TaskCreated",
            "TaskCompleted",
            "Elicitation",
            "ElicitationResult",
            "ConfigChange",
            "WorktreeCreate",
            "WorktreeRemove",
            "InstructionsLoaded",
            "CwdChanged",
            "FileChanged",
        ];
        assert_eq!(cc_events.len(), HookTrigger::ALL.len());
        for name in cc_events {
            let parsed: HookTrigger = serde_json::from_str(&format!("\"{name}\"")).unwrap();
            assert_eq!(parsed.event_name(), name);
        }
    }

    #[test]
    fn dispatched_events_are_a_subset() {
        let dispatched = HookTrigger::ALL
            .iter()
            .filter(|t| t.is_dispatched())
            .count();
        assert_eq!(dispatched, 10);
    }
}
