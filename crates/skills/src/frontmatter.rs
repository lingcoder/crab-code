//! Skill frontmatter parsing.
//!
//! Skills are markdown files with YAML frontmatter containing metadata.
//! This module handles splitting the frontmatter from the body and parsing
//! the metadata fields into [`Skill`] instances.
//!
//! ## Supported frontmatter fields
//!
//! | Field | Type | Description |
//! |-------|------|-------------|
//! | `name` | string | Skill identifier |
//! | `description` | string | Human-readable description |
//! | `aliases` | comma-separated | Alternative names |
//! | `when_to_use` | string | When model should invoke |
//! | `argument-hint` | string | Expected arguments hint |
//! | `allowed-tools` | comma-separated | Tool allowlist |
//! | `model` | string | Model override |
//! | `context` | `inline`/`fork` | Execution mode |
//! | `agent` | string | Agent type for fork |
//! | `effort` | string | Effort level hint |
//! | `user-invocable` | `true`/`false` | User slash access |
//! | `disable-model-invocation` | `true`/`false` | Prevent auto-invoke |
//! | `trigger` | nested | Trigger definition |
//! | `hooks` | nested | Hook definitions (opaque) |

use std::path::Path;

use crate::types::{Skill, SkillContext, SkillSource, SkillTrigger};

// ─── Public API ────────────────────────────────────────────────────────

/// Parse skill content from a string (frontmatter + markdown body).
///
/// Returns a fully populated [`Skill`] with fields extracted from both
/// the YAML frontmatter and the markdown body.
pub fn parse_skill_content(content: &str, source_path: Option<&Path>) -> crab_core::Result<Skill> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let yaml = parse_simple_yaml(&frontmatter);

    let mut skill = extract_skill_fields(&yaml)?;
    skill.content = body;
    skill.source_path = source_path.map(Path::to_path_buf);
    skill.source = SkillSource::Disk;

    // Fall back to filename if name is empty.
    if skill.name.is_empty()
        && let Some(stem) = source_path.and_then(|p| p.file_stem())
    {
        skill.name = stem.to_string_lossy().into_owned();
    }

    Ok(skill)
}

/// Load a skill from a markdown file on disk.
pub fn load_skill_file(path: &Path) -> crab_core::Result<Skill> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| crab_core::Error::Other(format!("failed to read skill file: {e}")))?;

    parse_skill_content(&content, Some(path))
}

// ─── Frontmatter splitting ─────────────────────────────────────────────

/// Split frontmatter from body. Frontmatter is delimited by `---` lines.
pub fn split_frontmatter(content: &str) -> crab_core::Result<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(crab_core::Error::Other(
            "skill file must start with '---' frontmatter delimiter".into(),
        ));
    }

    let after_first = &trimmed[3..].trim_start_matches(['\r', '\n']);
    after_first.find("\n---").map_or_else(
        || {
            Err(crab_core::Error::Other(
                "skill file missing closing '---' frontmatter delimiter".into(),
            ))
        },
        |end_pos| {
            let frontmatter = after_first[..end_pos].to_string();
            let body_start = end_pos + 4; // skip \n---
            let body = after_first[body_start..]
                .trim_start_matches(['\r', '\n'])
                .to_string();
            Ok((frontmatter, body))
        },
    )
}

// ─── YAML parsing ──────────────────────────────────────────────────────

/// Parse simple YAML-like frontmatter into a JSON value.
///
/// Supports flat key-value pairs and one level of nesting via indentation.
/// This avoids pulling in a full YAML parser dependency.
pub fn parse_simple_yaml(yaml: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut current_nested_key: Option<String> = None;
    let mut nested_map = serde_json::Map::new();

    for line in yaml.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();

        if indent >= 2 {
            // Nested value under current_nested_key.
            if let Some(colon_pos) = trimmed.find(':') {
                let key = trimmed[..colon_pos].trim();
                let value = trimmed[colon_pos + 1..].trim();
                nested_map.insert(
                    key.to_string(),
                    serde_json::Value::String(value.to_string()),
                );
            }
        } else if let Some(colon_pos) = trimmed.find(':') {
            // Flush previous nested map.
            if let Some(ref nk) = current_nested_key
                && !nested_map.is_empty()
            {
                map.insert(nk.clone(), serde_json::Value::Object(nested_map.clone()));
                nested_map.clear();
            }

            let key = trimmed[..colon_pos].trim().to_string();
            let value = trimmed[colon_pos + 1..].trim();

            if value.is_empty() {
                // This key has nested children.
                current_nested_key = Some(key);
            } else {
                current_nested_key = None;
                map.insert(key, serde_json::Value::String(value.to_string()));
            }
        }
    }

    // Flush final nested map.
    if let Some(ref nk) = current_nested_key
        && !nested_map.is_empty()
    {
        map.insert(nk.clone(), serde_json::Value::Object(nested_map));
    }

    serde_json::Value::Object(map)
}

