use serde::{Deserialize, Serialize};

/// When a hook fires.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookTrigger {
    /// Before a tool is invoked.
    PreToolUse,
    /// After a tool completes.
    PostToolUse,
    /// When user submits a prompt.
    PromptSubmit,
    /// When a notification is sent.
    Notification,
}

/// A single hook definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    /// When this hook fires.
    pub trigger: HookTrigger,
    /// Optional tool name matcher (glob pattern). Only for Pre/PostToolUse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Shell command to execute.
    pub command: String,
    /// Timeout in milliseconds. Defaults to 60000 (60s).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    60_000
}

/// Parse hooks from the `hooks` field of settings (a JSON value).
pub fn parse_hooks(value: &serde_json::Value) -> crab_common::Result<Vec<Hook>> {
    let hooks: Vec<Hook> = serde_json::from_value(value.clone())
        .map_err(|e| crab_common::Error::Config(format!("hooks parse error: {e}")))?;
    Ok(hooks)
}

/// Load hooks from a `Settings` struct.
pub fn load_hooks(settings: &crate::Settings) -> crab_common::Result<Vec<Hook>> {
    settings
        .hooks
        .as_ref()
        .map_or_else(|| Ok(Vec::new()), parse_hooks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_trigger_serde_roundtrip() {
        let trigger = HookTrigger::PreToolUse;
        let json = serde_json::to_string(&trigger).unwrap();
        assert_eq!(json, r#""pre_tool_use""#);
        let back: HookTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(back, trigger);
    }

    #[test]
    fn hook_trigger_all_variants() {
        let triggers = [
            (HookTrigger::PreToolUse, "pre_tool_use"),
            (HookTrigger::PostToolUse, "post_tool_use"),
            (HookTrigger::PromptSubmit, "prompt_submit"),
            (HookTrigger::Notification, "notification"),
        ];
        for (trigger, expected) in triggers {
            let json = serde_json::to_string(&trigger).unwrap();
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    #[test]
    fn parse_hook_definition() {
        let json = serde_json::json!([{
            "trigger": "pre_tool_use",
            "toolName": "bash",
            "command": "echo checking",
            "timeoutMs": 5000
        }]);
        let hooks = parse_hooks(&json).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].trigger, HookTrigger::PreToolUse);
        assert_eq!(hooks[0].tool_name.as_deref(), Some("bash"));
        assert_eq!(hooks[0].command, "echo checking");
        assert_eq!(hooks[0].timeout_ms, 5000);
    }

    #[test]
    fn parse_hook_default_timeout() {
        let json = serde_json::json!([{
            "trigger": "post_tool_use",
            "command": "echo done"
        }]);
        let hooks = parse_hooks(&json).unwrap();
        assert_eq!(hooks[0].timeout_ms, 60_000);
    }

    #[test]
    fn parse_empty_hooks() {
        let json = serde_json::json!([]);
        let hooks = parse_hooks(&json).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn load_hooks_from_settings_none() {
        let settings = crate::Settings::default();
        let hooks = load_hooks(&settings).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn load_hooks_from_settings_with_hooks() {
        let settings = crate::Settings {
            hooks: Some(serde_json::json!([{
                "trigger": "prompt_submit",
                "command": "echo hi"
            }])),
            ..Default::default()
        };
        let hooks = load_hooks(&settings).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].trigger, HookTrigger::PromptSubmit);
    }

    #[test]
    fn parse_invalid_hooks_returns_error() {
        let json = serde_json::json!({"not": "an array"});
        let result = parse_hooks(&json);
        assert!(result.is_err());
    }
}
