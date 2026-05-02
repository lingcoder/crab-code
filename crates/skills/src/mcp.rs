//! Convert MCP prompts into [`Skill`] instances.
//!
//! The skills crate must NOT depend on the `crab-mcp` crate, so this module
//! takes primitive types (strings, slices) and produces a [`Skill`]. The
//! actual bridge that pulls from a live `McpManager` lives in the higher
//! layer (`crab-agents`).
//!
//! Skill naming convention is `mcp__<server_name>__<prompt_name>`, mirroring
//! the namespace used for MCP-sourced tools.

use std::fmt::Write as _;

use crate::builder::SkillBuilder;
use crate::types::{Skill, SkillSource};

/// Build a [`Skill`] from a single MCP prompt definition.
///
/// `arguments` is a list of `(name, description, required)` tuples. When
/// non-empty, an "## Prompt Arguments" section is appended to the skill's
/// content so the model knows which arguments the prompt expects.
///
/// # Errors
///
/// Returns `Err` when the underlying [`SkillBuilder::build`] rejects the
/// inputs (e.g. empty name or empty content).
pub fn mcp_prompt_to_skill(
    server_name: &str,
    prompt_name: &str,
    description: Option<&str>,
    arguments: &[(String, Option<String>, bool)],
    content: &str,
) -> Result<Skill, String> {
    let skill_name = format!("mcp__{server_name}__{prompt_name}");
    let desc = description.unwrap_or(prompt_name);

    let mut full_content = content.to_string();
    if !arguments.is_empty() {
        full_content.push_str("\n\n## Prompt Arguments\n\n");
        for (name, arg_desc, required) in arguments {
            let req = if *required {
                " (required)"
            } else {
                " (optional)"
            };
            let d = arg_desc.as_deref().unwrap_or("No description");
            let _ = writeln!(full_content, "- **{name}**{req}: {d}");
        }
    }

    SkillBuilder::new(&skill_name)
        .description(desc)
        .command_trigger(&skill_name)
        .source(SkillSource::Mcp {
            server_name: server_name.into(),
        })
        .when_to_use(format!(
            "When the user wants to use the '{prompt_name}' prompt from '{server_name}' MCP server"
        ))
        .content(full_content)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SkillTrigger;

    #[test]
    fn skill_name_uses_double_underscore_namespace() {
        let skill = mcp_prompt_to_skill(
            "filesystem",
            "summarize_dir",
            Some("Summarize a directory"),
            &[],
            "Summarize the contents of {{path}}.",
        )
        .unwrap();
        assert_eq!(skill.name, "mcp__filesystem__summarize_dir");
    }

    #[test]
    fn source_is_mcp_with_server_name() {
        let skill = mcp_prompt_to_skill("github", "review_pr", None, &[], "body").unwrap();
        match &skill.source {
            SkillSource::Mcp { server_name } => assert_eq!(server_name, "github"),
            other => panic!("expected SkillSource::Mcp, got {other:?}"),
        }
    }

    #[test]
    fn trigger_is_command_with_full_skill_name() {
        let skill = mcp_prompt_to_skill("srv", "do_thing", None, &[], "x").unwrap();
        match &skill.trigger {
            SkillTrigger::Command { name } => assert_eq!(name, "mcp__srv__do_thing"),
            other => panic!("expected SkillTrigger::Command, got {other:?}"),
        }
    }

    #[test]
    fn content_includes_arguments_section_when_provided() {
        let args = vec![
            ("path".to_string(), Some("Target path".to_string()), true),
            ("recursive".to_string(), None, false),
        ];
        let skill = mcp_prompt_to_skill("fs", "scan", None, &args, "Scan a path.").unwrap();
        assert!(skill.content.contains("## Prompt Arguments"));
        assert!(skill.content.contains("**path** (required): Target path"));
        assert!(
            skill
                .content
                .contains("**recursive** (optional): No description")
        );
    }

    #[test]
    fn content_omits_arguments_section_when_empty() {
        let skill = mcp_prompt_to_skill("srv", "noargs", None, &[], "Just text.").unwrap();
        assert!(!skill.content.contains("## Prompt Arguments"));
        assert_eq!(skill.content, "Just text.");
    }

    #[test]
    fn description_falls_back_to_prompt_name_when_missing() {
        let skill = mcp_prompt_to_skill("srv", "fallback_name", None, &[], "x").unwrap();
        assert_eq!(skill.description, "fallback_name");
    }

    #[test]
    fn when_to_use_mentions_server_and_prompt() {
        let skill = mcp_prompt_to_skill("github", "review_pr", None, &[], "body").unwrap();
        let when = skill.when_to_use.as_deref().unwrap_or_default();
        assert!(when.contains("review_pr"));
        assert!(when.contains("github"));
    }

    #[test]
    fn empty_content_is_rejected() {
        let result = mcp_prompt_to_skill("srv", "p", None, &[], "");
        assert!(result.is_err());
    }
}
