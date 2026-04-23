use std::future::Future;
use std::pin::Pin;

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;

/// Tool for returning structured output validated against a JSON Schema.
///
/// Mirrors CCB's `StructuredOutput` tool. Only registered in non-interactive
/// mode when the user provides `--json-schema`. The tool's input schema is
/// replaced by the user-supplied schema, and `execute()` validates the model's
/// output against it before returning.
pub struct StructuredOutputTool {
    /// The user-supplied JSON Schema (used as `input_schema`).
    schema: Value,
    /// Pre-compiled validator for fast repeated checks.
    validator: jsonschema::Validator,
}

impl StructuredOutputTool {
    /// Create a new tool from a JSON Schema value.
    ///
    /// Returns an error if the schema itself is invalid.
    pub fn new(json_schema: Value) -> Result<Self> {
        let validator = jsonschema::validator_for(&json_schema)
            .map_err(|e| crab_core::Error::Other(format!("invalid JSON Schema: {e}")))?;
        Ok(Self {
            schema: json_schema,
            validator,
        })
    }

    /// Parse a `--json-schema` argument (file path or inline JSON string) into
    /// a `Value`, then build the tool.
    pub fn from_arg(arg: &str) -> Result<Self> {
        let value = if arg.trim_start().starts_with('{') {
            // Inline JSON
            serde_json::from_str(arg)
                .map_err(|e| crab_core::Error::Other(format!("invalid inline JSON Schema: {e}")))?
        } else {
            // File path
            let content = std::fs::read_to_string(arg).map_err(|e| {
                crab_core::Error::Other(format!("failed to read JSON Schema file '{arg}': {e}"))
            })?;
            serde_json::from_str(&content).map_err(|e| {
                crab_core::Error::Other(format!("invalid JSON in schema file '{arg}': {e}"))
            })?
        };
        Self::new(value)
    }
}

impl Tool for StructuredOutputTool {
    fn name(&self) -> &'static str {
        "StructuredOutput"
    }

    fn description(&self) -> &'static str {
        "Return structured output in the requested format"
    }

    fn input_schema(&self) -> Value {
        self.schema.clone()
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate input against the user-supplied schema
            let errors: Vec<String> = self
                .validator
                .iter_errors(&input)
                .map(|e| {
                    let path = e.instance_path().to_string();
                    let loc = if path.is_empty() {
                        "root".to_string()
                    } else {
                        path
                    };
                    format!("{loc}: {e}")
                })
                .collect();

            if !errors.is_empty() {
                let msg = format!(
                    "Output does not match required schema: {}",
                    errors.join(", ")
                );
                return Ok(ToolOutput::error(msg));
            }

            Ok(ToolOutput::success(
                "Structured output provided successfully",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// System prompt appendix instructing the model to use `StructuredOutput`.
pub const STRUCTURED_OUTPUT_PROMPT: &str = "\n\n\
Use the `StructuredOutput` tool to return your final response in the requested \
structured format. You MUST call this tool exactly once at the end of your \
response to provide the structured output.";

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::PermissionPolicy;
    use std::path::PathBuf;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn tool_name_matches_ccb() {
        let tool = StructuredOutputTool::new(serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" }
            },
            "required": ["title"]
        }))
        .unwrap();

        assert_eq!(tool.name(), "StructuredOutput");
        assert!(tool.is_read_only());
    }

    #[test]
    fn invalid_schema_rejected() {
        // An invalid schema (type must be a string, not a number)
        let result = StructuredOutputTool::new(serde_json::json!({
            "type": 42
        }));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn valid_input_succeeds() {
        let tool = StructuredOutputTool::new(serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        }))
        .unwrap();

        let ctx = test_ctx();
        let output = tool
            .execute(serde_json::json!({"name": "Alice"}), &ctx)
            .await
            .unwrap();

        assert!(!output.is_error);
        assert!(output.text().contains("successfully"));
    }

    #[tokio::test]
    async fn invalid_input_returns_error() {
        let tool = StructuredOutputTool::new(serde_json::json!({
            "type": "object",
            "properties": {
                "age": { "type": "integer" }
            },
            "required": ["age"]
        }))
        .unwrap();

        let ctx = test_ctx();
        let output = tool
            .execute(serde_json::json!({"age": "not a number"}), &ctx)
            .await
            .unwrap();

        assert!(output.is_error);
        assert!(output.text().contains("does not match"));
    }

    #[tokio::test]
    async fn missing_required_field_returns_error() {
        let tool = StructuredOutputTool::new(serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" }
            },
            "required": ["title"]
        }))
        .unwrap();

        let ctx = test_ctx();
        let output = tool.execute(serde_json::json!({}), &ctx).await.unwrap();

        assert!(output.is_error);
        assert!(output.text().contains("does not match"));
    }

    #[test]
    fn from_arg_inline_json() {
        let tool = StructuredOutputTool::from_arg(
            r#"{"type":"object","properties":{"x":{"type":"number"}}}"#,
        )
        .unwrap();
        assert_eq!(tool.name(), "StructuredOutput");
    }
}
