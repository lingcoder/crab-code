use crab_core::message::Message;
use std::future::Future;
use std::pin::Pin;

use crate::conversation::Conversation;

/// 5-level compaction strategy, triggered by context usage thresholds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionStrategy {
    /// Level 1 (70-80%): Trim old tool output, keep only summary lines.
    Snip,
    /// Level 2 (80-85%): Replace large results (>500 tokens) with AI summary.
    Microcompact,
    /// Level 3 (85-90%): Summarize old messages via small model.
    Summarize,
    /// Level 4 (90-95%): Keep recent N turns + summarize the rest.
    Hybrid { keep_recent: usize },
    /// Level 5 (>95%): Emergency truncation via `Conversation::truncate_to_budget`.
    Truncate,
}

/// Abstraction for the LLM client used during compaction.
/// Decouples compaction logic from a specific API backend.
pub trait CompactionClient: Send + Sync {
    fn summarize(
        &self,
        messages: &[Message],
        instruction: &str,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<String>> + Send + '_>>;
}

pub async fn compact(
    conversation: &mut Conversation,
    strategy: CompactionStrategy,
    _client: &impl CompactionClient,
) -> crab_common::Result<()> {
    match strategy {
        CompactionStrategy::Truncate => {
            // Emergency: use core's truncate_to_budget
            let budget = conversation.context_window * 50 / 100;
            conversation.inner.truncate_to_budget(budget);
            Ok(())
        }
        _ => {
            // Other strategies require LLM summarization
            todo!()
        }
    }
}
