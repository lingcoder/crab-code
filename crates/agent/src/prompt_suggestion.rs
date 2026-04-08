//! Suggests follow-up prompts based on conversation context.
//!
//! After each assistant turn, this module analyzes the conversation summary
//! and the last tool used to generate a small set of contextual suggestions
//! that help the user continue productively. Suggestions are displayed in
//! the TUI input area as ghost text or a dropdown.
//!
//! Maps to CCB `suggestions/generateSuggestions.ts`.

use serde::{Deserialize, Serialize};

// ── Types ─────────────────────────────────────────────────────────────

/// A single suggested follow-up prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSuggestion {
    /// The full text of the suggested prompt.
    pub text: String,
    /// Classification of the suggestion's intent.
    pub category: SuggestionCategory,
}

/// Category of a prompt suggestion — helps the UI group and style them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionCategory {
    /// Continue the current line of work.
    FollowUp,
    /// Ask a clarifying question about the result.
    Clarification,
    /// Suggest an alternative approach.
    Alternative,
    /// Explore a related topic.
    Related,
}

// ── Generation ────────────────────────────────────────────────────────

/// Generate a list of follow-up prompt suggestions.
///
/// # Arguments
///
/// * `conversation_summary` — A compact summary of the conversation so far
///   (produced by the summarizer).
/// * `last_tool_used` — The name of the most recently executed tool, if any.
///   Certain tools (e.g. `Edit`, `Bash`) have domain-specific follow-ups.
///
/// # Returns
///
/// Up to 4 suggestions ordered by estimated relevance (best first).
pub fn suggest_prompts(
    _conversation_summary: &str,
    _last_tool_used: Option<&str>,
) -> Vec<PromptSuggestion> {
    // Placeholder: real implementation will analyze the conversation
    // context and generate relevant follow-up suggestions via the LLM.
    Vec::new()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_category_serde_roundtrip() {
        let cat = SuggestionCategory::FollowUp;
        let json = serde_json::to_string(&cat).unwrap();
        assert_eq!(json, "\"follow_up\"");
        let parsed: SuggestionCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, cat);
    }

    #[test]
    fn prompt_suggestion_fields() {
        let suggestion = PromptSuggestion {
            text: "Run the tests to verify".into(),
            category: SuggestionCategory::FollowUp,
        };
        assert!(!suggestion.text.is_empty());
        assert_eq!(suggestion.category, SuggestionCategory::FollowUp);
    }

    #[test]
    fn all_categories_serialize() {
        let cats = [
            SuggestionCategory::FollowUp,
            SuggestionCategory::Clarification,
            SuggestionCategory::Alternative,
            SuggestionCategory::Related,
        ];
        for cat in &cats {
            let json = serde_json::to_string(cat).unwrap();
            let parsed: SuggestionCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, cat);
        }
    }
}
