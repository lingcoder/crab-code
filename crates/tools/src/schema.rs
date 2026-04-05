/// Convert internal tool schemas to the Anthropic API `tools` parameter format.
///
/// Input: `[{name, description, input_schema}, ...]`
/// Output: `[{name, description, input_schema}, ...]` (Anthropic uses this directly)
#[must_use]
pub fn to_anthropic_tools(tool_schemas: &[serde_json::Value]) -> Vec<serde_json::Value> {
    tool_schemas.to_vec()
}

/// Convert internal tool schemas to the `OpenAI` API `tools` parameter format.
///
/// Input: `[{name, description, input_schema}, ...]`
/// Output: `[{type: "function", function: {name, description, parameters}}, ...]`
#[must_use]
pub fn to_openai_tools(tool_schemas: &[serde_json::Value]) -> Vec<serde_json::Value> {
    tool_schemas
        .iter()
        .map(|schema| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": schema.get("name").cloned().unwrap_or_default(),
                    "description": schema.get("description").cloned().unwrap_or_default(),
                    "parameters": schema.get("input_schema").cloned().unwrap_or_default(),
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schemas() -> Vec<serde_json::Value> {
        vec![serde_json::json!({
            "name": "read",
            "description": "read a file",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }
        })]
    }

    #[test]
    fn anthropic_format_passthrough() {
        let schemas = sample_schemas();
        let result = to_anthropic_tools(&schemas);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "read");
        assert!(result[0].get("input_schema").is_some());
    }

    #[test]
    fn openai_format_wraps_function() {
        let schemas = sample_schemas();
        let result = to_openai_tools(&schemas);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "read");
        assert_eq!(result[0]["function"]["parameters"]["type"], "object");
    }
}
