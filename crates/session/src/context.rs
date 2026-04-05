use crate::conversation::Conversation;

/// Manages context window budget and triggers compaction when needed.
#[derive(Debug, Clone)]
pub struct ContextManager {
    pub warn_threshold_percent: u8,
    pub compact_threshold_percent: u8,
}

impl Default for ContextManager {
    fn default() -> Self {
        Self {
            warn_threshold_percent: 70,
            compact_threshold_percent: 80,
        }
    }
}

impl ContextManager {
    /// Check usage and return the appropriate action.
    pub fn check(&self, conversation: &Conversation) -> ContextAction {
        let used = conversation.estimated_tokens();
        let limit = conversation.context_window;
        if limit == 0 {
            return ContextAction::Ok;
        }
        #[allow(clippy::cast_possible_truncation)]
        let percent = (used * 100 / limit) as u8;
        if percent >= self.compact_threshold_percent {
            ContextAction::NeedsCompaction {
                used,
                limit,
                percent,
            }
        } else if percent >= self.warn_threshold_percent {
            ContextAction::Warning {
                used,
                limit,
                percent,
            }
        } else {
            ContextAction::Ok
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAction {
    Ok,
    Warning { used: u64, limit: u64, percent: u8 },
    NeedsCompaction { used: u64, limit: u64, percent: u8 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Conversation;

    #[test]
    fn default_thresholds() {
        let cm = ContextManager::default();
        assert_eq!(cm.warn_threshold_percent, 70);
        assert_eq!(cm.compact_threshold_percent, 80);
    }

    #[test]
    fn check_empty_conversation_is_ok() {
        let cm = ContextManager::default();
        let conv = Conversation::new("s".into(), String::new(), 100_000);
        assert_eq!(cm.check(&conv), ContextAction::Ok);
    }

    #[test]
    fn check_zero_context_window_is_ok() {
        let cm = ContextManager::default();
        let mut conv = Conversation::new("s".into(), String::new(), 0);
        conv.push_user("hello");
        assert_eq!(cm.check(&conv), ContextAction::Ok);
    }

    #[test]
    fn check_below_warn_threshold_is_ok() {
        let cm = ContextManager::default();
        // With a very large context window, a small conversation should be Ok
        let mut conv = Conversation::new("s".into(), String::new(), 1_000_000);
        conv.push_user("short message");
        assert_eq!(cm.check(&conv), ContextAction::Ok);
    }

    #[test]
    fn check_returns_warning_at_threshold() {
        let cm = ContextManager {
            warn_threshold_percent: 50,
            compact_threshold_percent: 80,
        };
        // Create a conversation with a tiny context window to trigger warning
        let mut conv = Conversation::new("s".into(), String::new(), 100);
        // Each message is roughly estimated at ~role_len + content_len / 4 tokens
        // Push enough content to get above 50% of 100 tokens
        let big_text = "x".repeat(300); // ~75 tokens estimate
        conv.push_user(&big_text);
        let action = cm.check(&conv);
        assert!(
            matches!(
                action,
                ContextAction::Warning { .. } | ContextAction::NeedsCompaction { .. }
            ),
            "Expected Warning or NeedsCompaction, got {action:?}"
        );
    }

    #[test]
    fn check_returns_needs_compaction_above_compact_threshold() {
        let cm = ContextManager {
            warn_threshold_percent: 10,
            compact_threshold_percent: 20,
        };
        let mut conv = Conversation::new("s".into(), String::new(), 100);
        let big_text = "x".repeat(500); // ~125 tokens, well above 20% of 100
        conv.push_user(&big_text);
        let action = cm.check(&conv);
        assert!(
            matches!(action, ContextAction::NeedsCompaction { .. }),
            "Expected NeedsCompaction, got {action:?}"
        );
    }

    #[test]
    fn context_action_equality() {
        assert_eq!(ContextAction::Ok, ContextAction::Ok);
        assert_ne!(
            ContextAction::Ok,
            ContextAction::Warning {
                used: 70,
                limit: 100,
                percent: 70,
            }
        );
    }

    #[test]
    fn custom_thresholds() {
        let cm = ContextManager {
            warn_threshold_percent: 50,
            compact_threshold_percent: 60,
        };
        assert_eq!(cm.warn_threshold_percent, 50);
        assert_eq!(cm.compact_threshold_percent, 60);
    }
}
