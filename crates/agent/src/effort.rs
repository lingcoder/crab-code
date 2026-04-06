//! Effort level system: maps user-facing effort levels to API parameters.

use std::fmt;
use std::str::FromStr;

/// Effort level controlling reasoning depth and token budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    /// Minimal reasoning, fast responses.
    Low,
    /// Moderate reasoning depth.
    Medium,
    /// Deep reasoning.
    High,
    /// Maximum reasoning depth, highest token budget.
    Max,
}

impl EffortLevel {
    /// Map effort level to extended thinking `budget_tokens`.
    ///
    /// - `Low`: `None` (thinking disabled)
    /// - `Medium`: 5,000 tokens
    /// - `High`: 10,000 tokens
    /// - `Max`: 50,000 tokens
    #[must_use]
    pub fn to_budget_tokens(self) -> Option<u32> {
        match self {
            Self::Low => None,
            Self::Medium => Some(5_000),
            Self::High => Some(10_000),
            Self::Max => Some(50_000),
        }
    }
}

impl fmt::Display for EffortLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => f.write_str("low"),
            Self::Medium => f.write_str("medium"),
            Self::High => f.write_str("high"),
            Self::Max => f.write_str("max"),
        }
    }
}

impl FromStr for EffortLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" | "med" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "max" => Ok(Self::Max),
            other => Err(format!(
                "unknown effort level: '{other}'. Valid: low, medium, high, max"
            )),
        }
    }
}

/// Thinking mode for extended reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Always enable extended thinking.
    Enabled,
    /// Enable thinking adaptively based on query complexity.
    Adaptive,
    /// Disable extended thinking entirely.
    Disabled,
}

impl fmt::Display for ThinkingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled => f.write_str("enabled"),
            Self::Adaptive => f.write_str("adaptive"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}

impl FromStr for ThinkingMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "enabled" | "on" | "true" => Ok(Self::Enabled),
            "adaptive" | "auto" => Ok(Self::Adaptive),
            "disabled" | "off" | "false" => Ok(Self::Disabled),
            other => Err(format!(
                "unknown thinking mode: '{other}'. Valid: enabled, adaptive, disabled"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_level_from_str() {
        assert_eq!("low".parse::<EffortLevel>().unwrap(), EffortLevel::Low);
        assert_eq!(
            "medium".parse::<EffortLevel>().unwrap(),
            EffortLevel::Medium
        );
        assert_eq!("med".parse::<EffortLevel>().unwrap(), EffortLevel::Medium);
        assert_eq!("high".parse::<EffortLevel>().unwrap(), EffortLevel::High);
        assert_eq!("max".parse::<EffortLevel>().unwrap(), EffortLevel::Max);
        assert_eq!("LOW".parse::<EffortLevel>().unwrap(), EffortLevel::Low);
        assert!("unknown".parse::<EffortLevel>().is_err());
    }

    #[test]
    fn effort_level_display() {
        assert_eq!(EffortLevel::Low.to_string(), "low");
        assert_eq!(EffortLevel::Medium.to_string(), "medium");
        assert_eq!(EffortLevel::High.to_string(), "high");
        assert_eq!(EffortLevel::Max.to_string(), "max");
    }

    #[test]
    fn effort_to_budget_tokens_mapping() {
        assert_eq!(EffortLevel::Low.to_budget_tokens(), None);
        assert_eq!(EffortLevel::Medium.to_budget_tokens(), Some(5_000));
        assert_eq!(EffortLevel::High.to_budget_tokens(), Some(10_000));
        assert_eq!(EffortLevel::Max.to_budget_tokens(), Some(50_000));
    }

    #[test]
    fn thinking_mode_from_str() {
        assert_eq!(
            "enabled".parse::<ThinkingMode>().unwrap(),
            ThinkingMode::Enabled
        );
        assert_eq!("on".parse::<ThinkingMode>().unwrap(), ThinkingMode::Enabled);
        assert_eq!(
            "adaptive".parse::<ThinkingMode>().unwrap(),
            ThinkingMode::Adaptive
        );
        assert_eq!(
            "auto".parse::<ThinkingMode>().unwrap(),
            ThinkingMode::Adaptive
        );
        assert_eq!(
            "disabled".parse::<ThinkingMode>().unwrap(),
            ThinkingMode::Disabled
        );
        assert_eq!(
            "off".parse::<ThinkingMode>().unwrap(),
            ThinkingMode::Disabled
        );
        assert!("bogus".parse::<ThinkingMode>().is_err());
    }

    #[test]
    fn thinking_mode_display() {
        assert_eq!(ThinkingMode::Enabled.to_string(), "enabled");
        assert_eq!(ThinkingMode::Adaptive.to_string(), "adaptive");
        assert_eq!(ThinkingMode::Disabled.to_string(), "disabled");
    }
}
