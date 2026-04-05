//! Conversion between Anthropic API types and internal types.

use crab_core::message::{ContentBlock, ImageSource, Message, Role};
use crab_core::model::TokenUsage;

use super::types::{
    AnthropicContentBlock, AnthropicDelta, AnthropicImageSource, AnthropicMessage,
    AnthropicRequest, AnthropicResponse, AnthropicSseEvent, AnthropicUsage, CacheControlDirective,
    SystemBlock,
};
use crate::cache::CacheControl;
use crate::error::Result;
use crate::types::{CacheBreakpoint, MessageRequest, StreamEvent};

/// Convert internal `MessageRequest` to Anthropic API request.
pub fn to_anthropic_request(req: &MessageRequest<'_>, stream: bool) -> AnthropicRequest {
    let messages = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(message_to_anthropic)
        .collect();

    let cache_system = req.cache_breakpoints.contains(&CacheBreakpoint::System);

    let system = req.system.as_ref().map(|s| {
        vec![SystemBlock {
            block_type: "text".to_string(),
            text: s.clone(),
            cache_control: if cache_system {
                Some(CacheControlDirective {
                    directive_type: CacheControl::Ephemeral.as_type_str().to_string(),
                })
            } else {
                None
            },
        }]
    });

    let mut tools = req.tools.clone();
    if req.cache_breakpoints.contains(&CacheBreakpoint::Tools) {
        // Attach cache_control to the last tool definition
        if let Some(last) = tools.last_mut()
            && let Some(obj) = last.as_object_mut()
        {
            obj.insert(
                "cache_control".to_string(),
                serde_json::json!({"type": CacheControl::Ephemeral.as_type_str()}),
            );
        }
    }

    AnthropicRequest {
        model: req.model.0.clone(),
        messages,
        max_tokens: req.max_tokens,
        system,
        temperature: req.temperature,
        tools,
        stream,
    }
}

/// Convert an internal `Message` to Anthropic format.
fn message_to_anthropic(msg: &Message) -> AnthropicMessage {
    let role = match msg.role {
        Role::User | Role::System => "user",
        Role::Assistant => "assistant",
    };

    let content = msg.content.iter().map(content_block_to_anthropic).collect();

    AnthropicMessage {
        role: role.to_string(),
        content,
    }
}

/// Convert an internal `ContentBlock` to Anthropic format.
fn content_block_to_anthropic(block: &ContentBlock) -> AnthropicContentBlock {
    match block {
        ContentBlock::Text { text } => AnthropicContentBlock::Text { text: text.clone() },
        ContentBlock::ToolUse { id, name, input } => AnthropicContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => AnthropicContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        },
        ContentBlock::Image { source } => AnthropicContentBlock::Image {
            source: AnthropicImageSource {
                source_type: source.source_type.clone(),
                media_type: source.media_type.clone(),
                data: source.data.clone(),
            },
        },
    }
}

/// Convert Anthropic non-streaming response to internal types.
///
/// # Errors
///
/// Returns `ApiError` if response parsing fails.
pub fn from_anthropic_response(resp: AnthropicResponse) -> Result<(Message, TokenUsage)> {
    let content = resp
        .content
        .into_iter()
        .map(content_block_from_anthropic)
        .collect();

    let message = Message {
        role: Role::Assistant,
        content,
    };

    let usage = from_anthropic_usage(&resp.usage);

    Ok((message, usage))
}

/// Convert Anthropic content block to internal format.
fn content_block_from_anthropic(block: AnthropicContentBlock) -> ContentBlock {
    match block {
        AnthropicContentBlock::Text { text } => ContentBlock::Text { text },
        AnthropicContentBlock::ToolUse { id, name, input } => {
            ContentBlock::ToolUse { id, name, input }
        }
        AnthropicContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        },
        AnthropicContentBlock::Image { source } => ContentBlock::Image {
            source: ImageSource {
                source_type: source.source_type,
                media_type: source.media_type,
                data: source.data,
            },
        },
    }
}

/// Convert Anthropic usage to internal `TokenUsage`.
pub const fn from_anthropic_usage(usage: &AnthropicUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
    }
}

