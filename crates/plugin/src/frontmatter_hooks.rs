//! Parse hook definitions from skill YAML frontmatter.
//!
//! Skills can declare hooks in their YAML frontmatter that should be registered
//! when the skill is loaded. This module extracts those hook definitions and
//! registers them with the [`HookRegistry`](super::hook_registry::HookRegistry).
//!
//! Maps to CCB `hooks/registerFrontmatterHooks.ts` + `hooks/registerSkillHooks.ts`.

use super::hook_registry::HookRegistry;

// ─── Frontmatter hook definition ───────────────────────────────────────

/// A hook definition parsed from a skill file's YAML frontmatter.
#[allow(dead_code)]
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
/// # Arguments
///
/// * `registry` — The hook registry to register hooks with.
/// * `skill_name` — Name of the skill (used for hook ID prefixing and logging).
/// * `frontmatter` — Raw YAML frontmatter string (without `---` delimiters).
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
pub fn register_frontmatter_hooks(
    _registry: &HookRegistry,
    _skill_name: &str,
    _frontmatter: &str,
) -> Vec<String> {
    todo!(
        "register_frontmatter_hooks: parse frontmatter, extract hooks section, register with registry"
    )
}

/// Parse the `hooks` section from frontmatter YAML.
///
/// Extracts an array of hook definitions from the JSON representation of
/// the frontmatter. Returns an empty vec if no `hooks` key is present.
#[allow(dead_code)]
fn parse_hooks_section(_yaml: &serde_json::Value) -> Vec<FrontmatterHookDef> {
    todo!("parse_hooks_section: extract hooks array from YAML value and map to FrontmatterHookDef")
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_hook_def_fields() {
        // Verify the struct can be constructed and has the expected fields.
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
}
