//! Selectively replace large tool outputs with "[snipped]" markers.
//!
//! Unlike full compaction (which summarizes or truncates the entire conversation),
//! snip compaction targets individual tool results that exceed a character threshold
//! and replaces them with a short marker. This is a fast, non-LLM operation that
//! reduces context usage without altering conversation structure.
//!
//! # Relationship to `micro_compact.rs`
//!
//! `micro_compact` replaces large tool results with LLM-generated summaries.
//! `snip_compact` is a cheaper alternative that simply truncates the output
//! and inserts a "[snipped]" marker — no LLM call required.

use crab_core::message::{ContentBlock, Message};

// ─── Configuration ─────────────────────────────────────────────────────

/// Configuration for snip compaction.
#[derive(Debug, Clone)]
pub struct SnipConfig {
    /// Maximum character count for a single tool result before it is snipped.
    pub max_result_chars: usize,
    /// Marker string inserted in place of the snipped content.
    /// `{n}` is replaced with the original character count.
    pub snip_marker: String,
}

impl Default for SnipConfig {
    fn default() -> Self {
        Self {
            max_result_chars: 10_000,
            snip_marker: "[output snipped — was {n} chars]".into(),
        }
    }
}

impl SnipConfig {
    /// Format the snip marker, replacing `{n}` with the actual character count.
    #[must_use]
    pub fn format_marker(&self, original_len: usize) -> String {
        self.snip_marker.replace("{n}", &original_len.to_string())
    }
}

// ─── Snip function ─────────────────────────────────────────────────────

/// Scan messages and replace tool results exceeding `config.max_result_chars`
/// with a snip marker.
///
/// Returns the number of tool results that were snipped.
pub fn snip_large_outputs(messages: &mut [Message], config: &SnipConfig) -> usize {
    let mut snipped_count = 0;

    for message in messages.iter_mut() {
        for block in &mut message.content {
            if let ContentBlock::ToolResult {
                content, is_error, ..
            } = block
            {
                // Never snip error results — they're important for debugging
                if *is_error {
                    continue;
                }

                if content.len() > config.max_result_chars {
                    let marker = config.format_marker(content.len());
                    *content = marker;
                    snipped_count += 1;
                }
            }
        }
    }

    snipped_count
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Role;

    #[test]
    fn default_config_values() {
        let config = SnipConfig::default();
        assert_eq!(config.max_result_chars, 10_000);
        assert!(config.snip_marker.contains("{n}"));
    }

    #[test]
    fn format_marker_replaces_n() {
        let config = SnipConfig::default();
        let marker = config.format_marker(25_000);
        assert!(marker.contains("25000"));
        assert!(!marker.contains("{n}"));
    }

    #[test]
    fn format_marker_custom() {
        let config = SnipConfig {
            max_result_chars: 5_000,
            snip_marker: "SNIPPED({n})".into(),
        };
        assert_eq!(config.format_marker(12_345), "SNIPPED(12345)");
    }

    #[test]
    fn snip_large_tool_result() {
        let config = SnipConfig {
            max_result_chars: 10,
            snip_marker: "[snipped {n}]".into(),
        };

        let large_content = "a".repeat(20);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result("id1", large_content, false)],
        }];

        let count = snip_large_outputs(&mut messages, &config);
        assert_eq!(count, 1);

        if let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] {
            assert!(content.contains("snipped"));
            assert!(content.contains("20"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn does_not_snip_small_result() {
        let config = SnipConfig {
            max_result_chars: 100,
            snip_marker: "[snipped {n}]".into(),
        };

        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result("id1", "short output", false)],
        }];

        let count = snip_large_outputs(&mut messages, &config);
        assert_eq!(count, 0);

        if let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] {
            assert_eq!(content, "short output");
        }
    }

    #[test]
    fn does_not_snip_error_results() {
        let config = SnipConfig {
            max_result_chars: 5,
            snip_marker: "[snipped {n}]".into(),
        };

        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result(
                "id1",
                "this is a long error message that should be preserved",
                true,
            )],
        }];

        let count = snip_large_outputs(&mut messages, &config);
        assert_eq!(count, 0);
    }

    #[test]
    fn snip_multiple_results() {
        let config = SnipConfig {
            max_result_chars: 5,
            snip_marker: "[snipped {n}]".into(),
        };

        let mut messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::tool_result("id1", "long output here", false)],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::tool_result("id2", "ok", false)],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::tool_result(
                    "id3",
                    "another long output",
                    false,
                )],
            },
        ];

        let count = snip_large_outputs(&mut messages, &config);
        assert_eq!(count, 2); // id1 and id3 snipped, id2 preserved
    }

    #[test]
    fn snip_skips_text_blocks() {
        let config = SnipConfig {
            max_result_chars: 5,
            snip_marker: "[snipped {n}]".into(),
        };

        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::text(
                "this is a long text block that should not be snipped",
            )],
        }];

        let count = snip_large_outputs(&mut messages, &config);
        assert_eq!(count, 0);
    }

    #[test]
    fn empty_messages_returns_zero() {
        let config = SnipConfig::default();
        let mut messages: Vec<Message> = vec![];
        assert_eq!(snip_large_outputs(&mut messages, &config), 0);
    }
}
