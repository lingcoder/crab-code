//! `SkillPromptHandler` — bridges `SkillRegistry` skills to MCP prompts.
//!
//! Each `(name, description, content)` tuple becomes a `prompts/list` entry
//! and returns its content as a single user message with `{{arg}}`
//! placeholders substituted at `prompts/get` time.

use super::PromptHandler;
use crate::protocol::{
    McpPrompt, PromptArgument, PromptGetResult, PromptMessage, PromptMessageContent,
};

/// A prompt handler that bridges `SkillRegistry` skills to MCP prompts.
///
/// Each skill becomes an MCP prompt. The skill content is returned as a
/// single user message, with any `{{arg}}` placeholders substituted.
pub struct SkillPromptHandler {
    prompts: Vec<(McpPrompt, String)>, // (definition, content template)
}

impl SkillPromptHandler {
    /// Create a handler from a list of `(name, description, content)` tuples.
    ///
    /// Arguments are detected by scanning content for `{{placeholder}}` tokens.
    pub fn new(skills: Vec<(String, String, String)>) -> Self {
        let prompts = skills
            .into_iter()
            .map(|(name, description, content)| {
                let arguments = extract_placeholders(&content);
                let prompt = McpPrompt {
                    name,
                    description: if description.is_empty() {
                        None
                    } else {
                        Some(description)
                    },
                    arguments,
                };
                (prompt, content)
            })
            .collect();
        Self { prompts }
    }
}

/// Extract `{{placeholder}}` tokens from content and return them as prompt arguments.
pub(crate) fn extract_placeholders(content: &str) -> Vec<PromptArgument> {
    let mut seen = std::collections::HashSet::new();
    let mut args = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim().to_string();
            if !name.is_empty() && seen.insert(name.clone()) {
                args.push(PromptArgument {
                    name,
                    description: None,
                    required: true,
                });
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    args
}

impl PromptHandler for SkillPromptHandler {
    fn list_prompts(&self) -> Vec<McpPrompt> {
        self.prompts.iter().map(|(p, _)| p.clone()).collect()
    }

    fn get_prompt(
        &self,
        name: &str,
        arguments: &std::collections::HashMap<String, String>,
    ) -> Result<PromptGetResult, String> {
        let (prompt_def, template) = self
            .prompts
            .iter()
            .find(|(p, _)| p.name == name)
            .ok_or_else(|| format!("prompt not found: {name}"))?;

        // Validate required arguments
        for arg in &prompt_def.arguments {
            if arg.required && !arguments.contains_key(&arg.name) {
                return Err(format!("missing required argument: {}", arg.name));
            }
        }

        // Substitute placeholders
        let mut text = template.clone();
        for (key, value) in arguments {
            text = text.replace(&format!("{{{{{key}}}}}"), value);
        }

        Ok(PromptGetResult {
            description: prompt_def.description.clone(),
            messages: vec![PromptMessage {
                role: "user".into(),
                content: PromptMessageContent::Text { text },
            }],
        })
    }
}
