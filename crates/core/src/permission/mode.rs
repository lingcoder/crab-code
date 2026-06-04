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
    /// Never prompt: anything that would require confirmation is denied.
    /// Only read-only and explicitly-allowed tools run.
    DontAsk,
    /// Auto-approve everything (except `denied_tools`). Use with caution.
    Dangerously,
    /// Planning-only mode: the agent may read but not mutate.
    Plan,
    /// Heuristic classifier: Safe = auto-allow, Risky = prompt, Dangerous = deny.
    Auto,
}

impl PermissionMode {
    fn restrictiveness(self) -> u8 {
        // DontAsk denies anything that would prompt, so it is strict — second
        // only to Plan. A sub-agent must never be granted a more permissive
        // mode than a DontAsk parent.
        match self {
            Self::Plan => 0,
            Self::DontAsk => 1,
            Self::Default => 2,
            Self::Auto => 3,
            Self::AcceptEdits => 4,
            Self::TrustProject => 5,
            Self::Dangerously => 6,
        }
    }

    /// Return the more restrictive of `self` and `ceiling`.
    /// Sub-agents use this to ensure they never exceed their parent's mode.
    #[must_use]
    pub fn restrict_to(self, ceiling: Self) -> Self {
        if self.restrictiveness() <= ceiling.restrictiveness() {
            self
        } else {
            ceiling
        }
    }
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
            Self::Auto => f.write_str("auto"),
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
            "auto" => Ok(Self::Auto),
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
        assert_eq!(PermissionMode::Auto.to_string(), "auto");
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
        assert_eq!(
            "auto".parse::<PermissionMode>().unwrap(),
            PermissionMode::Auto
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
    fn restrict_to_caps_at_ceiling() {
        assert_eq!(
            PermissionMode::Dangerously.restrict_to(PermissionMode::Default),
            PermissionMode::Default
        );
    }

    #[test]
    fn restrict_to_keeps_lower_when_self_is_more_restrictive() {
        assert_eq!(
            PermissionMode::Default.restrict_to(PermissionMode::Dangerously),
            PermissionMode::Default
        );
    }

    #[test]
    fn restrict_to_plan_always_wins() {
        for ceiling in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::TrustProject,
            PermissionMode::DontAsk,
            PermissionMode::Dangerously,
            PermissionMode::Plan,
            PermissionMode::Auto,
        ] {
            assert_eq!(
                PermissionMode::Plan.restrict_to(ceiling),
                PermissionMode::Plan
            );
        }
    }

    #[test]
    fn restrict_to_dont_ask_caps_more_permissive_children() {
        // DontAsk is strict: a child set to a more permissive mode must be
        // capped back to DontAsk.
        for child in [
            PermissionMode::Default,
            PermissionMode::Auto,
            PermissionMode::AcceptEdits,
            PermissionMode::TrustProject,
            PermissionMode::Dangerously,
        ] {
            assert_eq!(
                child.restrict_to(PermissionMode::DontAsk),
                PermissionMode::DontAsk,
                "child {child} under a DontAsk ceiling"
            );
        }
        // Plan is stricter still and is never loosened to DontAsk.
        assert_eq!(
            PermissionMode::Plan.restrict_to(PermissionMode::DontAsk),
            PermissionMode::Plan
        );
    }

    #[test]
    fn restrict_to_equal_modes() {
        assert_eq!(
            PermissionMode::AcceptEdits.restrict_to(PermissionMode::AcceptEdits),
            PermissionMode::AcceptEdits
        );
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
            PermissionMode::Auto,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: PermissionMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, parsed);
        }
    }
}
