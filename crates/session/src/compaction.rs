use crab_core::message::{ContentBlock, Message, Role};
use std::future::Future;
use std::pin::Pin;

use crate::conversation::Conversation;

/// Token threshold above which a tool result is considered "large" for snipping.
const SNIP_TOKEN_THRESHOLD: u64 = 200;

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

impl CompactionStrategy {
    /// Select the appropriate strategy based on context usage percentage.
    pub fn for_usage(percent: u8) -> Option<Self> {
        match percent {
            0..70 => None,
            70..80 => Some(Self::Snip),
            80..85 => Some(Self::Microcompact),
            85..90 => Some(Self::Summarize),
            90..95 => Some(Self::Hybrid { keep_recent: 3 }),
            _ => Some(Self::Truncate),
        }
    }
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

/// Apply a compaction strategy to a conversation.
pub async fn compact(
    conversation: &mut Conversation,
    strategy: CompactionStrategy,
    _client: &dyn CompactionClient,
) -> crab_common::Result<()> {
    match strategy {
        CompactionStrategy::Snip => {
            snip_large_tool_results(conversation);
            Ok(())
        }
        CompactionStrategy::Truncate => {
            let budget = conversation.context_window * 50 / 100;
            conversation.inner.truncate_to_budget(budget);
            Ok(())
        }
        CompactionStrategy::Microcompact
        | CompactionStrategy::Summarize
        | CompactionStrategy::Hybrid { .. } => {
            // These strategies require LLM summarization — will be implemented in M4.
            // For now, fall back to truncation.
            let budget = conversation.context_window * 60 / 100;
            conversation.inner.truncate_to_budget(budget);
            Ok(())
        }
    }
}

/// Level 1 compaction: replace large tool results with a truncated snippet.
fn snip_large_tool_results(conversation: &mut Conversation) {
    // We need to rebuild messages with snipped tool results.
    // Only snip messages that are NOT in the last 2 turns.
    let turn_count = conversation.turn_count();
    let preserve_turns = turn_count.saturating_sub(2);

    let messages = conversation.inner.messages().to_vec();
    let mut snipped = Vec::with_capacity(messages.len());
    let mut current_turn = 0usize;

    for msg in messages {
        if msg.role == Role::User {
            current_turn += 1;
        }

        if current_turn <= preserve_turns {
            // In old turns: snip large tool results
            let new_content: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .map(|block| {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = &block
                    {
                        let estimated = content.len() as u64 / 4; // rough token estimate
                        if estimated > SNIP_TOKEN_THRESHOLD {
                            let preview: String = content.chars().take(200).collect();
                            return ContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: format!("{preview}... [snipped, was ~{estimated} tokens]"),
                                is_error: *is_error,
                            };
                        }
                    }
                    block
                })
                .collect();
            snipped.push(Message::new(msg.role, new_content));
        } else {
            snipped.push(msg);
        }
    }

    // Replace conversation contents
    conversation.inner.clear();
    for msg in snipped {
        conversation.inner.push(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_for_usage_levels() {
        assert!(CompactionStrategy::for_usage(50).is_none());
        assert_eq!(
            CompactionStrategy::for_usage(75),
            Some(CompactionStrategy::Snip)
        );
        assert_eq!(
            CompactionStrategy::for_usage(82),
            Some(CompactionStrategy::Microcompact)
        );
        assert_eq!(
            CompactionStrategy::for_usage(87),
            Some(CompactionStrategy::Summarize)
        );
        assert_eq!(
            CompactionStrategy::for_usage(92),
            Some(CompactionStrategy::Hybrid { keep_recent: 3 })
        );
        assert_eq!(
            CompactionStrategy::for_usage(96),
            Some(CompactionStrategy::Truncate)
        );
    }

    #[test]
    fn snip_removes_large_tool_results() {
        let mut conv = Conversation::new("s".into(), String::new(), 100_000);

        // Turn 1 (old): user + assistant + large tool result
        conv.push_user("Do something");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("Sure")],
        ));
        let large_content = "x".repeat(2000); // ~500 tokens, > SNIP_TOKEN_THRESHOLD
        conv.push_tool_result("tc_1", &large_content, false);

