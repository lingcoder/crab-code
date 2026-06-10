//! Suggestion ranking, dedup, and filtering before surfacing to the UI.

use serde::{Deserialize, Serialize};

/// The kind of action a suggestion recommends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    /// Run a shell command (e.g. tests, build).
    RunCommand(String),
    /// Read a specific file for context.
    ReadFile(String),
    /// Fix an error that was detected.
    FixError(String),
    /// General advice (no specific action).
    Advice,
}

/// A ranked suggestion for the user's next action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// Short title (e.g. "Run tests to verify fix").
    pub title: String,
    /// Longer description of why this action is suggested.
    pub description: String,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f64,
    /// The recommended action type.
    pub action_type: ActionType,
}

impl Suggestion {
    /// Create a new suggestion with the given fields.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        confidence: f64,
        action_type: ActionType,
    ) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            confidence: confidence.clamp(0.0, 1.0),
            action_type,
        }
    }
}

/// Deduplicate suggestions by title, keeping the one with highest confidence.
#[must_use]
pub fn deduplicate(mut suggestions: Vec<Suggestion>) -> Vec<Suggestion> {
    suggestions.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut seen = std::collections::HashSet::new();
    suggestions.retain(|s| seen.insert(s.title.clone()));
    suggestions
}

/// Filter out suggestions below a confidence threshold.
#[must_use]
pub fn filter_by_confidence(suggestions: Vec<Suggestion>, min_confidence: f64) -> Vec<Suggestion> {
    suggestions
        .into_iter()
        .filter(|s| s.confidence >= min_confidence)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_clamps_confidence() {
        let s = Suggestion::new("title", "desc", 1.5, ActionType::Advice);
        assert!((s.confidence - 1.0).abs() < f64::EPSILON);

        let s = Suggestion::new("title", "desc", -0.5, ActionType::Advice);
        assert!((s.confidence).abs() < f64::EPSILON);
    }

    #[test]
    fn deduplicate_keeps_highest_confidence() {
        let suggestions = vec![
            Suggestion::new("Run tests", "desc1", 0.5, ActionType::Advice),
            Suggestion::new("Run tests", "desc2", 0.9, ActionType::Advice),
            Suggestion::new("Build", "desc3", 0.7, ActionType::Advice),
        ];
        let deduped = deduplicate(suggestions);
        assert_eq!(deduped.len(), 2);
        // "Run tests" should have confidence 0.9 (the higher one).
        let run_tests = deduped.iter().find(|s| s.title == "Run tests").unwrap();
        assert!((run_tests.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn filter_by_confidence_removes_low() {
        let suggestions = vec![
            Suggestion::new("high", "desc", 0.8, ActionType::Advice),
            Suggestion::new("low", "desc", 0.2, ActionType::Advice),
        ];
        let filtered = filter_by_confidence(suggestions, 0.5);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].title, "high");
    }

    #[test]
    fn action_type_serde_roundtrip() {
        let actions = vec![
            ActionType::RunCommand("cargo test".into()),
            ActionType::ReadFile("src/main.rs".into()),
            ActionType::FixError("null pointer".into()),
            ActionType::Advice,
        ];
        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            let parsed: ActionType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, action);
        }
    }
}
