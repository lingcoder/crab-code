use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crab_core::hook::HookTrigger;

/// Default per-hook timeout in seconds, matching the CC hooks protocol.
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 600;

/// A single hook definition, flattened from the CC-shaped config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// When this hook fires.
    pub trigger: HookTrigger,
    /// Matcher pattern from the enclosing group. `None` / empty / `"*"`
    /// matches everything; otherwise exact name, `|`-separated names, or a
    /// regex. Only meaningful for tool events.
    pub matcher: Option<String>,
    /// Shell command to execute.
    pub command: String,
    /// Timeout in seconds.
    pub timeout_secs: u64,
}

/// One matcher group in the CC hooks config: a matcher pattern plus the
/// hooks that run when it matches.
#[derive(Debug, Clone, Deserialize)]
struct MatcherGroup {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<HookEntry>,
}

/// One hook entry inside a matcher group.
#[derive(Debug, Clone, Deserialize)]
struct HookEntry {
    /// Hook execution type. Only `"command"` is supported; other values
    /// are skipped with a warning.
    #[serde(default = "default_hook_type", rename = "type")]
    hook_type: String,
    #[serde(default)]
    command: String,
    /// Timeout in seconds (CC protocol semantics).
    #[serde(default)]
    timeout: Option<u64>,
}

fn default_hook_type() -> String {
    "command".to_string()
}

/// Parse hooks from the `hooks` field of settings.
///
/// Expects the CC hooks shape — a map keyed by event name:
///
/// ```json
/// {
///   "PreToolUse": [
///     {
///       "matcher": "Edit|Write",
///       "hooks": [{ "type": "command", "command": "...", "timeout": 600 }]
///     }
///   ]
/// }
/// ```
pub fn parse_hooks(value: &serde_json::Value) -> crab_core::Result<Vec<Hook>> {
    let groups: BTreeMap<HookTrigger, Vec<MatcherGroup>> = serde_json::from_value(value.clone())
        .map_err(|e| crab_core::Error::Config(format!("hooks parse error: {e}")))?;

    let mut hooks = Vec::new();
    for (trigger, matcher_groups) in groups {
        for group in matcher_groups {
            for entry in group.hooks {
                if entry.hook_type != "command" {
                    tracing::warn!(
                        hook_type = entry.hook_type.as_str(),
                        event = trigger.event_name(),
                        "skipping unsupported hook type (only \"command\" is supported)"
                    );
                    continue;
                }
                if entry.command.is_empty() {
                    tracing::warn!(
                        event = trigger.event_name(),
                        "skipping hook with empty command"
                    );
                    continue;
                }
                hooks.push(Hook {
                    trigger,
                    matcher: group.matcher.clone(),
                    command: entry.command,
                    timeout_secs: entry.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS),
                });
            }
        }
    }
    Ok(hooks)
}

/// Load hooks from a `Config` struct.
pub fn load_hooks(config: &crate::Config) -> crab_core::Result<Vec<Hook>> {
    config
        .hooks
        .as_ref()
        .map_or_else(|| Ok(Vec::new()), parse_hooks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cc_shaped_hooks() {
        let json = serde_json::json!({
            "PreToolUse": [
                {
                    "matcher": "Edit|Write",
                    "hooks": [
                        { "type": "command", "command": "check.sh", "timeout": 30 }
                    ]
                }
            ],
            "PostToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [
                        { "type": "command", "command": "log.sh" }
                    ]
                }
            ]
        });
        let mut hooks = parse_hooks(&json).unwrap();
        hooks.sort_by_key(|h| h.command.clone());
        assert_eq!(hooks.len(), 2);

        assert_eq!(hooks[0].trigger, HookTrigger::PreToolUse);
        assert_eq!(hooks[0].matcher.as_deref(), Some("Edit|Write"));
        assert_eq!(hooks[0].command, "check.sh");
        assert_eq!(hooks[0].timeout_secs, 30);

        assert_eq!(hooks[1].trigger, HookTrigger::PostToolUse);
        assert_eq!(hooks[1].timeout_secs, DEFAULT_HOOK_TIMEOUT_SECS);
    }

    #[test]
    fn parse_hooks_without_matcher() {
        let json = serde_json::json!({
            "SessionStart": [
                { "hooks": [{ "type": "command", "command": "init.sh" }] }
            ]
        });
        let hooks = parse_hooks(&json).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].trigger, HookTrigger::SessionStart);
        assert!(hooks[0].matcher.is_none());
    }

    #[test]
    fn parse_hooks_multiple_hooks_per_group() {
        let json = serde_json::json!({
            "Stop": [
                {
                    "hooks": [
                        { "type": "command", "command": "a.sh" },
                        { "type": "command", "command": "b.sh" }
                    ]
                }
            ]
        });
        let hooks = parse_hooks(&json).unwrap();
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn parse_hooks_skips_unsupported_type() {
        let json = serde_json::json!({
            "PreToolUse": [
                {
                    "hooks": [
                        { "type": "prompt", "command": "ignored" },
                        { "type": "command", "command": "kept.sh" }
                    ]
                }
            ]
        });
        let hooks = parse_hooks(&json).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].command, "kept.sh");
    }

    #[test]
    fn parse_hooks_default_type_is_command() {
        let json = serde_json::json!({
            "Notification": [
                { "hooks": [{ "command": "notify.sh" }] }
            ]
        });
        let hooks = parse_hooks(&json).unwrap();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn parse_empty_hooks() {
        let json = serde_json::json!({});
        let hooks = parse_hooks(&json).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn parse_invalid_hooks_returns_error() {
        let json = serde_json::json!(["not", "a", "map"]);
        assert!(parse_hooks(&json).is_err());
    }

    #[test]
    fn parse_unknown_event_name_returns_error() {
        let json = serde_json::json!({
            "NoSuchEvent": [{ "hooks": [{ "command": "x" }] }]
        });
        assert!(parse_hooks(&json).is_err());
    }

    #[test]
    fn load_hooks_from_settings_none() {
        let settings = crate::Config::default();
        let hooks = load_hooks(&settings).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn load_hooks_from_settings_with_hooks() {
        let settings = crate::Config {
            hooks: Some(serde_json::json!({
                "UserPromptSubmit": [
                    { "hooks": [{ "type": "command", "command": "echo hi" }] }
                ]
            })),
            ..Default::default()
        };
        let hooks = load_hooks(&settings).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].trigger, HookTrigger::UserPromptSubmit);
    }
}
