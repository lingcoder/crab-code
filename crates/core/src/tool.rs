use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::permission::{PermissionMode, PermissionPolicy};
use crab_common::Result;

/// Tool source classification — determines the permission matrix column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolSource {
    /// Built-in tools (Bash/Read/Write/Edit/Glob/Grep etc.)
    BuiltIn,
    /// External MCP server tools (untrusted source, Default/TrustProject require Prompt)
    McpExternal {
        /// Name of the MCP server that provides this tool.
        server_name: String,
    },
    /// Sub-agent spawned tools (`TrustProject` auto-allows)
    AgentSpawn,
}

/// Core trait for all tools — built-in, MCP-bridged, and plugin-provided.
///
/// Returns `Pin<Box<dyn Future>>` (not `async fn`) to guarantee object safety
/// when stored as `Arc<dyn Tool>`.
pub trait Tool: Send + Sync {
    /// Unique tool identifier (e.g. "bash", "read", `mcp__server__tool`).
    fn name(&self) -> &str;

    /// Human-readable description (included in the system prompt).
    fn description(&self) -> &str;

    /// JSON Schema describing the input parameters.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given input and context.
    ///
    /// Long-running tools should periodically check `ctx.cancellation_token`
    /// and return early when cancelled.
    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>>;

    /// Tool source (default: `BuiltIn`) — affects the permission check matrix.
    fn source(&self) -> ToolSource {
        ToolSource::BuiltIn
    }

    /// Whether user confirmation is required before execution (default: false).
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// Whether this tool is read-only (read-only tools can skip confirmation).
    fn is_read_only(&self) -> bool {
        false
    }

    // ── Rendering hooks (Phase 1.5) ─────────────────────────────────
    //
    // All have default implementations so existing tools don't break.
    // Override in individual tool impls for customized TUI display.

    /// One-line summary shown when the tool is invoked.
    ///
    /// E.g. `BashTool` → `"$ ls -la"`, `ReadTool` → `"src/main.rs:1-50"`.
    /// Returns `None` to use the default `"● {tool_name}"`.
    fn format_use_summary(&self, _input: &Value) -> Option<String> {
        None
    }

    /// Custom result formatting for TUI display.
    ///
    /// Returns `None` to use the default plain-text rendering (10-line truncation).
    fn format_result(&self, _output: &ToolOutput) -> Option<ToolDisplayResult> {
        None
    }

    /// Whether the result should be collapsible in the TUI.
    ///
    /// Default: collapse when output exceeds 5 lines.
    fn is_result_collapsible(&self, output: &ToolOutput) -> bool {
        output.text().lines().count() > 5
    }

    /// Summary shown when the tool invocation was rejected by the user.
    ///
    /// E.g. `EditTool` → `"Edit rejected: src/main.rs"`.
    fn format_rejected_summary(&self, _input: &Value) -> Option<String> {
        None
    }

    /// Whether this tool supports real-time streaming progress display.
    ///
    /// E.g. `BashTool` can stream stdout as it runs.
    fn supports_streaming_progress(&self) -> bool {
        false
    }
}

/// Tool-customized rendering output for TUI display.
#[derive(Debug, Clone)]
pub struct ToolDisplayResult {
    /// Styled lines to render.
    pub lines: Vec<ToolDisplayLine>,
    /// Number of lines to show when collapsed (default 3).
    pub preview_lines: usize,
}

/// A single styled line in a tool display result.
#[derive(Debug, Clone)]
pub struct ToolDisplayLine {
    /// The text content.
    pub text: String,
    /// Optional display style. `None` uses the default style.
    pub style: Option<ToolDisplayStyle>,
}

impl ToolDisplayLine {
    /// Create a new line with the given text and style.
    pub fn new(text: impl Into<String>, style: ToolDisplayStyle) -> Self {
        Self {
            text: text.into(),
            style: Some(style),
        }
    }

    /// Create a new line with default styling.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: None,
        }
    }
}

/// Display style hint for tool output lines.
///
/// The TUI maps these to actual terminal colors/attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDisplayStyle {
    /// Normal text.
    Normal,
    /// Error text (typically red).
    Error,
    /// Diff addition (typically green).
    DiffAdd,
    /// Diff removal (typically red).
    DiffRemove,
    /// Diff context (unchanged lines).
    DiffContext,
    /// Muted / de-emphasized text (typically dim gray).
    Muted,
    /// Highlighted / emphasized text.
    Highlight,
}