/// Convert Anthropic SSE event to internal `StreamEvent`.
///
/// Returns `None` for events that have no internal equivalent (e.g. `Ping`).
pub fn sse_event_to_stream_event(event: AnthropicSseEvent) -> Option<StreamEvent> {
    match event {
        AnthropicSseEvent::MessageStart { message } => Some(StreamEvent::MessageStart {
            id: message.id,
            usage: from_anthropic_usage(&message.usage),
        }),
        AnthropicSseEvent::ContentBlockStart {
            index,
            content_block,
        } => Some(StreamEvent::ContentBlockStart {
            index,
            content_type: content_block.block_type,
        }),
        AnthropicSseEvent::ContentBlockDelta { index, delta } => {
            let text = match delta {
                AnthropicDelta::TextDelta { text } => text,
                AnthropicDelta::InputJsonDelta { partial_json } => partial_json,
            };
            Some(StreamEvent::ContentDelta { index, delta: text })
        }
        AnthropicSseEvent::ContentBlockStop { index } => {
            Some(StreamEvent::ContentBlockStop { index })
        }
        AnthropicSseEvent::MessageDelta { delta, usage } => Some(StreamEvent::MessageDelta {
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: usage.output_tokens,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            stop_reason: delta.stop_reason,
        }),
        AnthropicSseEvent::MessageStop => Some(StreamEvent::MessageStop),
        AnthropicSseEvent::Error { error } => Some(StreamEvent::Error {
            message: error.message,
        }),
        AnthropicSseEvent::Ping => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CacheBreakpoint;
    use crab_core::message::{ContentBlock, ImageSource, Message};
    use crab_core::model::ModelId;
    use serde_json::json;

    fn make_request() -> MessageRequest<'static> {
        MessageRequest {
            model: ModelId::from("claude-sonnet-4-20250514"),
            messages: std::borrow::Cow::Owned(vec![
                Message::user("Hello"),
                Message::assistant("Hi there!"),
            ]),
            system: Some("You are helpful.".into()),
            max_tokens: 4096,
            tools: vec![],
            temperature: Some(0.5),
            cache_breakpoints: vec![],
        }
    }

    // ─── to_anthropic_request tests ───

    #[test]
    fn to_request_basic_structure() {
        let req = make_request();
        let api_req = to_anthropic_request(&req, false);
        assert_eq!(api_req.model, "claude-sonnet-4-20250514");
        assert_eq!(api_req.max_tokens, 4096);
        assert_eq!(api_req.temperature, Some(0.5));
        assert!(!api_req.stream);
    }

    #[test]
    fn to_request_system_prompt_as_block() {
        let req = make_request();
        let api_req = to_anthropic_request(&req, false);
        let system = api_req.system.unwrap();
        assert_eq!(system.len(), 1);
        assert_eq!(system[0].block_type, "text");
        assert_eq!(system[0].text, "You are helpful.");
        assert!(system[0].cache_control.is_none());
    }

    #[test]
    fn to_request_system_cache_breakpoint() {
        let mut req = make_request();
        req.cache_breakpoints = vec![CacheBreakpoint::System];
        let api_req = to_anthropic_request(&req, true);
        let system = api_req.system.unwrap();
        assert!(system[0].cache_control.is_some());
        assert_eq!(
            system[0].cache_control.as_ref().unwrap().directive_type,
            "ephemeral"
        );
    }

    #[test]
    fn to_request_filters_system_role_from_messages() {
        let mut req = make_request();
        req.messages = std::borrow::Cow::Owned(vec![
            Message::system("This should be filtered"),
            Message::user("Hello"),
        ]);
        let api_req = to_anthropic_request(&req, false);
        // System-role messages are filtered; only user message remains
        assert_eq!(api_req.messages.len(), 1);
        assert_eq!(api_req.messages[0].role, "user");
    }

    #[test]
    fn to_request_no_system_prompt() {
        let mut req = make_request();
        req.system = None;
        let api_req = to_anthropic_request(&req, false);
        assert!(api_req.system.is_none());
    }

    #[test]
    fn to_request_tool_use_content_block() {
        let msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("Let me check."),
                ContentBlock::tool_use("toolu_01", "bash", json!({"command": "ls"})),
            ],
        );
        let req = MessageRequest {
            model: ModelId::from("claude-sonnet-4-20250514"),
            messages: std::borrow::Cow::Owned(vec![msg]),
            system: None,
            max_tokens: 1024,
            tools: vec![],
            temperature: None,
            cache_breakpoints: vec![],
        };
        let api_req = to_anthropic_request(&req, false);
        let blocks = &api_req.messages[0].content;
        assert_eq!(blocks.len(), 2);
        assert!(
            matches!(&blocks[0], AnthropicContentBlock::Text { text } if text == "Let me check.")
        );
        assert!(
            matches!(&blocks[1], AnthropicContentBlock::ToolUse { id, name, .. } if id == "toolu_01" && name == "bash")
        );
    }

    #[test]
    fn to_request_tool_result_content_block() {
        let msg = Message::tool_result("toolu_01", "file1.txt", false);
        let req = MessageRequest {
            model: ModelId::from("claude-sonnet-4-20250514"),
            messages: std::borrow::Cow::Owned(vec![msg]),
            system: None,
            max_tokens: 1024,
            tools: vec![],
            temperature: None,
            cache_breakpoints: vec![],
        };
        let api_req = to_anthropic_request(&req, false);
        let blocks = &api_req.messages[0].content;
        assert!(
            matches!(&blocks[0], AnthropicContentBlock::ToolResult { tool_use_id, is_error, .. }
                if tool_use_id == "toolu_01" && !is_error
            )
        );
    }

    #[test]
    fn to_request_image_content_block() {
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                source: ImageSource::base64("image/png", "iVBOR..."),
            }],
        );
        let req = MessageRequest {
            model: ModelId::from("claude-sonnet-4-20250514"),
            messages: std::borrow::Cow::Owned(vec![msg]),
            system: None,
            max_tokens: 1024,
            tools: vec![],
            temperature: None,
            cache_breakpoints: vec![],
        };
        let api_req = to_anthropic_request(&req, false);
        let blocks = &api_req.messages[0].content;
        assert!(matches!(&blocks[0], AnthropicContentBlock::Image { source }
            if source.media_type == "image/png" && source.source_type == "base64"
        ));
    }

    #[test]
    fn to_request_tools_cache_breakpoint() {
        let mut req = make_request();
        req.tools = vec![json!({"name": "bash", "description": "Run shell"})];
        req.cache_breakpoints = vec![CacheBreakpoint::Tools];
        let api_req = to_anthropic_request(&req, false);
        let last_tool = api_req.tools.last().unwrap();
        assert!(last_tool.get("cache_control").is_some());
        assert_eq!(last_tool["cache_control"]["type"], "ephemeral");
    }

    // ─── from_anthropic_response tests ───

    #[test]
    fn from_response_text_only() {
        let resp = AnthropicResponse {
            id: "msg_abc".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![AnthropicContentBlock::Text {
                text: "Hello!".into(),
            }],
            model: "claude-sonnet-4-20250514".into(),
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: 10,
                cache_creation_input_tokens: 5,
            },
        };
        let (msg, usage) = from_anthropic_response(resp).unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.text(), "Hello!");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 10);
        assert_eq!(usage.cache_creation_tokens, 5);
    }

    #[test]
    fn from_response_with_tool_use() {
        let resp = AnthropicResponse {
            id: "msg_tu".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![
                AnthropicContentBlock::Text {
                    text: "Let me read that.".into(),
                },
                AnthropicContentBlock::ToolUse {
                    id: "toolu_01".into(),
                    name: "read_file".into(),
                    input: json!({"path": "/tmp/test.rs"}),
                },
            ],
            model: "claude-sonnet-4-20250514".into(),
            stop_reason: Some("tool_use".into()),
            usage: AnthropicUsage {
                input_tokens: 200,
                output_tokens: 80,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };
        let (msg, _) = from_anthropic_response(resp).unwrap();
        assert!(msg.has_tool_use());
        let uses: Vec<_> = msg.tool_uses().collect();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, "read_file");
    }

    #[test]
    fn from_response_image_roundtrip() {
        let resp = AnthropicResponse {
            id: "msg_img".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![AnthropicContentBlock::Image {
                source: AnthropicImageSource {
                    source_type: "base64".into(),
                    media_type: "image/jpeg".into(),
                    data: "abc123".into(),
                },
            }],
            model: "claude-sonnet-4-20250514".into(),
            stop_reason: None,
            usage: AnthropicUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };
        let (msg, _) = from_anthropic_response(resp).unwrap();
        assert!(matches!(&msg.content[0], ContentBlock::Image { source }
            if source.media_type == "image/jpeg"
        ));
    }

    // ─── SSE event conversion tests ───

    #[test]
    fn sse_message_start() {
        let event = AnthropicSseEvent::MessageStart {
            message: super::super::types::AnthropicSseMessageStart {
                id: "msg_001".into(),
                model: "claude-sonnet-4-20250514".into(),
                usage: AnthropicUsage {
                    input_tokens: 50,
                    output_tokens: 0,
                    cache_read_input_tokens: 20,
                    cache_creation_input_tokens: 0,
                },
            },
        };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(matches!(se, StreamEvent::MessageStart { id, usage }
            if id == "msg_001" && usage.input_tokens == 50 && usage.cache_read_tokens == 20
        ));
    }

    #[test]
    fn sse_content_block_start() {
        let event = AnthropicSseEvent::ContentBlockStart {
            index: 0,
            content_block: super::super::types::AnthropicContentBlockInfo {
                block_type: "text".into(),
            },
        };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(
            matches!(se, StreamEvent::ContentBlockStart { index: 0, content_type } if content_type == "text")
        );
    }

    #[test]
    fn sse_text_delta() {
        let event = AnthropicSseEvent::ContentBlockDelta {
            index: 0,
            delta: AnthropicDelta::TextDelta {
                text: "Hello".into(),
            },
        };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(matches!(se, StreamEvent::ContentDelta { index: 0, delta } if delta == "Hello"));
    }

    #[test]
    fn sse_input_json_delta() {
        let event = AnthropicSseEvent::ContentBlockDelta {
            index: 1,
            delta: AnthropicDelta::InputJsonDelta {
                partial_json: r#"{"path":"/tmp"#.into(),
            },
        };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(
            matches!(se, StreamEvent::ContentDelta { index: 1, delta } if delta.contains("path"))
        );
    }

    #[test]
    fn sse_content_block_stop() {
        let event = AnthropicSseEvent::ContentBlockStop { index: 0 };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(matches!(se, StreamEvent::ContentBlockStop { index: 0 }));
    }

    #[test]
    fn sse_message_delta() {
        let event = AnthropicSseEvent::MessageDelta {
            delta: super::super::types::AnthropicMessageDeltaBody {
                stop_reason: Some("end_turn".into()),
            },
            usage: super::super::types::AnthropicDeltaUsage { output_tokens: 42 },
        };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(
            matches!(se, StreamEvent::MessageDelta { usage, stop_reason }
                if usage.output_tokens == 42 && stop_reason.as_deref() == Some("end_turn")
            )
        );
    }

    #[test]
    fn sse_message_stop() {
        let event = AnthropicSseEvent::MessageStop;
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(matches!(se, StreamEvent::MessageStop));
    }

    #[test]
    fn sse_error() {
        let event = AnthropicSseEvent::Error {
            error: super::super::types::AnthropicApiError {
                error_type: "overloaded_error".into(),
                message: "Overloaded".into(),
            },
        };
        let se = sse_event_to_stream_event(event).unwrap();
        assert!(matches!(se, StreamEvent::Error { message } if message == "Overloaded"));
    }

    #[test]
    fn sse_ping_returns_none() {
        let event = AnthropicSseEvent::Ping;
        assert!(sse_event_to_stream_event(event).is_none());
    }

    // ─── Usage conversion ───

    #[test]
    fn usage_conversion_all_fields() {
        let anthropic_usage = AnthropicUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: 200,
            cache_creation_input_tokens: 100,
        };
        let usage = from_anthropic_usage(&anthropic_usage);
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.cache_read_tokens, 200);
        assert_eq!(usage.cache_creation_tokens, 100);
    }
}
