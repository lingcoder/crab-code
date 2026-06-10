/// Resolve well-known model aliases to their full model IDs.
/// Unknown strings are returned unchanged.
pub fn resolve_model_alias(model: &str) -> String {
    match model {
        "sonnet" => "claude-sonnet-4-6".to_string(),
        "opus" => "claude-opus-4-6".to_string(),
        "haiku" => "claude-haiku-4-5-20251001".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_alias_known() {
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
    }

    #[test]
    fn resolve_model_alias_passthrough() {
        assert_eq!(resolve_model_alias("gpt-4o"), "gpt-4o");
        assert_eq!(
            resolve_model_alias("claude-sonnet-4-20250514"),
            "claude-sonnet-4-20250514"
        );
    }
}
