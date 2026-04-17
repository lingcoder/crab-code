//! [`PermissionDecision`] — the outcome of a permission check.

/// Result of a permission check for a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool execution is allowed without user interaction.
    Allow,
    /// Tool execution is denied; includes the reason.
    Deny(String),
    /// Tool execution requires user confirmation; includes a prompt message.
    AskUser(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_decision_variants() {
        let allow = PermissionDecision::Allow;
        let deny = PermissionDecision::Deny("denied by policy".into());
        let ask = PermissionDecision::AskUser("confirm bash execution?".into());

        assert_eq!(allow, PermissionDecision::Allow);
        assert_eq!(deny, PermissionDecision::Deny("denied by policy".into()));
        assert_eq!(
            ask,
            PermissionDecision::AskUser("confirm bash execution?".into())
        );
    }

    #[test]
    fn permission_decision_deny_message() {
        let decision = PermissionDecision::Deny("tool is in denied list".into());
        if let PermissionDecision::Deny(msg) = &decision {
            assert!(msg.contains("denied"));
        } else {
            panic!("expected Deny");
        }
    }

    #[test]
    fn permission_decision_ask_message() {
        let decision = PermissionDecision::AskUser("Allow bash to run 'rm -rf /'?".into());
        if let PermissionDecision::AskUser(msg) = &decision {
            assert!(msg.contains("bash"));
        } else {
            panic!("expected AskUser");
        }
    }
}
