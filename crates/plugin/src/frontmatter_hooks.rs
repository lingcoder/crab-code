//! Parse hook definitions from skill YAML frontmatter.
//!
//! Skills can declare hooks in their YAML frontmatter that should be registered
//! when the skill is loaded. This module extracts those hook definitions and
//! registers them with the [`HookRegistry`](super::hook_registry::HookRegistry).
//!
//! Maps to CCB `hooks/registerFrontmatterHooks.ts` + `hooks/registerSkillHooks.ts`.

use super::hook_registry::{HookEventType, HookRegistry, HookSource, RegisteredHook};
use super::hook_types::{CommandHook, HookType, PromptHook};

// ─── Frontmatter hook definition ───────────────────────────────────────

/// A hook definition parsed from a skill file's YAML frontmatter.
struct FrontmatterHookDef {
    /// The event this hook responds to (e.g. `pre_tool_use`, `session_start`).
    event: String,
    /// Shell command to execute (mutually exclusive with `prompt`).
    command: Option<String>,
    /// Prompt template to pass through the LLM (mutually exclusive with `command`).
    prompt: Option<String>,
}

// ─── Registration ──────────────────────────────────────────────────────

/// Extract and register hooks from a skill file's YAML frontmatter.
///
/// Parses the `hooks` section from the frontmatter YAML and registers each
/// hook definition with the provided [`HookRegistry`]. Returns the IDs of
/// all successfully registered hooks.
///
/// # Expected frontmatter format
///
/// ```yaml
/// hooks:
///   - event: pre_tool_use
///     command: echo "before tool"
///   - event: session_start
///     prompt: "Initialize the session context for {{tool_name}}"
/// ```
pub async fn register_frontmatter_hooks(
    registry: &HookRegistry,
    skill_name: &str,
    frontmatter: &str,
) -> Vec<String> {
    // Parse the frontmatter as simple key-value YAML, then extract hooks
    let yaml_value = parse_simple_yaml_to_json(frontmatter);
    let hook_defs = parse_hooks_section(&yaml_value);

    let mut registered_ids = Vec::new();

    for def in hook_defs {
        // Parse the event type
        let event_filter = parse_event_type(&def.event);

        // Build the HookType from the definition
        let hook_type = if let Some(ref cmd) = def.command {
            HookType::Command(CommandHook {
                command: cmd.clone(),
                timeout_secs: 10,
            })
        } else if let Some(ref prompt) = def.prompt {
            HookType::Prompt(PromptHook {
                prompt_template: prompt.clone(),
            })
        } else {
            // Neither command nor prompt — skip
            continue;
        };

        let hook = RegisteredHook {
            id: format!("{skill_name}:{}", def.event),
            hook_type,
            event_filter: event_filter.into_iter().collect(),
            source: HookSource::Frontmatter,
        };

        let id = registry.register(hook).await;
        registered_ids.push(id);
    }

    registered_ids
}

/// Parse the `hooks` section from frontmatter JSON/YAML value.
///
/// Extracts an array of hook definitions from the JSON representation of
/// the frontmatter. Returns an empty vec if no `hooks` key is present.
fn parse_hooks_section(yaml: &serde_json::Value) -> Vec<FrontmatterHookDef> {
    let Some(hooks_array) = yaml.get("hooks").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    hooks_array
        .iter()
        .filter_map(|entry| {
            let event = entry.get("event")?.as_str()?.to_string();
            let command = entry
                .get("command")
                .and_then(|v| v.as_str())
                .map(String::from);
            let prompt = entry
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Must have at least one of command or prompt
            if command.is_none() && prompt.is_none() {
                return None;
            }

            Some(FrontmatterHookDef {
                event,
                command,
                prompt,
            })
        })
        .collect()
}

