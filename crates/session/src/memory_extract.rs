//! Automatic memory extraction from conversations.
//!
//! Periodically scans recent conversation messages for information worth
//! persisting as memories (user preferences, corrections, project facts,
//! reference links). Extracted memories are proposed to the memory store
//! for deduplication and persistence.
//!
//! Maps to CCB `memdir/extractMemories.ts`.

use crab_core::message::{Message, Role};
use crab_memory::types::MemoryType;

/// Minimum new messages before triggering extraction.
const EXTRACTION_INTERVAL: usize = 10;

// ── Types ─────────────────────────────────────────────────────────────

/// Result of a memory extraction pass.
#[derive(Debug, Clone)]
pub struct ExtractionResult {
    /// Memories extracted from the conversation.
    pub memories: Vec<ExtractedMemory>,
}

/// A single memory extracted from conversation content.
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    /// Classification of this memory.
    pub memory_type: MemoryType,
    /// Short identifier / file-name slug for the memory.
    pub name: String,
    /// The markdown body content of the memory.
    pub content: String,
}

// ── Extraction ────────────────────────────────────────────────────────

/// Extract memories from a slice of conversation messages.
///
/// Scans messages for patterns like user corrections ("actually, I prefer X"),
/// reference links, project configuration hints, and tool usage patterns.
/// This is a heuristic, keyword-based extraction pass — not LLM-based.
///
/// In CCB, this is done via a forked agent with full LLM capability. Our
/// implementation uses pattern matching as a lightweight local fallback.
pub fn extract_memories_from_conversation(messages: &[Message]) -> ExtractionResult {
    let mut memories = Vec::new();

    for msg in messages {
        if msg.role != Role::User {
            continue;
        }

        let text = msg.text();
        if text.is_empty() {
            continue;
        }

        // Feedback patterns: corrections and preferences
        if let Some(mem) = extract_feedback(&text) {
            memories.push(mem);
        }

        // Reference patterns: URLs and external system pointers
        if let Some(mem) = extract_reference(&text) {
            memories.push(mem);
        }

        // Project patterns: deadlines, decisions, context
        if let Some(mem) = extract_project_fact(&text) {
            memories.push(mem);
        }
    }

    ExtractionResult { memories }
}

/// Determine whether extraction should be attempted based on message count
/// and how recently the last extraction was run.
///
/// Returns `true` if enough new messages have accumulated to justify a scan.
#[must_use]
pub fn should_extract(message_count: usize, last_extraction_turn: usize) -> bool {
    message_count > last_extraction_turn
        && (message_count - last_extraction_turn) >= EXTRACTION_INTERVAL
}

// ── Pattern matchers ──────────────────────────────────────────────────

/// Detect user corrections and preferences.
fn extract_feedback(text: &str) -> Option<ExtractedMemory> {
    let lower = text.to_lowercase();

    // Look for correction patterns
    let correction_markers = [
        "don't ",
        "do not ",
        "stop ",
        "never ",
        "always ",
        "prefer ",
        "i prefer ",
        "actually, ",
        "no, ",
        "please don't",
        "instead of ",
    ];

    for marker in correction_markers {
        if lower.contains(marker) && text.len() < 500 {
            let slug = slugify_first_words(text, 4);
            return Some(ExtractedMemory {
                memory_type: MemoryType::Feedback,
                name: format!("feedback_{slug}"),
                content: text.to_string(),
            });
        }
    }

    None
}

/// Detect external system references (URLs, links).
fn extract_reference(text: &str) -> Option<ExtractedMemory> {
    // Look for URL patterns
    if (text.contains("http://") || text.contains("https://"))
        && text.len() < 500
        && (text.contains("check ") || text.contains("look at ") || text.contains("see "))
    {
        let slug = slugify_first_words(text, 4);
        return Some(ExtractedMemory {
            memory_type: MemoryType::Reference,
            name: format!("reference_{slug}"),
            content: text.to_string(),
        });
    }

    None
}

/// Detect project facts (deadlines, decisions, context).
fn extract_project_fact(text: &str) -> Option<ExtractedMemory> {
    let lower = text.to_lowercase();

    let project_markers = [
        "deadline",
        "freeze",
        "release",
        "merge ",
        "the reason ",
        "because ",
        "we decided ",
        "we're ",
        "we are ",
    ];

    for marker in project_markers {
        if lower.contains(marker) && text.len() < 500 && text.len() > 20 {
            let slug = slugify_first_words(text, 4);
            return Some(ExtractedMemory {
                memory_type: MemoryType::Project,
                name: format!("project_{slug}"),
                content: text.to_string(),
            });
        }
    }

    None
}

/// Create a slug from the first N words of text.
fn slugify_first_words(text: &str, n: usize) -> String {
    text.split_whitespace()
        .take(n)
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .collect::<Vec<_>>()
        .join("_")
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_result_fields() {
        let result = ExtractionResult {
            memories: vec![ExtractedMemory {
                memory_type: MemoryType::Feedback,
                name: "prefer_tabs".into(),
                content: "User prefers tabs over spaces".into(),
            }],
        };
        assert_eq!(result.memories.len(), 1);
        assert_eq!(result.memories[0].name, "prefer_tabs");
    }

    #[test]
    fn extracted_memory_debug() {
        let mem = ExtractedMemory {
            memory_type: MemoryType::User,
            name: "test".into(),
            content: "test content".into(),
        };
        let debug = format!("{mem:?}");
        assert!(debug.contains("test"));
    }

    #[test]
    fn should_extract_enough_messages() {
        assert!(should_extract(15, 0));
        assert!(should_extract(20, 10));
    }

    #[test]
    fn should_not_extract_too_few_messages() {
        assert!(!should_extract(5, 0));
        assert!(!should_extract(12, 5));
    }

    #[test]
    fn should_not_extract_already_extracted() {
        assert!(!should_extract(10, 10));
    }

    #[test]
    fn extract_feedback_correction() {
        let text = "don't use semicolons in JavaScript";
        let mem = extract_feedback(text).unwrap();
        assert_eq!(mem.memory_type, MemoryType::Feedback);
        assert!(mem.name.starts_with("feedback_"));
    }

    #[test]
    fn extract_feedback_preference() {
        let text = "I prefer using async/await over callbacks";
        let mem = extract_feedback(text).unwrap();
        assert_eq!(mem.memory_type, MemoryType::Feedback);
    }

    #[test]
    fn extract_feedback_no_match() {
        let text = "How does the authentication work?";
        assert!(extract_feedback(text).is_none());
    }

    #[test]
    fn extract_reference_with_url() {
        let text = "check https://grafana.internal/dashboard for the metrics";
        let mem = extract_reference(text).unwrap();
        assert_eq!(mem.memory_type, MemoryType::Reference);
    }

    #[test]
    fn extract_reference_no_url() {
        let text = "the dashboard is useful";
        assert!(extract_reference(text).is_none());
    }

    #[test]
    fn extract_project_deadline() {
        let text = "the deadline for the release is next Friday";
        let mem = extract_project_fact(text).unwrap();
        assert_eq!(mem.memory_type, MemoryType::Project);
    }

    #[test]
    fn slugify_creates_slug() {
        assert_eq!(
            slugify_first_words("Hello World! 123", 3),
            "hello_world_123"
        );
    }

    #[test]
    fn extract_from_empty_conversation() {
        let result = extract_memories_from_conversation(&[]);
        assert!(result.memories.is_empty());
    }
}
