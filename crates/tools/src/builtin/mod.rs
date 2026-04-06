pub mod agent;
pub mod ask_user;
pub mod bash;
pub mod bash_security;
pub mod cron;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod image_read;
pub mod lsp;
pub mod mcp_tool;
pub mod notebook;
pub mod plan_approval;
pub mod plan_file;
pub mod plan_mode;
#[cfg(target_os = "windows")]
pub mod powershell;
pub mod read;
pub mod read_enhanced;
pub mod remote_trigger;
pub mod task;
pub mod team;
pub mod web_cache;
pub mod web_fetch;
pub mod web_formatter;
pub mod web_search;
pub mod worktree;
pub mod write;

use std::sync::Arc;

use crate::registry::ToolRegistry;

/// Register all built-in tools with the given registry.
///
/// Accepts an optional shared task store. If `None`, a new one is created.
pub fn register_all_builtins(
    registry: &mut ToolRegistry,
    task_store: Option<task::SharedTaskStore>,
) {
    let store = task_store.unwrap_or_else(task::shared_task_store);

    registry.register(Arc::new(bash::BashTool));
    registry.register(Arc::new(read::ReadTool));
    registry.register(Arc::new(write::WriteTool));
    registry.register(Arc::new(edit::EditTool));
    registry.register(Arc::new(glob::GlobTool));
    registry.register(Arc::new(grep::GrepTool));
    registry.register(Arc::new(notebook::NotebookTool));
    registry.register(Arc::new(notebook::NotebookReadTool));
    registry.register(Arc::new(lsp::LspTool));
    registry.register(Arc::new(agent::AgentTool));
    registry.register(Arc::new(web_search::WebSearchTool));
    registry.register(Arc::new(web_fetch::WebFetchTool));
    registry.register(Arc::new(ask_user::AskUserQuestionTool));
    registry.register(Arc::new(plan_mode::EnterPlanModeTool));
    registry.register(Arc::new(plan_mode::ExitPlanModeTool));
    registry.register(Arc::new(image_read::ImageReadTool));
    registry.register(Arc::new(task::TaskCreateTool::new(Arc::clone(&store))));
    registry.register(Arc::new(task::TaskListTool::new(Arc::clone(&store))));
    registry.register(Arc::new(task::TaskUpdateTool::new(Arc::clone(&store))));
    registry.register(Arc::new(task::TaskGetTool::new(store)));
    registry.register(Arc::new(worktree::EnterWorktreeTool));
    registry.register(Arc::new(worktree::ExitWorktreeTool));
    registry.register(Arc::new(team::TeamCreateTool));
    registry.register(Arc::new(team::TeamDeleteTool));
    registry.register(Arc::new(team::SendMessageTool));
    registry.register(Arc::new(task::TaskStopTool));
    registry.register(Arc::new(task::TaskOutputTool));

    let cron_store = cron::shared_cron_store();
    registry.register(Arc::new(cron::CronCreateTool::new(Arc::clone(&cron_store))));
    registry.register(Arc::new(cron::CronDeleteTool::new(Arc::clone(&cron_store))));
    registry.register(Arc::new(cron::CronListTool::new(cron_store)));

    let trigger_store = remote_trigger::shared_trigger_store();
    registry.register(Arc::new(remote_trigger::RemoteTriggerTool::new(
        trigger_store,
    )));

    // PowerShell tool — registered on Windows only
    #[cfg(target_os = "windows")]
    registry.register(Arc::new(powershell::PowerShellTool));
}

/// Create a `ToolRegistry` pre-populated with all built-in tools.
#[must_use]
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_all_builtins(&mut registry, None);
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
        assert!(registry.get("notebook_read").is_some());
        assert!(registry.get("lsp").is_some());
        assert!(registry.get("web_search").is_some());
        assert!(registry.get("web_fetch").is_some());
        assert!(registry.get("ask_user").is_some());
        assert!(registry.get("enter_plan_mode").is_some());
        assert!(registry.get("exit_plan_mode").is_some());
        assert!(registry.get("image_read").is_some());
        assert!(registry.get("task_create").is_some());
        assert!(registry.get("task_list").is_some());
        assert!(registry.get("task_update").is_some());
        assert!(registry.get("task_get").is_some());
        assert!(registry.get("enter_worktree").is_some());
        assert!(registry.get("exit_worktree").is_some());
        assert!(registry.get("team_create").is_some());
        assert!(registry.get("team_delete").is_some());
        assert!(registry.get("send_message").is_some());
        assert!(registry.get("task_stop").is_some());
        assert!(registry.get("task_output").is_some());
        assert!(registry.get("cron_create").is_some());
        assert!(registry.get("cron_delete").is_some());
        assert!(registry.get("cron_list").is_some());
        assert!(registry.get("remote_trigger").is_some());

        // PowerShell tool — only on Windows
        if cfg!(windows) {
            assert!(registry.get("powershell").is_some());
        }
    }

    #[test]
    fn default_registry_has_expected_tool_count() {
        let registry = create_default_registry();
        let expected = if cfg!(windows) { 32 } else { 31 };
        assert_eq!(registry.len(), expected);
    }

    #[test]
    fn all_tools_have_schemas() {
        let registry = create_default_registry();
        let schemas = registry.tool_schemas();
        let expected = if cfg!(windows) { 32 } else { 31 };
        assert_eq!(schemas.len(), expected);
        for schema in &schemas {
            assert!(schema.get("name").is_some());
            assert!(schema.get("description").is_some());
            assert!(schema.get("input_schema").is_some());
        }
    }
}