/// Parse a simple flat YAML string into a JSON Value.
///
/// Handles the basic `key: value` and `key:` + array format used in
/// skill frontmatter. This is not a full YAML parser.
fn parse_simple_yaml_to_json(yaml: &str) -> serde_json::Value {
    let mut root = serde_json::Map::new();
    let mut current_array: Option<(String, Vec<serde_json::Value>)> = None;
    let mut current_item: Option<serde_json::Map<String, serde_json::Value>> = None;

    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Array item: "  - event: pre_tool_use" or "  - event: ..."
        if let Some(rest) = trimmed.strip_prefix("- ") {
            // Start a new item in the current array
            if let Some(ref mut item) = current_item {
                // Save previous item
                if let Some((_, ref mut arr)) = current_array {
                    arr.push(serde_json::Value::Object(item.clone()));
                }
            }
            current_item = Some(serde_json::Map::new());

            // Parse key: value from the rest
            if let Some((key, value)) = rest.split_once(':') {
                let key = key.trim();
                let value = value.trim();
                if !value.is_empty()
                    && let Some(ref mut item) = current_item
                {
                    item.insert(
                        key.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
            }
        } else if trimmed.contains(':') && !line.starts_with(' ') && !line.starts_with('\t') {
            // Top-level key
            // Flush any current array
            #[allow(clippy::collapsible_if)]
            if let Some(ref item) = current_item {
                if let Some((_, ref mut arr)) = current_array {
                    arr.push(serde_json::Value::Object(item.clone()));
                }
                current_item = None;
            }
            if let Some((name, arr)) = current_array.take() {
                root.insert(name, serde_json::Value::Array(arr));
            }

            if let Some((key, value)) = trimmed.split_once(':') {
                let key = key.trim();
                let value = value.trim();
                if value.is_empty() {
                    // Start of an array or nested object
                    current_array = Some((key.to_string(), Vec::new()));
                } else {
                    root.insert(
                        key.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
            }
        } else if let Some(ref mut item) = current_item {
            // Continuation of array item properties: "    command: echo check"
            if let Some((key, value)) = trimmed.split_once(':') {
                let key = key.trim();
                let value = value.trim();
                if !value.is_empty() {
                    item.insert(
                        key.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
            }
        }
    }

    // Flush remaining
    if let Some(item) = current_item
        && let Some((_, ref mut arr)) = current_array
    {
        arr.push(serde_json::Value::Object(item));
    }
    if let Some((name, arr)) = current_array {
        root.insert(name, serde_json::Value::Array(arr));
    }

    serde_json::Value::Object(root)
}

/// Map event name string to `HookEventType`.
fn parse_event_type(event: &str) -> Vec<HookEventType> {
    match event.to_lowercase().as_str() {
        "session_start" | "sessionstart" => vec![HookEventType::SessionStart],
        "session_end" | "sessionend" => vec![HookEventType::SessionEnd],
        "pre_tool_use" | "pretooluse" => vec![HookEventType::PreToolUse],
        "post_tool_use" | "posttooluse" => vec![HookEventType::PostToolUse],
        "user_prompt_submit" | "userpromptsubmit" => vec![HookEventType::UserPromptSubmit],
        "stop" => vec![HookEventType::Stop],
        "file_changed" | "filechanged" => vec![HookEventType::FileChanged],
        "notification" => vec![HookEventType::Notification],
        _ => Vec::new(), // Unknown event — no filter, won't match anything
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_hook_def_fields() {
        let def = FrontmatterHookDef {
            event: "pre_tool_use".into(),
            command: Some("echo check".into()),
            prompt: None,
        };
        assert_eq!(def.event, "pre_tool_use");
        assert!(def.command.is_some());
        assert!(def.prompt.is_none());
    }

    #[test]
    fn frontmatter_hook_def_prompt_variant() {
        let def = FrontmatterHookDef {
            event: "session_start".into(),
            command: None,
            prompt: Some("Initialize context".into()),
        };
        assert_eq!(def.event, "session_start");
        assert!(def.command.is_none());
        assert!(def.prompt.is_some());
    }

    #[test]
    fn parse_hooks_section_with_hooks() {
        let yaml = serde_json::json!({
            "hooks": [
                {"event": "pre_tool_use", "command": "echo check"},
                {"event": "session_start", "prompt": "Init context"}
            ]
        });
        let defs = parse_hooks_section(&yaml);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].event, "pre_tool_use");
        assert_eq!(defs[0].command.as_deref(), Some("echo check"));
        assert_eq!(defs[1].event, "session_start");
        assert_eq!(defs[1].prompt.as_deref(), Some("Init context"));
    }

    #[test]
    fn parse_hooks_section_no_hooks_key() {
        let yaml = serde_json::json!({"name": "test"});
        let defs = parse_hooks_section(&yaml);
        assert!(defs.is_empty());
    }

    #[test]
    fn parse_hooks_section_skips_invalid() {
        let yaml = serde_json::json!({
            "hooks": [
                {"event": "pre_tool_use"},  // no command or prompt
                {"command": "echo"},         // no event
                {"event": "stop", "command": "echo done"}
            ]
        });
        let defs = parse_hooks_section(&yaml);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].event, "stop");
    }

    #[test]
    fn parse_simple_yaml_basic() {
        let yaml = "name: test-skill\ndescription: A test";
        let json = parse_simple_yaml_to_json(yaml);
        assert_eq!(json["name"], "test-skill");
        assert_eq!(json["description"], "A test");
    }

    #[test]
    fn parse_simple_yaml_with_hooks_array() {
        let yaml = "name: test\nhooks:\n  - event: pre_tool_use\n    command: echo check\n  - event: stop\n    prompt: verify";
        let json = parse_simple_yaml_to_json(yaml);
        let hooks = json["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0]["event"], "pre_tool_use");
        assert_eq!(hooks[0]["command"], "echo check");
        assert_eq!(hooks[1]["event"], "stop");
        assert_eq!(hooks[1]["prompt"], "verify");
    }

    #[test]
    fn parse_event_type_known() {
        assert_eq!(parse_event_type("pre_tool_use").len(), 1);
        assert_eq!(parse_event_type("session_start").len(), 1);
        assert_eq!(parse_event_type("PreToolUse").len(), 1);
    }

    #[test]
    fn parse_event_type_unknown() {
        assert!(parse_event_type("unknown_event").is_empty());
    }

    #[tokio::test]
    async fn register_frontmatter_hooks_basic() {
        let registry = HookRegistry::new();
        let frontmatter = "hooks:\n  - event: pre_tool_use\n    command: echo check";
        let ids = register_frontmatter_hooks(&registry, "my-skill", frontmatter).await;
        assert_eq!(ids.len(), 1);
    }

    #[tokio::test]
    async fn register_frontmatter_hooks_empty() {
        let registry = HookRegistry::new();
        let ids = register_frontmatter_hooks(&registry, "my-skill", "name: test").await;
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn register_frontmatter_hooks_multiple() {
        let registry = HookRegistry::new();
        let frontmatter = "hooks:\n  - event: pre_tool_use\n    command: echo before\n  - event: stop\n    prompt: check done";
        let ids = register_frontmatter_hooks(&registry, "my-skill", frontmatter).await;
        assert_eq!(ids.len(), 2);
    }
}
