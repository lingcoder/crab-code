//! Internal unified request/response/event types (Crab Code's own data model).
//!
//! These are NOT API abstractions — each client independently converts between
//! these types and its own API-native format.

use crab_core::message::Message;
use crab_core::model::{ModelId, TokenUsage};

/// Internal message request — each client converts this to its own API format.
#[derive(Debug, Clone)]
pub struct MessageRequest<'a> {
    pub model: ModelId,
    pub messages: std::borrow::Cow<'a, [Message]>,
    pub system: Option<String>,
    pub max_tokens: u32,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
    /// Cache breakpoints — Anthropic-specific, ignored by other providers.
    pub cache_breakpoints: Vec<CacheBreakpoint>,
    /// Extended thinking budget in tokens. When > 0, Anthropic provider enables
    /// extended thinking mode. Other providers silently ignore this.
    pub budget_tokens: Option<u32>,
    /// Response format constraint (`OpenAI` only — Anthropic ignores this).
    pub response_format: Option<serde_json::Value>,
    /// Tool choice constraint (`OpenAI` only — Anthropic ignores this).
    pub tool_choice: Option<serde_json::Value>,
}

/// Specifies where to place a `cache_control: {"type": "ephemeral"}` marker.
///
/// Only meaningful for the Anthropic provider path; other providers ignore this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheBreakpoint {
    /// Place on the last system prompt block.
    System,
    /// Place on the last tool definition.
    Tools,
    /// Place on the last user message (turn boundary).
    LastMessage,
}

/// Internal unified stream event — each client maps its SSE format to this enum.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Emitted once at the start; carries initial usage (cache read/creation tokens).
    MessageStart {
        id: String,
        usage: TokenUsage,
    },
    ContentBlockStart {
        index: usize,
        content_type: String,
    },
    ContentDelta {
        index: usize,
        delta: String,
    },
    /// Incremental thinking content from extended thinking mode.
    ThinkingDelta {
        index: usize,
        delta: String,
    },
    ContentBlockStop {
        index: usize,
    },
    /// Final usage update with optional stop reason.
    MessageDelta {
        usage: TokenUsage,
        stop_reason: Option<String>,
    },
    MessageStop,
    Error {
        message: String,
    },
}

/// Non-streaming response wrapper.
#[derive(Debug, Clone)]
pub struct MessageResponse {
    pub id: String,
    pub message: Message,
    pub usage: TokenUsage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Message;
    use crab_core::model::ModelId;

    #[test]
    fn message_request_construction() {
        let req = MessageRequest {
            model: ModelId::from("claude-sonnet-4-20250514"),
            messages: std::borrow::Cow::Owned(vec![Message::user("hi")]),
            system: Some("Be helpful.".into()),
            max_tokens: 2048,
            tools: vec![],
            temperature: None,
            cache_breakpoints: vec![],
            budget_tokens: None,
            response_format: None,
            tool_choice: None,
        };
        assert_eq!(req.model.as_str(), "claude-sonnet-4-20250514");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.system.as_deref(), Some("Be helpful."));
    }

    #[test]
    fn cache_breakpoint_equality() {
        assert_eq!(CacheBreakpoint::System, CacheBreakpoint::System);
        assert_ne!(CacheBreakpoint::System, CacheBreakpoint::Tools);
        assert_ne!(CacheBreakpoint::Tools, CacheBreakpoint::LastMessage);
    }

    #[test]
    fn stream_event_message_start() {
        let event = StreamEvent::MessageStart {
            id: "msg_01".into(),
            usage: TokenUsage::default(),
        };
        assert!(matches!(event, StreamEvent::MessageStart { id, .. } if id == "msg_01"));
    }

    #[test]
    fn stream_event_content_delta() {
        let event = StreamEvent::ContentDelta {
            index: 0,
            delta: "Hello".into(),
        };
        assert!(matches!(event, StreamEvent::ContentDelta { index: 0, delta } if delta == "Hello"));
    }

    #[test]
    fn stream_event_error() {
        let event = StreamEvent::Error {
            message: "rate limited".into(),
        };
        assert!(matches!(event, StreamEvent::Error { message } if message.contains("rate")));
    }

    #[test]
    fn stream_event_message_stop() {
        let event = StreamEvent::MessageStop;
        assert!(matches!(event, StreamEvent::MessageStop));
    }

    #[test]
    fn message_response_construction() {
        let resp = MessageResponse {
            id: "msg_01".into(),
            message: Message::assistant("Hello!"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        };
        assert_eq!(resp.id, "msg_01");
        assert_eq!(resp.message.text(), "Hello!");
        assert_eq!(resp.usage.total(), 15);
    }
}
