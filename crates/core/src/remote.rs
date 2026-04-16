//! Remote (claude.ai) session and trigger identifiers — shared with TUI.
//!
//! Business behaviour (spawning, polling, cron scheduling) lives in
//! `crab-remote`; this module only carries the shapes `core::Event` and
//! the TUI need to display progress.

use serde::{Deserialize, Serialize};

/// Stable identifier for a remote agent session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteSessionId(pub String);

/// Stable identifier for a saved remote trigger.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub String);

/// Lifecycle state of a remote session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteSessionStatus {
    Pending,
    Running,
    Succeeded,
    Failed(String),
    Cancelled,
}

/// Snapshot of a remote session for UI display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSessionInfo {
    pub id: RemoteSessionId,
    pub prompt_preview: String,
    /// Unix epoch millis.
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(RemoteSessionId("abc".into()));
        assert!(set.contains(&RemoteSessionId("abc".into())));
    }

    #[test]
    fn status_serde_roundtrip() {
        let s = RemoteSessionStatus::Failed("timeout".into());
        let json = serde_json::to_string(&s).unwrap();
        let back: RemoteSessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn info_serde_roundtrip() {
        let info = RemoteSessionInfo {
            id: RemoteSessionId("sess_1".into()),
            prompt_preview: "fix the bug".into(),
            created_at: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: RemoteSessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }
}
