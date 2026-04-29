use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(pub String);

impl ModelId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for ModelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_creation_tokens == 0
    }
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cache_read_tokens += rhs.cache_read_tokens;
        self.cache_creation_tokens += rhs.cache_creation_tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_display() {
        let id = ModelId::from("claude-sonnet-4-20250514");
        assert_eq!(id.to_string(), "claude-sonnet-4-20250514");
        assert_eq!(id.as_str(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn model_id_from_string() {
        let id = ModelId::from("gpt-4o".to_string());
        assert_eq!(id.0, "gpt-4o");
    }

    #[test]
    fn token_usage_total() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 20,
            cache_creation_tokens: 10,
        };
        assert_eq!(usage.total(), 150);
        assert!(!usage.is_empty());
    }

    #[test]
    fn token_usage_is_empty() {
        let usage = TokenUsage::default();
        assert!(usage.is_empty());
        assert_eq!(usage.total(), 0);
    }

    #[test]
    fn token_usage_add_assign() {
        let mut a = TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 5,
            cache_creation_tokens: 3,
        };
        let b = TokenUsage {
            input_tokens: 5,
            output_tokens: 10,
            cache_read_tokens: 2,
            cache_creation_tokens: 1,
        };
        a += b;
        assert_eq!(a.input_tokens, 15);
        assert_eq!(a.output_tokens, 30);
        assert_eq!(a.cache_read_tokens, 7);
        assert_eq!(a.cache_creation_tokens, 4);
    }

    #[test]
    fn token_usage_serde_roundtrip() {
        let usage = TokenUsage {
            input_tokens: 42,
            output_tokens: 13,
            cache_read_tokens: 7,
            cache_creation_tokens: 3,
        };
        let json = serde_json::to_string(&usage).unwrap();
        let parsed: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(usage, parsed);
    }

    #[test]
    fn model_id_serde_roundtrip() {
        let id = ModelId::from("claude-opus-4-20250514");
        let json = serde_json::to_string(&id).unwrap();
        let parsed: ModelId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    // ─── Additional coverage tests ───

    #[test]
    fn model_id_equality() {
        let a = ModelId::from("claude-sonnet-4-6");
        let b = ModelId::from("claude-sonnet-4-6");
        let c = ModelId::from("gpt-4o");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn model_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ModelId::from("model-a"));
        set.insert(ModelId::from("model-b"));
        set.insert(ModelId::from("model-a")); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn token_usage_add_assign_to_default() {
        let mut usage = TokenUsage::default();
        usage += TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
        assert!(!usage.is_empty());
    }

    #[test]
    fn token_usage_serde_defaults_to_zero() {
        let json = r#"{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_creation_tokens":0}"#;
        let usage: TokenUsage = serde_json::from_str(json).unwrap();
        assert!(usage.is_empty());
    }
}
