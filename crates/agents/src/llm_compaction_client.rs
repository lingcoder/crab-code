//! LLM-backed [`CompactionClient`] implementation.
//!
//! Wraps an [`LlmBackend`] (any provider) and answers
//! [`CompactionClient::summarize`] by issuing a non-streaming, low-temperature
//! request whose system prompt is the caller-supplied instruction and whose
//! user message is a flat dump of the messages to summarise.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crab_api::LlmBackend;
use crab_api::types::MessageRequest;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::ModelId;
use crab_session::CompactionClient;

/// Hard cap on the response length the model is allowed to spend on a
/// single summarisation. Compaction is meant to *reduce* tokens, not
/// generate prose, so we keep this tight.
const MAX_SUMMARY_TOKENS: u32 = 1024;

/// Compaction calls are deterministic by design — the same conversation
/// must always compact to the same summary so resume + retry produce
/// stable transcripts.
const COMPACTION_TEMPERATURE: f32 = 0.0;

/// LLM-backed compaction client.
pub struct LlmCompactionClient {
    backend: Arc<LlmBackend>,
    model: ModelId,
    max_tokens: u32,
}

impl LlmCompactionClient {
    /// Build a client that issues compaction calls against `backend` using
    /// `model`.
    #[must_use]
    pub fn new(backend: Arc<LlmBackend>, model: ModelId) -> Self {
        Self {
            backend,
            model,
            max_tokens: MAX_SUMMARY_TOKENS,
        }
    }

    /// Override the per-call output budget. Defaults to
    /// [`MAX_SUMMARY_TOKENS`].
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

impl CompactionClient for LlmCompactionClient {
    fn summarize(
        &self,
        messages: &[Message],
        instruction: &str,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<String>> + Send + '_>> {
        let serialized = serialize_messages(messages);
        let instruction = instruction.to_string();
        let model = self.model.clone();
        let max_tokens = self.max_tokens;
        let backend = Arc::clone(&self.backend);

        Box::pin(async move {
            let req = MessageRequest {
                model,
                messages: Cow::Owned(vec![Message::user(&serialized)]),
                system: Some(instruction),
                max_tokens,
                tools: vec![],
                temperature: Some(COMPACTION_TEMPERATURE),
                cache_breakpoints: vec![],
                budget_tokens: None,
                response_format: None,
                tool_choice: None,
            };

            let response = backend
                .send_message(req)
                .await
                .map_err(|e| crab_core::Error::Other(format!("compaction LLM call failed: {e}")))?;

            Ok(response.message.text())
        })
    }
}

/// Render messages as a plain transcript the model can summarise.
///
/// Each message becomes one section: `<role>: <text>`. Tool results are
/// labelled and prefixed with their `tool_use_id` so the summary can
/// reference specific calls. Tool-use blocks are flattened to
/// `[tool_use:<name>]`.
fn serialize_messages(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        out.push_str(role);
        out.push_str(":\n");

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::ToolUse { name, .. } => {
                    out.push_str("[tool_use:");
                    out.push_str(name);
                    out.push_str("]\n");
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    out.push_str("[tool_result ");
                    out.push_str(tool_use_id);
                    if *is_error {
                        out.push_str(" error");
                    }
                    out.push_str("]\n");
                    out.push_str(content);
                    out.push('\n');
                }
                ContentBlock::Thinking { thinking } => {
                    out.push_str("[thinking]\n");
                    out.push_str(thinking);
                    out.push('\n');
                }
                ContentBlock::Image { .. } => {
                    out.push_str("[image]\n");
                }
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_text_messages() {
        let messages = vec![Message::user("hello"), Message::assistant("hi there")];
        let out = serialize_messages(&messages);
        assert!(out.contains("user:"));
        assert!(out.contains("hello"));
        assert!(out.contains("assistant:"));
        assert!(out.contains("hi there"));
    }

    #[test]
    fn serialize_tool_result_includes_id() {
        let messages = vec![Message::tool_result("tc_42", "exit code 0", false)];
        let out = serialize_messages(&messages);
        assert!(out.contains("tc_42"));
        assert!(out.contains("exit code 0"));
        assert!(!out.contains("error"));
    }

    #[test]
    fn serialize_tool_error_marked() {
        let messages = vec![Message::tool_result("tc_1", "command not found", true)];
        let out = serialize_messages(&messages);
        assert!(out.contains("error"));
        assert!(out.contains("command not found"));
    }

    #[test]
    fn serialize_empty_is_empty() {
        let out = serialize_messages(&[]);
        assert!(out.is_empty());
    }
}