// ─── Field extraction ──────────────────────────────────────────────────

/// Extract all skill fields from parsed YAML frontmatter.
fn extract_skill_fields(yaml: &serde_json::Value) -> crab_core::Result<Skill> {
    let name = str_field(yaml, "name").unwrap_or_default();
    let description = str_field(yaml, "description").unwrap_or_default();

    // Aliases: comma-separated string.
    let aliases = str_field(yaml, "aliases")
        .map(|s| {
            s.split(',')
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Trigger: nested object with `type` + `name`/`regex`.
    let trigger = parse_trigger(yaml);

    // Model invocation metadata.
    let when_to_use = str_field(yaml, "when_to_use").or_else(|| str_field(yaml, "when-to-use"));
    let argument_hint =
        str_field(yaml, "argument-hint").or_else(|| str_field(yaml, "argument_hint"));

    // Access control.
    let allowed_tools = str_field(yaml, "allowed-tools")
        .or_else(|| str_field(yaml, "allowed_tools"))
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let model = str_field(yaml, "model");
    let disable_model_invocation = bool_field(yaml, "disable-model-invocation")
        .or_else(|| bool_field(yaml, "disable_model_invocation"))
        .unwrap_or(false);
    let user_invocable = bool_field(yaml, "user-invocable")
        .or_else(|| bool_field(yaml, "user_invocable"))
        .unwrap_or(true);

    // Execution context.
    let context = str_field(yaml, "context")
        .map(|s| match s.as_str() {
            "fork" => SkillContext::Fork,
            _ => SkillContext::Inline,
        })
        .unwrap_or_default();
    let agent = str_field(yaml, "agent");
    let effort = str_field(yaml, "effort");

    // Hooks: pass through as opaque JSON.
    let hooks = yaml.get("hooks").cloned();

    Ok(Skill {
        name,
        description,
        aliases,
        trigger,
        content: String::new(), // filled by caller
        source_path: None,      // filled by caller
        when_to_use,
        argument_hint,
        allowed_tools,
        model,
        disable_model_invocation,
        user_invocable,
        context,
        agent,
        effort,
        source: SkillSource::default(),
        files: None,
        hooks,
    })
}

/// Parse trigger definition from frontmatter YAML.
fn parse_trigger(yaml: &serde_json::Value) -> SkillTrigger {
    let Some(trigger_obj) = yaml.get("trigger") else {
        return SkillTrigger::Manual;
    };

    let trigger_type = trigger_obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("manual");

    match trigger_type {
        "command" => {
            let name = trigger_obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            SkillTrigger::Command { name }
        }
        "pattern" => {
            let regex = trigger_obj
                .get("regex")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            SkillTrigger::Pattern { regex }
        }
        _ => SkillTrigger::Manual,
    }
}

/// Extract a string field from YAML.
fn str_field(yaml: &serde_json::Value, key: &str) -> Option<String> {
    yaml.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Extract a boolean field from YAML (stored as string "true"/"false").
fn bool_field(yaml: &serde_json::Value, key: &str) -> Option<bool> {
    yaml.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("true"))
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let content = "---\nname: test\n---\nBody here";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm, "name: test");
        assert_eq!(body, "Body here");
    }

    #[test]
    fn split_frontmatter_missing_close() {
        let content = "---\nname: test\nno closing";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn split_frontmatter_no_start() {
        let content = "no frontmatter here";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn split_frontmatter_multiline_body() {
        let content = "---\nname: test\n---\nLine 1\nLine 2\nLine 3";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm, "name: test");
        assert!(body.contains("Line 1"));
        assert!(body.contains("Line 3"));
    }

    #[test]
    fn parse_simple_yaml_flat() {
        let yaml = "name: commit\ndescription: Create a commit";
        let val = parse_simple_yaml(yaml);
        assert_eq!(val["name"], "commit");
        assert_eq!(val["description"], "Create a commit");
    }

    #[test]
    fn parse_simple_yaml_nested() {
        let yaml = "name: test\ntrigger:\n  type: command\n  name: test";
        let val = parse_simple_yaml(yaml);
        assert_eq!(val["name"], "test");
        assert_eq!(val["trigger"]["type"], "command");
        assert_eq!(val["trigger"]["name"], "test");
    }

    #[test]
    fn parse_simple_yaml_empty_lines() {
        let yaml = "name: test\n\ndescription: with gaps";
        let val = parse_simple_yaml(yaml);
        assert_eq!(val["name"], "test");
        assert_eq!(val["description"], "with gaps");
    }

    #[test]
    fn parse_skill_content_command_trigger() {
        let content = "---\nname: commit\ndescription: Create a git commit\ntrigger:\n  type: command\n  name: commit\n---\nYou are a commit helper.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.name, "commit");
        assert_eq!(skill.description, "Create a git commit");
        assert_eq!(skill.content, "You are a commit helper.");
        assert!(matches!(
            skill.trigger,
            SkillTrigger::Command { ref name } if name == "commit"
        ));
        assert_eq!(skill.source, SkillSource::Disk);
    }

    #[test]
    fn parse_skill_content_manual_trigger() {
        let content = "---\nname: helper\ndescription: A helper skill\n---\nHelp content.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.name, "helper");
        assert!(matches!(skill.trigger, SkillTrigger::Manual));
    }

    #[test]
    fn parse_skill_content_pattern_trigger() {
        let content = "---\nname: fix-bug\ndescription: Fix a bug\ntrigger:\n  type: pattern\n  regex: (?i)fix\\s+bug\n---\nFixing instructions here.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.name, "fix-bug");
        assert!(matches!(skill.trigger, SkillTrigger::Pattern { .. }));
    }

    #[test]
    fn parse_skill_content_name_from_filename() {
        let content = "---\ndescription: Test\n---\nBody.";
        let path = std::path::Path::new("/skills/my-skill.md");
        let skill = parse_skill_content(content, Some(path)).unwrap();
        assert_eq!(skill.name, "my-skill");
    }

    #[test]
    fn parse_extended_frontmatter_fields() {
        let content = "\
---
name: batch
description: Parallel execution
when_to_use: Large-scale changes
argument-hint: <instruction>
allowed-tools: Read, Bash, Grep
model: sonnet
context: fork
agent: code-architect
effort: high
user-invocable: true
disable-model-invocation: true
aliases: parallel, sweep
---
Batch prompt content.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.name, "batch");
        assert_eq!(skill.when_to_use.as_deref(), Some("Large-scale changes"));
        assert_eq!(skill.argument_hint.as_deref(), Some("<instruction>"));
        assert_eq!(skill.allowed_tools, vec!["Read", "Bash", "Grep"]);
        assert_eq!(skill.model.as_deref(), Some("sonnet"));
        assert_eq!(skill.context, SkillContext::Fork);
        assert_eq!(skill.agent.as_deref(), Some("code-architect"));
        assert_eq!(skill.effort.as_deref(), Some("high"));
        assert!(skill.user_invocable);
        assert!(skill.disable_model_invocation);
        assert_eq!(skill.aliases, vec!["parallel", "sweep"]);
    }

    #[test]
    fn parse_frontmatter_kebab_and_snake_case() {
        // Both `when_to_use` and `when-to-use` should work.
        let content = "---\nname: t\nwhen-to-use: hint\n---\nBody.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.when_to_use.as_deref(), Some("hint"));
    }

    #[test]
    fn parse_frontmatter_hooks_passthrough() {
        let content =
            "---\nname: t\nhooks:\n  event: pre_tool_use\n  command: echo check\n---\nBody.";
        let skill = parse_skill_content(content, None).unwrap();
        assert!(skill.hooks.is_some());
        let hooks = skill.hooks.unwrap();
        assert_eq!(hooks["event"], "pre_tool_use");
    }

    #[test]
    fn parse_frontmatter_defaults() {
        let content = "---\nname: minimal\n---\nBody.";
        let skill = parse_skill_content(content, None).unwrap();
        assert!(skill.user_invocable); // default true
        assert!(!skill.disable_model_invocation); // default false
        assert_eq!(skill.context, SkillContext::Inline); // default inline
        assert!(skill.allowed_tools.is_empty());
        assert!(skill.aliases.is_empty());
    }
}
