//! Conversion between Chat Completions API types and internal types.

use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::TokenUsage;

use super::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
    CompletionUsage, ToolCall,
};
use crate::error::Result;
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

/// Convert internal `MessageRequest` to Chat Completions request.
pub fn to_chat_completion_request(req: &MessageRequest<'_>, stream: bool) -> ChatCompletionRequest {
    let mut messages = Vec::new();

    // System prompt → messages[0] with role="system"
    if let Some(system) = &req.system {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: Some(system.clone()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
    }

    // Convert each internal message
    for msg in req.messages.iter() {
        let converted = messages_to_openai(msg);
        messages.extend(converted);
    }

    ChatCompletionRequest {
        model: req.model.0.clone(),
        messages,
        max_tokens: Some(req.max_tokens),
        temperature: req.temperature,
        tools: req.tools.clone(),
        stream,
    }
}

/// Convert an internal `Message` to one or more chat messages.
///
/// A single internal message may produce multiple chat messages because
/// tool results become separate `role="tool"` messages.
fn messages_to_openai(msg: &Message) -> Vec<ChatMessage> {
    let role_str = match msg.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };

    // Collect tool_use blocks into tool_calls
    let tool_calls: Vec<ToolCall> = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                id: id.clone(),
                call_type: "function".to_string(),
                function: super::types::FunctionCall {
                    name: name.clone(),
                    arguments: input.to_string(),
                },
            }),
            _ => None,
        })
        .collect();

    // Collect text content
    let text: String = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    // Collect tool results as separate messages
    let tool_result_messages: Vec<ChatMessage> = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some(ChatMessage {
                role: "tool".to_string(),
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: Some(tool_use_id.clone()),
                name: None,
            }),
            _ => None,
        })
        .collect();

    if !tool_result_messages.is_empty() {
        return tool_result_messages;
    }

    let main_message = ChatMessage {
        role: role_str.to_string(),
        content: if text.is_empty() { None } else { Some(text) },
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        tool_call_id: None,
        name: None,
    };

    vec![main_message]
}

/// Convert non-streaming response to internal types.
///
/// # Errors
///
/// Returns `ApiError` if the response has no choices.
pub fn from_chat_completion_response(resp: ChatCompletionResponse) -> Result<MessageResponse> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| crate::error::ApiError::Api {
            status: 0,
            message: "no choices in response".to_string(),
        })?;

    let content = chat_message_to_content_blocks(&choice.message);
    let message = Message {
        role: Role::Assistant,
        content,
    };

    let usage = resp
        .usage
        .map(|u| from_completion_usage(&u))
        .unwrap_or_default();

    Ok(MessageResponse {
        id: resp.id,
        message,
        usage,
    })
}

/// Convert a `ChatMessage` to internal `ContentBlock`s.
fn chat_message_to_content_blocks(msg: &ChatMessage) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    if let Some(text) = &msg.content
        && !text.is_empty()
    {
        blocks.push(ContentBlock::Text { text: text.clone() });
    }

    if let Some(tool_calls) = &msg.tool_calls {
        for tc in tool_calls {
            let input = serde_json::from_str(&tc.function.arguments)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            blocks.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                input,
            });
        }
    }

    blocks
}

/// Convert completion usage to internal `TokenUsage`.
pub const fn from_completion_usage(usage: &CompletionUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    }
}

