pub mod agent;
pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod mcp_tool;
pub mod notebook;
pub mod read;
pub mod task;
pub mod web_fetch;
pub mod web_search;
pub mod write;

use std::sync::Arc;

use crate::registry::ToolRegistry;

/// Register all built-in tools with the given registry.
pub fn register_all_builtins(registry: &mut ToolRegistry) {
    registry.register(Arc::new(bash::BashTool));
    registry.register(Arc::new(read::ReadTool));
    registry.register(Arc::new(write::WriteTool));
    registry.register(Arc::new(edit::EditTool));
    registry.register(Arc::new(glob::GlobTool));
    registry.register(Arc::new(grep::GrepTool));
    registry.register(Arc::new(notebook::NotebookTool));
    registry.register(Arc::new(agent::AgentTool));
    registry.register(Arc::new(web_search::WebSearchTool));
    registry.register(Arc::new(web_fetch::WebFetchTool));
    registry.register(Arc::new(task::TaskCreateTool));
    registry.register(Arc::new(task::TaskListTool));
    registry.register(Arc::new(task::TaskUpdateTool));
    registry.register(Arc::new(task::TaskGetTool));
}

/// Create a `ToolRegistry` pre-populated with all built-in tools.
#[must_use]
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_all_builtins(&mut registry);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_all_builtins_populates_registry() {
        let registry = create_default_registry();
        assert!(!registry.is_empty());
        // Verify key tools are present
        assert!(registry.get("bash").is_some());
        assert!(registry.get("read").is_some());
        assert!(registry.get("write").is_some());
        assert!(registry.get("edit").is_some());
        assert!(registry.get("glob").is_some());
        assert!(registry.get("grep").is_some());
        assert!(registry.get("agent").is_some());
        assert!(registry.get("notebook_edit").is_some());
        assert!(registry.get("web_search").is_some());
        assert!(registry.get("web_fetch").is_some());
        assert!(registry.get("task_create").is_some());
        assert!(registry.get("task_list").is_some());
        assert!(registry.get("task_update").is_some());
        assert!(registry.get("task_get").is_some());
    }

    #[test]
    fn default_registry_has_14_tools() {
        let registry = create_default_registry();
        assert_eq!(registry.len(), 14);
    }

    #[test]
    fn all_tools_have_schemas() {
        let registry = create_default_registry();
        let schemas = registry.tool_schemas();
        assert_eq!(schemas.len(), 14);
        for schema in &schemas {
            assert!(schema.get("name").is_some());
            assert!(schema.get("description").is_some());
            assert!(schema.get("input_schema").is_some());
        }
    }
}
