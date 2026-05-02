//! Bridge between [`McpManager`] and [`SkillRegistry`].
//!
//! At runtime, every prompt exposed by every connected MCP server is fetched
//! once via `prompts/get`, converted into a [`Skill`] via
//! [`crab_skills::mcp::mcp_prompt_to_skill`], and registered into the global
//! [`SkillRegistry`] under the `mcp__<server>__<prompt>` namespace.
//!
//! This module lives in `crab-agents` because it is the lowest layer that
//! depends on both `crab-mcp` and `crab-skills` — the skills crate must
//! remain unaware of MCP, and `crab-mcp` must not pull in skill knowledge.
//!
//! [`McpManager`]: crab_mcp::McpManager
//! [`SkillRegistry`]: crab_skills::SkillRegistry
//! [`Skill`]: crab_skills::Skill

use std::collections::HashMap;

use crab_mcp::McpManager;
use crab_mcp::protocol::{PromptGetResult, PromptMessageContent};
use crab_skills::SkillRegistry;

/// Discover prompts on every connected MCP server, fetch their rendered
/// content, convert each into a [`Skill`], and register the skills into
/// the supplied registry.
///
/// Returns the number of prompt-skills successfully registered.
///
/// Failures (a single server's `prompts/get` call returning an error, or the
/// builder rejecting the converted skill) are logged via `tracing` and
/// skipped — they do not abort registration of remaining prompts.
///
/// [`Skill`]: crab_skills::Skill
pub async fn register_mcp_prompt_skills(
    manager: &McpManager,
    registry: &mut SkillRegistry,
) -> usize {
    let discovered = manager.discovered_prompts().await;
    let mut count = 0;

    for dp in discovered {
        let content = {
            let client = dp.client.lock().await;
            match client.get_prompt(&dp.prompt.name, HashMap::new()).await {
                Ok(result) => extract_prompt_text(&result),
                Err(e) => {
                    tracing::warn!(
                        server = dp.server_name.as_str(),
                        prompt = dp.prompt.name.as_str(),
                        error = %e,
                        "failed to fetch MCP prompt content, falling back to description"
                    );
                    dp.prompt.description.clone().unwrap_or_default()
                }
            }
        };

        let arguments: Vec<(String, Option<String>, bool)> = dp
            .prompt
            .arguments
            .iter()
            .map(|a| (a.name.clone(), a.description.clone(), a.required))
            .collect();

        match crab_skills::mcp::mcp_prompt_to_skill(
            &dp.server_name,
            &dp.prompt.name,
            dp.prompt.description.as_deref(),
            &arguments,
            &content,
        ) {
            Ok(skill) => {
                tracing::debug!(
                    name = skill.name.as_str(),
                    server = dp.server_name.as_str(),
                    "registered MCP prompt as skill"
                );
                registry.register(skill);
                count += 1;
            }
            Err(e) => {
                tracing::warn!(
                    prompt = dp.prompt.name.as_str(),
                    error = %e,
                    "failed to convert MCP prompt to skill"
                );
            }
        }
    }
    count
}

fn extract_prompt_text(result: &PromptGetResult) -> String {
    result
        .messages
        .iter()
        .filter_map(|msg| match &msg.content {
            PromptMessageContent::Text { text } => Some(text.as_str()),
            PromptMessageContent::Resource { resource } => resource.text.as_deref(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_mcp::protocol::{PromptMessage, ResourceContent};

    #[test]
    fn extract_prompt_text_concatenates_text_messages() {
        let result = PromptGetResult {
            description: None,
            messages: vec![
                PromptMessage {
                    role: "user".into(),
                    content: PromptMessageContent::Text {
                        text: "first".into(),
                    },
                },
                PromptMessage {
                    role: "assistant".into(),
                    content: PromptMessageContent::Text {
                        text: "second".into(),
                    },
                },
            ],
        };
        assert_eq!(extract_prompt_text(&result), "first\n\nsecond");
    }

    #[test]
    fn extract_prompt_text_pulls_resource_text_when_present() {
        let result = PromptGetResult {
            description: None,
            messages: vec![PromptMessage {
                role: "user".into(),
                content: PromptMessageContent::Resource {
                    resource: ResourceContent {
                        uri: "mem://x".into(),
                        mime_type: Some("text/plain".into()),
                        text: Some("embedded text".into()),
                    },
                },
            }],
        };
        assert_eq!(extract_prompt_text(&result), "embedded text");
    }

    #[test]
    fn extract_prompt_text_skips_resource_without_text() {
        let result = PromptGetResult {
            description: None,
            messages: vec![PromptMessage {
                role: "user".into(),
                content: PromptMessageContent::Resource {
                    resource: ResourceContent {
                        uri: "mem://blob".into(),
                        mime_type: Some("application/octet-stream".into()),
                        text: None,
                    },
                },
            }],
        };
        assert_eq!(extract_prompt_text(&result), "");
    }

    #[tokio::test]
    async fn register_with_no_servers_returns_zero() {
        let manager = McpManager::new();
        let mut registry = SkillRegistry::new();
        let count = register_mcp_prompt_skills(&manager, &mut registry).await;
        assert_eq!(count, 0);
    }
}
