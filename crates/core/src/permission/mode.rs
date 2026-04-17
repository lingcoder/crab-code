//! [`PermissionMode`] — how the agent treats tool invocations globally.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// All non-read-only tools require user confirmation.
    Default,
    /// Auto-approve file edits within the project; other mutations still prompt.
    AcceptEdits,
    /// Trust in-project file operations; out-of-project and dangerous still prompt.
    TrustProject,
    /// Auto-approve everything without prompting the user.
    DontAsk,
    /// Auto-approve everything (except `denied_tools`). Use with caution.
    Dangerously,
    /// Planning-only mode: the agent may read but not mutate.
    Plan,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::AcceptEdits => f.write_str("acceptEdits"),
            Self::TrustProject => f.write_str("trust-project"),
            Self::DontAsk => f.write_str("dontAsk"),
            Self::Dangerously => f.write_str("dangerously"),
            Self::Plan => f.write_str("plan"),
        }
    }
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "default" => Ok(Self::Default),
            "acceptEdits" | "accept-edits" | "accept_edits" => Ok(Self::AcceptEdits),
            "trust-project" | "trust_project" => Ok(Self::TrustProject),
            "dontAsk" | "dont-ask" | "dont_ask" => Ok(Self::DontAsk),
            "bypassPermissions" | "bypass-permissions" | "bypass_permissions" | "dangerously" => {
                Ok(Self::Dangerously)
            }
            "plan" => Ok(Self::Plan),
            other => Err(format!("unknown permission mode: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_mode_display() {
        assert_eq!(PermissionMode::Default.to_string(), "default");
        assert_eq!(PermissionMode::AcceptEdits.to_string(), "acceptEdits");
        assert_eq!(PermissionMode::TrustProject.to_string(), "trust-project");
        assert_eq!(PermissionMode::DontAsk.to_string(), "dontAsk");
        assert_eq!(PermissionMode::Dangerously.to_string(), "dangerously");
        assert_eq!(PermissionMode::Plan.to_string(), "plan");
    }

    #[test]
    fn permission_mode_from_str() {
        assert_eq!(
            "default".parse::<PermissionMode>().unwrap(),
            PermissionMode::Default
        );
        assert_eq!(
            "acceptEdits".parse::<PermissionMode>().unwrap(),
            PermissionMode::AcceptEdits
        );
        assert_eq!(
            "accept-edits".parse::<PermissionMode>().unwrap(),
            PermissionMode::AcceptEdits
        );
        assert_eq!(
            "trust-project".parse::<PermissionMode>().unwrap(),
            PermissionMode::TrustProject
        );
        assert_eq!(
            "trust_project".parse::<PermissionMode>().unwrap(),
            PermissionMode::TrustProject
        );
        assert_eq!(
            "dontAsk".parse::<PermissionMode>().unwrap(),
            PermissionMode::DontAsk
        );
        assert_eq!(
            "bypassPermissions".parse::<PermissionMode>().unwrap(),
            PermissionMode::Dangerously
        );
        assert_eq!(
            "dangerously".parse::<PermissionMode>().unwrap(),
            PermissionMode::Dangerously
        );
        assert_eq!(
            "plan".parse::<PermissionMode>().unwrap(),
            PermissionMode::Plan
        );
        assert!("unknown".parse::<PermissionMode>().is_err());
    }

    #[test]
    fn permission_mode_serde_roundtrip() {
        let mode = PermissionMode::TrustProject;
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, parsed);
    }

    #[test]
    fn permission_mode_all_variants_serde() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::TrustProject,
            PermissionMode::DontAsk,
            PermissionMode::Dangerously,
            PermissionMode::Plan,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: PermissionMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, parsed);
        }
    }
}