/// Execution context passed to every tool invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Current working directory for the tool.
    pub working_dir: PathBuf,
    /// Permission mode in effect.
    pub permission_mode: PermissionMode,
    /// Session identifier.
    pub session_id: String,
    /// Cancellation token — long-running tools (e.g. Bash) should check this
    /// periodically and exit early when triggered.
    pub cancellation_token: CancellationToken,
    /// Permission policy (merged result from all config layers).
    pub permission_policy: PermissionPolicy,
    /// Extended context for tools that need richer application state.
    pub ext: ToolContextExt,
}

/// Extended tool context fields — populated by the agent loop when available.
///
/// These fields are optional enrichment data. Tools that need them should
/// check and degrade gracefully if they are empty.
#[derive(Debug, Clone, Default)]
pub struct ToolContextExt {
    /// Pre-rendered tool name+description pairs for `ToolSearch`.
    pub tool_descriptions: Vec<String>,
    /// Recent conversation summary for `BriefTool`.
    pub conversation_summary: Option<String>,
    /// Names of connected MCP servers for `McpAuth`/`McpResource`.
    pub mcp_server_names: Vec<String>,
}

/// A single content block within a tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputContent {
    /// Plain text output.
    Text { text: String },
    /// Base64-encoded image output.
    Image { media_type: String, data: String },
    /// Structured JSON output.
    Json { value: Value },
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Output content blocks.
    pub content: Vec<ToolOutputContent>,
    /// Whether this output represents an error.
    pub is_error: bool,
}

