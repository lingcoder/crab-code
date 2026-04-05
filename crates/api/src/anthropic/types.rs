//! Anthropic Messages API native request/response types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Anthropic Messages API request body.
#[derive(Debug, Clone, Serialize)]
pub struct AnthropicRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
}

/// System prompt block (supports `cache_control`).
#[derive(Debug, Clone, Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlDirective>,
}

/// Cache control directive for Anthropic prompt caching.
#[derive(Debug, Clone, Serialize)]
pub struct CacheControlDirective {
    #[serde(rename = "type")]
    pub directive_type: String,
}

/// A message in Anthropic format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Vec<AnthropicContentBlock>,
}

/// Content block in Anthropic format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    Image {
        source: AnthropicImageSource,
    },
}

/// Image source in Anthropic format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// Anthropic Messages API response (non-streaming).
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: String,
    pub content: Vec<AnthropicContentBlock>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

/// Token usage in Anthropic format.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

// ─── SSE event types ───

/// Anthropic SSE event wrapper.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicSseEvent {
    MessageStart {
        message: AnthropicSseMessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: AnthropicContentBlockInfo,
    },
    ContentBlockDelta {
        index: usize,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: AnthropicMessageDeltaBody,
        usage: AnthropicDeltaUsage,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicApiError,
    },
}

/// Message start payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicSseMessageStart {
    pub id: String,
    pub model: String,
    pub usage: AnthropicUsage,
}

/// Content block info at start.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicContentBlockInfo {
    #[serde(rename = "type")]
    pub block_type: String,
}

/// Delta payload for content block.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

/// Message delta body.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDeltaBody {
    pub stop_reason: Option<String>,
}

/// Usage in `message_delta` event.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicDeltaUsage {
    pub output_tokens: u64,
}

/// Anthropic API error payload.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicApiError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn message_start_event_deserde() {
        let json = json!({
            "type": "message_start",
            "message": {
                "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
                "model": "claude-sonnet-4-20250514",
                "usage": {
                    "input_tokens": 25,
                    "output_tokens": 1,
                    "cache_read_input_tokens": 0,
                    "cache_creation_input_tokens": 0
                }
            }
        });
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, AnthropicSseEvent::MessageStart { message }
            if message.id == "msg_01XFDUDYJgAACzvnptvVoYEL"
               && message.usage.input_tokens == 25
        ));
    }

    #[test]
    fn content_block_start_event_deserde() {
        let json = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text"}
        });
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(
            matches!(event, AnthropicSseEvent::ContentBlockStart { index: 0, content_block }
                if content_block.block_type == "text"
            )
        );
    }

    #[test]
    fn content_block_delta_text_deserde() {
        let json = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"}
        });
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(
            matches!(event, AnthropicSseEvent::ContentBlockDelta { index: 0, delta: AnthropicDelta::TextDelta { text } }
                if text == "Hello"
            )
        );
    }

    #[test]
    fn content_block_delta_json_deserde() {
        let json = json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"/tmp"}
        });
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(
            matches!(event, AnthropicSseEvent::ContentBlockDelta { index: 1, delta: AnthropicDelta::InputJsonDelta { partial_json } }
                if partial_json.contains("path")
            )
        );
    }

    #[test]
    fn content_block_stop_event_deserde() {
        let json = json!({"type": "content_block_stop", "index": 0});
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            event,
            AnthropicSseEvent::ContentBlockStop { index: 0 }
        ));
    }

    #[test]
    fn message_delta_event_deserde() {
        let json = json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 15}
        });
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(
            matches!(event, AnthropicSseEvent::MessageDelta { delta, usage }
                if delta.stop_reason.as_deref() == Some("end_turn") && usage.output_tokens == 15
            )
        );
    }

    #[test]
    fn message_stop_event_deserde() {
        let json = json!({"type": "message_stop"});
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, AnthropicSseEvent::MessageStop));
    }

    #[test]
    fn ping_event_deserde() {
        let json = json!({"type": "ping"});
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, AnthropicSseEvent::Ping));
    }

    #[test]
    fn error_event_deserde() {
        let json = json!({
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": "Overloaded"
            }
        });
        let event: AnthropicSseEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, AnthropicSseEvent::Error { error }
            if error.error_type == "overloaded_error" && error.message == "Overloaded"
        ));
    }

    #[test]
    fn content_block_text_serde_roundtrip() {
        let block = AnthropicContentBlock::Text {
            text: "hello world".into(),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello world");
        let parsed: AnthropicContentBlock = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, AnthropicContentBlock::Text { text } if text == "hello world"));
    }

    #[test]
    fn content_block_tool_use_serde_roundtrip() {
        let block = AnthropicContentBlock::ToolUse {
            id: "toolu_01".into(),
            name: "bash".into(),
            input: json!({"command": "ls -la"}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["name"], "bash");
        let parsed: AnthropicContentBlock = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, AnthropicContentBlock::ToolUse { name, .. } if name == "bash"));
    }

    #[test]
    fn content_block_tool_result_serde_roundtrip() {
        let block = AnthropicContentBlock::ToolResult {
            tool_use_id: "toolu_01".into(),
            content: "output here".into(),
            is_error: true,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["is_error"], true);
        let parsed: AnthropicContentBlock = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            AnthropicContentBlock::ToolResult { is_error: true, .. }
        ));
    }

    #[test]
    fn tool_result_is_error_defaults_false() {
        let json = json!({
            "type": "tool_result",
            "tool_use_id": "toolu_01",
            "content": "ok"
        });
        let block: AnthropicContentBlock = serde_json::from_value(json).unwrap();
        assert!(matches!(
            block,
            AnthropicContentBlock::ToolResult {
                is_error: false,
                ..
            }
        ));
    }

    #[test]
    fn anthropic_response_deserde() {
        let json = json!({
            "id": "msg_01abc",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hi!"}],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });
        let resp: AnthropicResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.id, "msg_01abc");
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.usage.input_tokens, 10);
        // cache fields default to 0
        assert_eq!(resp.usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn anthropic_usage_with_cache_tokens() {
        let json = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 80,
            "cache_creation_input_tokens": 20
        });
        let usage: AnthropicUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.cache_read_input_tokens, 80);
        assert_eq!(usage.cache_creation_input_tokens, 20);
    }
}
