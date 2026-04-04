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
    Warning {
        used: u64,
        limit: u64,
        percent: u8,
    },
    NeedsCompaction {
        used: u64,
        limit: u64,
        percent: u8,
    },
}
