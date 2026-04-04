use std::collections::HashMap;
use std::sync::Arc;

use crab_core::tool::Tool;

/// Tool registry: registration, lookup, and schema generation.
///
/// Stores `Arc<dyn Tool>` instances indexed by name. Used by `ToolExecutor`
/// to look up tools at runtime, and by the API layer to generate the `tools`
/// parameter for LLM requests.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Overwrites any existing tool with the same name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// List all registered tool names (sorted for deterministic output).
    #[must_use]
    pub fn tool_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tools.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Get the internal JSON Schema representation for all registered tools.
    ///
    /// Each entry contains `name`, `description`, and `input_schema` fields.
    /// Use `schema::to_api_tools()` to convert these to the format expected
    /// by the LLM API.
    #[must_use]
    pub fn tool_schemas(&self) -> Vec<serde_json::Value> {
        let mut schemas: Vec<_> = self
            .tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect();
        // Sort by name for deterministic API requests
        schemas.sort_by(|a, b| {
            let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
            a_name.cmp(b_name)
        });
        schemas
    }

    /// Get schemas for a filtered set of tools.
    #[must_use]
    pub fn tool_schemas_filtered(&self, names: &[&str]) -> Vec<serde_json::Value> {
        names
            .iter()
            .filter_map(|name| self.tools.get(*name))
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_common::Result;
    use crab_core::tool::{ToolContext, ToolOutput};
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;

    struct DummyTool {
        tool_name: &'static str,
    }

    impl Tool for DummyTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn description(&self) -> &str {
            "a dummy tool"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _input: Value,
            _ctx: &ToolContext,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
            Box::pin(async { Ok(ToolOutput::success("ok")) })
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool { tool_name: "test" }));
        assert!(reg.get("test").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn len_and_is_empty() {
        let mut reg = ToolRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        reg.register(Arc::new(DummyTool { tool_name: "a" }));
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn tool_names_sorted() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool { tool_name: "zebra" }));
        reg.register(Arc::new(DummyTool { tool_name: "alpha" }));
        assert_eq!(reg.tool_names(), vec!["alpha", "zebra"]);
    }

    #[test]
    fn tool_schemas_sorted() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool { tool_name: "z" }));
        reg.register(Arc::new(DummyTool { tool_name: "a" }));
        let schemas = reg.tool_schemas();
        assert_eq!(schemas.len(), 2);
        assert_eq!(schemas[0]["name"], "a");
        assert_eq!(schemas[1]["name"], "z");
    }

    #[test]
    fn register_overwrites() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool { tool_name: "x" }));
        reg.register(Arc::new(DummyTool { tool_name: "x" }));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn tool_schemas_filtered() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool { tool_name: "a" }));
        reg.register(Arc::new(DummyTool { tool_name: "b" }));
        reg.register(Arc::new(DummyTool { tool_name: "c" }));
        let filtered = reg.tool_schemas_filtered(&["a", "c", "missing"]);
        assert_eq!(filtered.len(), 2);
    }
}