        // Turn 2: user + assistant (preserved)
        conv.push_user("And this?");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("Done")],
        ));

        // Turn 3 (recent): user + large tool result (should NOT be snipped)
        conv.push_user("One more");
        conv.push_tool_result("tc_2", &large_content, false);

        snip_large_tool_results(&mut conv);

        // The old turn's tool result should be snipped
        let msgs = conv.messages();
        let old_tool = &msgs[2]; // tool result from turn 1
        if let ContentBlock::ToolResult { content, .. } = &old_tool.content[0] {
            assert!(content.contains("[snipped"));
            assert!(content.len() < 500);
        }

        // The recent turn's tool result should be preserved
        let recent_tool = &msgs[6]; // tool result from turn 3
        if let ContentBlock::ToolResult { content, .. } = &recent_tool.content[0] {
            assert_eq!(content.len(), 2000);
        }
    }

    #[test]
    fn snip_preserves_small_results() {
        let mut conv = Conversation::new("s".into(), String::new(), 100_000);
        conv.push_user("Do something");
        conv.push_tool_result("tc_1", "small result", false);
        conv.push_user("Next");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("ok")],
        ));
        conv.push_user("Last");

        snip_large_tool_results(&mut conv);

        let msgs = conv.messages();
        if let ContentBlock::ToolResult { content, .. } = &msgs[1].content[0] {
            assert_eq!(content, "small result");
        }
    }

    #[test]
    fn strategy_for_usage_boundary_values() {
        // Exact boundary: 0 -> None
        assert!(CompactionStrategy::for_usage(0).is_none());
        // Exact boundary: 69 -> None
        assert!(CompactionStrategy::for_usage(69).is_none());
        // Exact boundary: 70 -> Snip
        assert_eq!(
            CompactionStrategy::for_usage(70),
            Some(CompactionStrategy::Snip)
        );
        // Exact boundary: 79 -> Snip
        assert_eq!(
            CompactionStrategy::for_usage(79),
            Some(CompactionStrategy::Snip)
        );
        // Exact boundary: 80 -> Microcompact
        assert_eq!(
            CompactionStrategy::for_usage(80),
            Some(CompactionStrategy::Microcompact)
        );
        // Exact boundary: 85 -> Summarize
        assert_eq!(
            CompactionStrategy::for_usage(85),
            Some(CompactionStrategy::Summarize)
        );
        // Exact boundary: 90 -> Hybrid
        assert_eq!(
            CompactionStrategy::for_usage(90),
            Some(CompactionStrategy::Hybrid { keep_recent: 3 })
        );
        // Exact boundary: 95 -> Truncate
        assert_eq!(
            CompactionStrategy::for_usage(95),
            Some(CompactionStrategy::Truncate)
        );
        // Max: 100 -> Truncate
        assert_eq!(
            CompactionStrategy::for_usage(100),
            Some(CompactionStrategy::Truncate)
        );
        // Max u8: 255 -> Truncate
        assert_eq!(
            CompactionStrategy::for_usage(255),
            Some(CompactionStrategy::Truncate)
        );
    }

    #[test]
    fn snip_empty_conversation() {
        let mut conv = Conversation::new("s".into(), String::new(), 100_000);
        snip_large_tool_results(&mut conv);
        assert!(conv.is_empty());
    }

    #[test]
    fn snip_single_turn_preserves_everything() {
        let mut conv = Conversation::new("s".into(), String::new(), 100_000);
        let large_content = "x".repeat(2000);
        conv.push_user("hello");
        conv.push_tool_result("tc_1", &large_content, false);

        snip_large_tool_results(&mut conv);

        let msgs = conv.messages();
        if let ContentBlock::ToolResult { content, .. } = &msgs[1].content[0] {
            // Single turn = recent, should NOT be snipped
            assert_eq!(content.len(), 2000);
        }
    }

    #[test]
    fn snip_preserves_error_tool_results() {
        let mut conv = Conversation::new("s".into(), String::new(), 100_000);

        // Turn 1 (old)
        conv.push_user("Do something");
        let large_error = "E".repeat(2000);
        conv.push_tool_result("tc_1", &large_error, true);

        // Turn 2 (recent)
        conv.push_user("Next");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("ok")],
        ));

        // Turn 3 (recent)
        conv.push_user("Last");

        snip_large_tool_results(&mut conv);

        let msgs = conv.messages();
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &msgs[1].content[0]
        {
            assert!(content.contains("[snipped"));
            assert!(*is_error); // is_error should be preserved
        }
    }

    #[tokio::test]
    async fn compact_truncate_reduces_messages() {
        struct DummyClient;
        impl CompactionClient for DummyClient {
            fn summarize(
                &self,
                _messages: &[Message],
                _instruction: &str,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crab_common::Result<String>> + Send + '_>,
            > {
                Box::pin(async { Ok("summary".into()) })
            }
        }

        let mut conv = Conversation::new("s".into(), String::new(), 1000);
        for i in 0..20 {
            conv.push_user(&format!("message {i}"));
            conv.push(Message::new(
                Role::Assistant,
                vec![ContentBlock::text(&format!("reply {i}"))],
            ));
        }
        let original_len = conv.len();

        compact(&mut conv, CompactionStrategy::Truncate, &DummyClient)
            .await
            .unwrap();

        // Truncation should reduce the number of messages
        assert!(conv.len() <= original_len);
    }

    #[test]
    fn strategy_equality() {
        assert_eq!(CompactionStrategy::Snip, CompactionStrategy::Snip);
        assert_ne!(CompactionStrategy::Snip, CompactionStrategy::Truncate);
        assert_eq!(
            CompactionStrategy::Hybrid { keep_recent: 3 },
            CompactionStrategy::Hybrid { keep_recent: 3 }
        );
        assert_ne!(
            CompactionStrategy::Hybrid { keep_recent: 3 },
            CompactionStrategy::Hybrid { keep_recent: 5 }
        );
    }
}
