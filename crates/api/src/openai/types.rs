//! OpenAI-compatible Chat Completions API native request/response types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Chat Completions request body.
#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
    /// Controls the output format of the model response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Controls which tool (if any) the model should call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

/// A message in Chat Completions format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Tool call in Chat Completions format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// Function call details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Chat Completions response (non-streaming).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Option<CompletionUsage>,
}

/// A choice in the response.
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub index: usize,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

/// Token usage in Chat Completions format.
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

// ─── SSE streaming types ───

/// SSE chunk (`data: {...}`).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<CompletionUsage>,
}

/// A choice delta in a streaming chunk.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkChoice {
    pub index: usize,
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

/// Delta content in a streaming chunk.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// Incremental tool call in a streaming chunk.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallDelta {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionCallDelta>,
}

/// Incremental function call data.
#[derive(Debug, Clone, Deserialize)]
pub struct FunctionCallDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

// ─── Structured output: ResponseFormat ───

/// Controls the output format of the model response.
///
/// See `OpenAI` API docs: `response_format` parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ResponseFormat {
    /// Plain text output (default behavior).
    #[serde(rename = "text")]
    Text,
    /// Force the model to output valid JSON.
    #[serde(rename = "json_object")]
    JsonObject,
    /// Force the model to output JSON conforming to a specific schema.
    #[serde(rename = "json_schema")]
    JsonSchema {
        /// The JSON schema specification.
        json_schema: JsonSchemaSpec,
    },
}

/// JSON Schema specification for structured output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonSchemaSpec {
    /// A name for the schema (used in API logging/identification).
    pub name: String,
    /// Optional description of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The JSON Schema definition.
    pub schema: Value,
    /// Whether to strictly enforce the schema (default: false).
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !b
}

// ─── Tool choice ───

/// Controls which tool the model should call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ToolChoice {
    /// String values: "none", "auto", "required".
    Mode(ToolChoiceMode),
    /// Force a specific function call.
    Function {
        /// Must be "function".
        #[serde(rename = "type")]
        choice_type: String,
        /// The function to call.
        function: ToolChoiceFunction,
    },
}

/// Tool choice mode string values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolChoiceMode {
    /// Do not call any tool.
    #[serde(rename = "none")]
    None,
    /// Model decides whether and which tool to call (default).
    #[serde(rename = "auto")]
    Auto,
    /// Model must call at least one tool.
    #[serde(rename = "required")]
    Required,
}

/// Specifies which function to call when using `ToolChoice::Function`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolChoiceFunction {
    /// The name of the function to call.
    pub name: String,
}

impl ToolChoice {
    /// Create a `ToolChoice::Function` variant for a specific function name.
    #[must_use]
    pub fn function(name: impl Into<String>) -> Self {
        Self::Function {
            choice_type: "function".into(),
            function: ToolChoiceFunction { name: name.into() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ResponseFormat tests ──────────────────────────────────────────

    #[test]
    fn response_format_text_serde() {
        let fmt = ResponseFormat::Text;
        let json = serde_json::to_string(&fmt).unwrap();
        assert_eq!(json, r#"{"type":"text"}"#);
        let back: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fmt);
    }

    #[test]
    fn response_format_json_object_serde() {
        let fmt = ResponseFormat::JsonObject;
        let json = serde_json::to_string(&fmt).unwrap();
        assert_eq!(json, r#"{"type":"json_object"}"#);
        let back: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fmt);
    }

    #[test]
    fn response_format_json_schema_serde() {
        let fmt = ResponseFormat::JsonSchema {
            json_schema: JsonSchemaSpec {
                name: "weather".into(),
                description: Some("Weather data".into()),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "temperature": {"type": "number"},
                        "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
                    },
                    "required": ["temperature", "unit"]
                }),
                strict: true,
            },
        };
        let json = serde_json::to_string_pretty(&fmt).unwrap();
        assert!(json.contains(r#""type": "json_schema""#));
        assert!(json.contains(r#""name": "weather""#));
        assert!(json.contains(r#""strict": true"#));
        let back: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fmt);
    }

    #[test]
    fn response_format_json_schema_no_strict() {
        let fmt = ResponseFormat::JsonSchema {
            json_schema: JsonSchemaSpec {
                name: "test".into(),
                description: None,
                schema: serde_json::json!({"type": "object"}),
                strict: false,
            },
        };
        let json = serde_json::to_string(&fmt).unwrap();
        // strict: false should be skipped
        assert!(!json.contains("strict"));
        // description: None should be skipped
        assert!(!json.contains("description"));
    }

    // ─── ToolChoice tests ──────────────────────────────────────────────

    #[test]
    fn tool_choice_none_serde() {
        let tc = ToolChoice::Mode(ToolChoiceMode::None);
        let json = serde_json::to_string(&tc).unwrap();
        assert_eq!(json, r#""none""#);
        let back: ToolChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tc);
    }

    #[test]
    fn tool_choice_auto_serde() {
        let tc = ToolChoice::Mode(ToolChoiceMode::Auto);
        let json = serde_json::to_string(&tc).unwrap();
        assert_eq!(json, r#""auto""#);
    }

    #[test]
    fn tool_choice_required_serde() {
        let tc = ToolChoice::Mode(ToolChoiceMode::Required);
        let json = serde_json::to_string(&tc).unwrap();
        assert_eq!(json, r#""required""#);
    }

    #[test]
    fn tool_choice_function_serde() {
        let tc = ToolChoice::function("get_weather");
        let json = serde_json::to_string(&tc).unwrap();
        assert!(json.contains(r#""type":"function""#));
        assert!(json.contains(r#""name":"get_weather""#));
        let back: ToolChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tc);
    }

    #[test]
    fn tool_choice_function_helper() {
        let tc = ToolChoice::function("my_func");
        match tc {
            ToolChoice::Function {
                choice_type,
                function,
            } => {
                assert_eq!(choice_type, "function");
                assert_eq!(function.name, "my_func");
            }
            _ => panic!("expected Function variant"),
        }
    }

    // ─── Integration with ChatCompletionRequest ────────────────────────

    #[test]
    fn chat_completion_request_with_response_format() {
        let req = ChatCompletionRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some("What's the weather?".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            max_tokens: Some(256),
            temperature: None,
            tools: vec![],
            stream: false,
            response_format: None,
            tool_choice: None,
        };
        // Verify request serializes cleanly (no response_format field yet in struct)
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "gpt-4o");
    }

    #[test]
    fn json_schema_spec_equality() {
        let s1 = JsonSchemaSpec {
            name: "test".into(),
            description: None,
            schema: serde_json::json!({"type": "object"}),
            strict: false,
        };
        let s2 = s1.clone();
        assert_eq!(s1, s2);
    }

    #[test]
    fn tool_choice_mode_equality() {
        assert_eq!(ToolChoiceMode::Auto, ToolChoiceMode::Auto);
        assert_ne!(ToolChoiceMode::None, ToolChoiceMode::Auto);
        assert_ne!(ToolChoiceMode::Required, ToolChoiceMode::None);
    }
}
