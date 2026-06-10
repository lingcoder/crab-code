/// Validate an `--effort` value.
///
/// Accepts: `low`, `medium`, `high`, `max` (case-insensitive).
pub fn validate_effort(effort: &str) -> Result<(), String> {
    match effort.to_lowercase().as_str() {
        "low" | "medium" | "high" | "max" => Ok(()),
        other => Err(format!(
            "invalid --effort value: '{other}'. Valid: low, medium, high, max"
        )),
    }
}

/// Validate a `--thinking` value.
///
/// Accepts: `enabled`, `adaptive`, `disabled` (case-insensitive).
pub fn validate_thinking(thinking: &str) -> Result<(), String> {
    match thinking.to_lowercase().as_str() {
        "enabled" | "adaptive" | "disabled" => Ok(()),
        other => Err(format!(
            "invalid --thinking value: '{other}'. Valid: enabled, adaptive, disabled"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_effort_valid() {
        for v in ["low", "medium", "high", "max", "LOW", "High"] {
            assert!(validate_effort(v).is_ok(), "should accept {v:?}");
        }
    }

    #[test]
    fn validate_effort_invalid() {
        assert!(validate_effort("bogus").is_err());
        assert!(validate_effort("").is_err());
    }

    #[test]
    fn validate_thinking_valid() {
        for v in ["enabled", "adaptive", "disabled", "ENABLED", "Adaptive"] {
            assert!(validate_thinking(v).is_ok(), "should accept {v:?}");
        }
    }

    #[test]
    fn validate_thinking_invalid() {
        assert!(validate_thinking("bogus").is_err());
        assert!(validate_thinking("").is_err());
    }
}
