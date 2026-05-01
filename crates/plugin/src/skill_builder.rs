//! MCP → skill bridge.
//!
//! Converts MCP server tool lists into native [`Skill`](crab_skills::Skill)
//! instances. This module lives in `plugin` (not `skills`) because it bridges
//! two independent subsystems: skills and MCP.

use crab_skills::builder::SkillBuilder;
use crab_skills::types::{Skill, SkillSource};

// ─── MCP skill loading ────────────────────────────────────────────────

/// Convert an MCP server's tool list into native [`Skill`] instances.
///
/// Each MCP tool becomes a skill named `<server_name>:<tool_name>` with
/// the tool's description as the skill description and a `Manual` trigger.
///
/// # Arguments
///
/// * `server_name` — Name of the MCP server (used as skill name prefix).
/// * `tools` — Array of MCP tool definition JSON objects (each with `name`,
///   `description`, etc.).
pub fn load_mcp_skills(server_name: &str, tools: &[serde_json::Value]) -> Vec<Skill> {
    tools
        .iter()
        .filter_map(|tool| {
            let tool_name = tool.get("name")?.as_str()?;
            let desc = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let full_name = format!("{server_name}:{tool_name}");
            SkillBuilder::new(full_name)
                .description(desc)
                .content(desc)
                .source(SkillSource::Mcp {
                    server_name: server_name.to_string(),
                })
                .build()
                .ok()
        })
        .collect()
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_mcp_skills_basic() {
        let tools = vec![serde_json::json!({
            "name": "read_file",
            "description": "Read a file from disk"
        })];
        let skills = load_mcp_skills("filesystem", &tools);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "filesystem:read_file");
        assert!(matches!(
            skills[0].source,
            SkillSource::Mcp { ref server_name } if server_name == "filesystem"
        ));
    }

    #[test]
    fn load_mcp_skills_skips_invalid() {
        let tools = vec![
            serde_json::json!({"name": "good", "description": "works"}),
            serde_json::json!({"invalid": "no name field"}),
        ];
        let skills = load_mcp_skills("srv", &tools);
        assert_eq!(skills.len(), 1);
    }
}