/// Convert a streaming chunk to internal `StreamEvent`s.
pub fn chunk_to_stream_event(chunk: &ChatCompletionChunk) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    for choice in &chunk.choices {
        if let Some(content) = &choice.delta.content
            && !content.is_empty()
        {
            events.push(StreamEvent::ContentDelta {
                index: choice.index,
                delta: content.clone(),
            });
        }

        if let Some(tool_calls) = &choice.delta.tool_calls {
            for tc in tool_calls {
                if let Some(func) = &tc.function
                    && let Some(args) = &func.arguments
                    && !args.is_empty()
                {
                    events.push(StreamEvent::ContentDelta {
                        index: tc.index,
                        delta: args.clone(),
                    });
                }
            }
        }

        if let Some(reason) = &choice.finish_reason {
            events.push(StreamEvent::MessageDelta {
                usage: TokenUsage::default(),
                stop_reason: Some(reason.clone()),
            });
            events.push(StreamEvent::MessageStop);
        }
    }

    if let Some(usage) = &chunk.usage {
        events.push(StreamEvent::MessageDelta {
            usage: from_completion_usage(usage),
            stop_reason: None,
        });
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::{ContentBlock, Message};
    use crab_core::model::ModelId;
    use serde_json::json;

    fn make_request() -> MessageRequest<'static> {
        MessageRequest {
            model: ModelId::from("gpt-4o"),
            messages: std::borrow::Cow::Owned(vec![
                Message::user("Hello"),
                Message::assistant("Hi there!"),
            ]),
            system: Some("You are helpful.".into()),
            max_tokens: 1024,
            tools: vec![],
            temperature: Some(0.7),
            cache_breakpoints: vec![],
        }
    }

    #[test]
    fn to_request_includes_system_as_first_message() {
        let req = make_request();
        let chat_req = to_chat_completion_request(&req, false);
        assert_eq!(chat_req.messages[0].role, "system");
        assert_eq!(
            chat_req.messages[0].content.as_deref(),
            Some("You are helpful.")
        );
        assert!(!chat_req.stream);
    }

    #[test]
    fn to_request_stream_flag() {
        let req = make_request();
        let chat_req = to_chat_completion_request(&req, true);
        assert!(chat_req.stream);
    }

    #[test]
    fn to_request_model_and_params() {
        let req = make_request();
        let chat_req = to_chat_completion_request(&req, false);
        assert_eq!(chat_req.model, "gpt-4o");
        assert_eq!(chat_req.max_tokens, Some(1024));
        assert_eq!(chat_req.temperature, Some(0.7));
    }

    #[test]
    fn to_request_user_and_assistant_messages() {
        let req = make_request();
        let chat_req = to_chat_completion_request(&req, false);
        // system + user + assistant = 3 messages
        assert_eq!(chat_req.messages.len(), 3);
        assert_eq!(chat_req.messages[1].role, "user");
        assert_eq!(chat_req.messages[1].content.as_deref(), Some("Hello"));
        assert_eq!(chat_req.messages[2].role, "assistant");
    }

    #[test]
    fn to_request_tool_use_becomes_tool_calls() {
        let msg = Message::new(
            crab_core::message::Role::Assistant,
            vec![
                ContentBlock::text("Let me check."),
                ContentBlock::tool_use("tc_1", "read_file", json!({"path": "/tmp/x"})),
            ],
        );
        let req = MessageRequest {
            model: ModelId::from("gpt-4o"),
            messages: std::borrow::Cow::Owned(vec![msg]),
            system: None,
            max_tokens: 1024,
            tools: vec![],
            temperature: None,
            cache_breakpoints: vec![],
        };
        let chat_req = to_chat_completion_request(&req, false);
        let m = &chat_req.messages[0];
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content.as_deref(), Some("Let me check."));
        let tc = m.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "tc_1");
        assert_eq!(tc[0].function.name, "read_file");
    }

    #[test]
    fn to_request_tool_result_becomes_tool_role() {
        let msg = Message::tool_result("tc_1", "file contents", false);
        let req = MessageRequest {
            model: ModelId::from("gpt-4o"),
            messages: std::borrow::Cow::Owned(vec![msg]),
            system: None,
            max_tokens: 1024,
            tools: vec![],
            temperature: None,
            cache_breakpoints: vec![],
        };
        let chat_req = to_chat_completion_request(&req, false);
        let m = &chat_req.messages[0];
        assert_eq!(m.role, "tool");
        assert_eq!(m.tool_call_id.as_deref(), Some("tc_1"));
        assert_eq!(m.content.as_deref(), Some("file contents"));
    }

    #[test]
    fn from_response_basic() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-123".into(),
            object: "chat.completion".into(),
            model: "gpt-4o".into(),
            choices: vec![super::super::types::Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content: Some("Hello!".into()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(CompletionUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let msg_resp = from_chat_completion_response(resp).unwrap();
        assert_eq!(msg_resp.id, "chatcmpl-123");
        assert_eq!(msg_resp.message.text(), "Hello!");
        assert_eq!(msg_resp.usage.input_tokens, 10);
        assert_eq!(msg_resp.usage.output_tokens, 5);
    }

    #[test]
    fn from_response_no_choices_is_error() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-empty".into(),
            object: "chat.completion".into(),
            model: "gpt-4o".into(),
            choices: vec![],
            usage: None,
        };
        assert!(from_chat_completion_response(resp).is_err());
    }

    #[test]
    fn from_response_with_tool_calls() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-tc".into(),
            object: "chat.completion".into(),
            model: "gpt-4o".into(),
            choices: vec![super::super::types::Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_1".into(),
                        call_type: "function".into(),
                        function: super::super::types::FunctionCall {
                            name: "read_file".into(),
                            arguments: r#"{"path":"/tmp"}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                    name: None,
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };
        let msg_resp = from_chat_completion_response(resp).unwrap();
        assert!(msg_resp.message.has_tool_use());
        let uses: Vec<_> = msg_resp.message.tool_uses().collect();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, "read_file");
    }

    #[test]
    fn chunk_to_events_content_delta() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-1".into(),
            object: "chat.completion.chunk".into(),
            model: "gpt-4o".into(),
            choices: vec![super::super::types::ChunkChoice {
                index: 0,
                delta: super::super::types::ChunkDelta {
                    role: None,
                    content: Some("Hello".into()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        let events = chunk_to_stream_event(&chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::ContentDelta { delta, .. } if delta == "Hello"));
    }

    #[test]
    fn chunk_to_events_finish_reason() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-2".into(),
            object: "chat.completion.chunk".into(),
            model: "gpt-4o".into(),
            choices: vec![super::super::types::ChunkChoice {
                index: 0,
                delta: super::super::types::ChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(CompletionUsage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            }),
        };
        let events = chunk_to_stream_event(&chunk);
        assert!(events.iter().any(|e| matches!(e, StreamEvent::MessageStop)));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::MessageDelta { .. }))
        );
    }

    #[test]
    fn from_completion_usage_maps_tokens() {
        let usage = CompletionUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        let internal = from_completion_usage(&usage);
        assert_eq!(internal.input_tokens, 100);
        assert_eq!(internal.output_tokens, 50);
        assert_eq!(internal.cache_read_tokens, 0);
        assert_eq!(internal.cache_creation_tokens, 0);
    }
}
