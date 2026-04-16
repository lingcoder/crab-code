//! Proactive suggestion types — produced by `crab-agent::proactive`,
//! displayed by `crab-tui`.

use serde::{Deserialize, Serialize};

/// Category of a proactive suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionKind {
    /// Completion for the current input draft.
    Autocomplete,
    /// Suggested next step / follow-up action.
    NextStep,
    /// Warning about a potential issue.
    Warning,
}

/// A single suggestion surfaced to the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProactiveSuggestion {
    pub kind: SuggestionKind,
    pub text: String,
    /// Relevance score in [0.0, 1.0]; higher is more relevant.
    pub score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serde_roundtrip() {
        let k = SuggestionKind::NextStep;
        let json = serde_json::to_string(&k).unwrap();
        let back: SuggestionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(k, back);
    }

    #[test]
    fn suggestion_serde_roundtrip() {
        let s = ProactiveSuggestion {
            kind: SuggestionKind::Warning,
            text: "unsaved changes in neighboring file".into(),
            score: 0.8,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ProactiveSuggestion = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