impl ToolOutput {
    /// Create a successful text output.
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolOutputContent::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Create an error text output.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolOutputContent::Text { text: text.into() }],
            is_error: true,
        }
    }

    /// Create an output with multiple content blocks.
    pub fn with_content(content: Vec<ToolOutputContent>, is_error: bool) -> Self {
        Self { content, is_error }
    }

    /// Get the text content, concatenating all text blocks.
    #[must_use]
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                ToolOutputContent::Text { text } => Some(text.as_str()),
                ToolOutputContent::Image { .. } | ToolOutputContent::Json { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_output_success() {
        let out = ToolOutput::success("hello");
        assert!(!out.is_error);
        assert_eq!(out.text(), "hello");
    }

    #[test]
    fn tool_output_error() {
        let out = ToolOutput::error("oops");
        assert!(out.is_error);
        assert_eq!(out.text(), "oops");
    }

    #[test]
    fn tool_output_text_concatenates() {
        let out = ToolOutput::with_content(
            vec![
                ToolOutputContent::Text { text: "a".into() },
                ToolOutputContent::Json { value: json!(42) },
                ToolOutputContent::Text { text: "b".into() },
            ],
            false,
        );
        assert_eq!(out.text(), "ab");
    }

    #[test]
    fn tool_output_empty_content() {
        let out = ToolOutput::with_content(vec![], false);
        assert_eq!(out.text(), "");
        assert!(!out.is_error);
    }

    #[test]
    fn tool_output_serde_roundtrip() {
        let out = ToolOutput::success("test output");
        let json = serde_json::to_string(&out).unwrap();
        let parsed: ToolOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.text(), "test output");
        assert!(!parsed.is_error);
    }

    #[test]
    fn tool_output_content_image() {
        let out = ToolOutput::with_content(
            vec![ToolOutputContent::Image {
                media_type: "image/png".into(),
                data: "base64data".into(),
            }],
            false,
        );
        // Image blocks don't contribute to text()
        assert_eq!(out.text(), "");
    }

    #[test]
    fn tool_source_default_is_builtin() {
        // Verify ToolSource variants construct correctly
        let src = ToolSource::BuiltIn;
        assert!(matches!(src, ToolSource::BuiltIn));

        let mcp = ToolSource::McpExternal {
            server_name: "playwright".into(),
        };
        assert!(matches!(mcp, ToolSource::McpExternal { .. }));
    }

    #[test]
    fn tool_source_serde_roundtrip() {
        let src = ToolSource::McpExternal {
            server_name: "test-server".into(),
        };
        let json = serde_json::to_string(&src).unwrap();
        let parsed: ToolSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, src);
    }

    // ─── Additional tests ───

    #[test]
    fn tool_source_all_variants_serde() {
        let variants = vec![
            ToolSource::BuiltIn,
            ToolSource::McpExternal {
                server_name: "playwright".into(),
            },
            ToolSource::AgentSpawn,
        ];
        for src in variants {
            let json = serde_json::to_string(&src).unwrap();
            let parsed: ToolSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, src);
        }
    }

    #[test]
    fn tool_output_content_text_serde() {
        let content = ToolOutputContent::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: ToolOutputContent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ToolOutputContent::Text { text } if text == "hello"));
    }

    #[test]
    fn tool_output_content_image_serde() {
        let content = ToolOutputContent::Image {
            media_type: "image/png".into(),
            data: "base64data".into(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: ToolOutputContent = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, ToolOutputContent::Image { media_type, data }
                if media_type == "image/png" && data == "base64data"
            )
        );
    }

    #[test]
    fn tool_output_content_json_serde() {
        let content = ToolOutputContent::Json {
            value: json!({"key": "value", "count": 42}),
        };
        let json_str = serde_json::to_string(&content).unwrap();
        let parsed: ToolOutputContent = serde_json::from_str(&json_str).unwrap();
        assert!(matches!(parsed, ToolOutputContent::Json { value } if value["key"] == "value"));
    }

    #[test]
    fn tool_output_success_is_not_error() {
        let out = ToolOutput::success("ok");
        assert!(!out.is_error);
        assert_eq!(out.content.len(), 1);
    }

    #[test]
    fn tool_output_error_is_error() {
        let out = ToolOutput::error("fail");
        assert!(out.is_error);
        assert_eq!(out.content.len(), 1);
    }

    #[test]
    fn tool_output_multi_content_serde_roundtrip() {
        let out = ToolOutput::with_content(
            vec![
                ToolOutputContent::Text {
                    text: "header".into(),
                },
                ToolOutputContent::Image {
                    media_type: "image/jpeg".into(),
                    data: "abc123".into(),
                },
                ToolOutputContent::Json {
                    value: json!({"status": "ok"}),
                },
            ],
            false,
        );
        let json = serde_json::to_string(&out).unwrap();
        let parsed: ToolOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content.len(), 3);
        assert!(!parsed.is_error);
        assert_eq!(parsed.text(), "header");
    }

    /// Mock tool for testing the Tool trait interface.
    struct MockTool {
        name: &'static str,
        read_only: bool,
    }

    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "A mock tool for testing"
        }

        fn input_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "arg": {"type": "string"}
                },
                "required": ["arg"]
            })
        }

        fn execute(
            &self,
            input: Value,
            _ctx: &ToolContext,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
            let text = input["arg"].as_str().unwrap_or("no arg").to_owned();
            Box::pin(async move { Ok(ToolOutput::success(text)) })
        }

        fn is_read_only(&self) -> bool {
            self.read_only
        }
    }

    #[test]
    fn mock_tool_name_and_description() {
        let tool = MockTool {
            name: "mock_read",
            read_only: true,
        };
        assert_eq!(tool.name(), "mock_read");
        assert_eq!(tool.description(), "A mock tool for testing");
        assert!(tool.is_read_only());
        assert!(!tool.requires_confirmation());
        assert!(matches!(tool.source(), ToolSource::BuiltIn));
    }

    #[test]
    fn mock_tool_input_schema_is_valid_json() {
        let tool = MockTool {
            name: "test",
            read_only: false,
        };
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["arg"].is_object());
        assert!(schema["required"].is_array());
    }

    #[test]
    fn mock_tool_is_object_safe() {
        // Verify Tool can be used as a trait object
        let tool: Box<dyn Tool> = Box::new(MockTool {
            name: "boxed",
            read_only: false,
        });
        assert_eq!(tool.name(), "boxed");
        assert!(!tool.is_read_only());
    }

    #[test]
    fn tool_context_construction() {
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: PermissionMode::Default,
            session_id: "sess_123".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: ToolContextExt::default(),
        };
        assert_eq!(ctx.working_dir, std::path::Path::new("/tmp"));
        assert_eq!(ctx.permission_mode, PermissionMode::Default);
        assert_eq!(ctx.session_id, "sess_123");
    }
}
