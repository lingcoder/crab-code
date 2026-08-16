//! [`SandboxMode`] — the user-facing on/off knob for sandboxing.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Whether command sandboxing is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// Enable sandboxing when the platform supports it (the default).
    #[default]
    Auto,
    /// Disable sandboxing entirely.
    Off,
}

impl SandboxMode {
    /// Whether this mode enables sandboxing.
    #[must_use]
    pub const fn enabled(self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl FromStr for SandboxMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" | "true" => Ok(Self::Auto),
            "off" | "false" | "none" | "disabled" => Ok(Self::Off),
            other => Err(format!("invalid sandbox mode: {other} (expected auto|off)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modes() {
        assert_eq!("auto".parse::<SandboxMode>().unwrap(), SandboxMode::Auto);
        assert_eq!("off".parse::<SandboxMode>().unwrap(), SandboxMode::Off);
        assert!("bogus".parse::<SandboxMode>().is_err());
    }

    #[test]
    fn default_is_auto_enabled() {
        assert_eq!(SandboxMode::default(), SandboxMode::Auto);
        assert!(SandboxMode::Auto.enabled());
        assert!(!SandboxMode::Off.enabled());
    }
}
