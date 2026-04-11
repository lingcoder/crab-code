//! Shared text utilities for TUI components.

/// Strip tool-call JSON that some models (`DeepSeek`) output as assistant text.
///
/// Handles two cases:
/// 1. Trailing JSON after real text: `"hello {\"tool\":1}"` → `"hello"`
/// 2. Entire text is JSON tool params: `"{\"command\":\"ls\"}"` → `""` (empty)
pub fn strip_trailing_tool_json(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        // Entire text looks like JSON — check if it parses as a JSON object
        // with typical tool parameter keys
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok_and(|v| v.is_object()) {
            return String::new();
        }
    }
    // Check for trailing JSON after real text
    if let Some(brace_start) = trimmed.rfind('{')
        && trimmed.ends_with('}')
    {
        let before = trimmed[..brace_start].trim_end();
        if !before.is_empty() {
            return before.to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_trailing_tool_json_works() {
        // Trailing JSON after real text → strip the JSON
        assert_eq!(
            strip_trailing_tool_json("hello world {\"tool\": true}"),
            "hello world"
        );
        // No JSON → unchanged
        assert_eq!(strip_trailing_tool_json("no json here"), "no json here");
        // Entire text is a JSON object → strip it (redundant tool params)
        assert_eq!(strip_trailing_tool_json("{\"only_json\": true}"), "");
        // Non-object JSON → unchanged
        assert_eq!(strip_trailing_tool_json("[1, 2, 3]"), "[1, 2, 3]");
    }
}
