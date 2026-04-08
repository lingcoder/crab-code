pub mod agent;
pub mod ask_user;
pub mod bash;
pub mod bash_classifier;
pub mod bash_security;
pub mod brief;
pub mod config_tool;
pub mod cron;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod image_read;
pub mod lsp;
pub mod mcp_auth;
pub mod mcp_resource;
pub mod mcp_tool;
pub mod monitor;
pub mod notebook;
pub mod plan_approval;
pub mod plan_file;
pub mod plan_mode;
#[cfg(target_os = "windows")]
pub mod powershell;
pub mod read;
pub mod read_enhanced;
pub mod remote_trigger;
pub mod send_message;
pub mod send_user_file;
pub mod skill;
pub mod sleep;
pub mod snip;
pub mod task;
pub mod team;
pub mod todo_write;
pub mod tool_search;
pub mod verify_plan;
pub mod web_browser;
pub mod web_cache;
pub mod web_fetch;
pub mod web_formatter;
pub mod web_search;
pub mod workflow;
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

    // P1 tools
    registry.register(Arc::new(config_tool::ConfigTool));
    registry.register(Arc::new(brief::BriefTool));
    registry.register(Arc::new(sleep::SleepTool));
    registry.register(Arc::new(snip::SnipTool));
    registry.register(Arc::new(todo_write::TodoWriteTool));
    registry.register(Arc::new(tool_search::ToolSearchTool));
    registry.register(Arc::new(verify_plan::VerifyPlanExecutionTool));
    registry.register(Arc::new(mcp_resource::ListMcpResourcesTool));
    registry.register(Arc::new(mcp_resource::ReadMcpResourceTool));
    registry.register(Arc::new(mcp_auth::McpAuthTool));

    // P2 tools
    registry.register(Arc::new(web_browser::WebBrowserTool));
    registry.register(Arc::new(workflow::WorkflowTool));
    registry.register(Arc::new(monitor::MonitorTool));
    registry.register(Arc::new(send_user_file::SendUserFileTool));

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
        // Verify key tools are present by canonical name.
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_some());
        assert!(registry.get("Edit").is_some());
        assert!(registry.get("Glob").is_some());
        assert!(registry.get("Grep").is_some());
        assert!(registry.get("Agent").is_some());
        assert!(registry.get("NotebookEdit").is_some());
        assert!(registry.get("NotebookRead").is_some());
        assert!(registry.get("LSP").is_some());
        assert!(registry.get("WebSearch").is_some());
        assert!(registry.get("WebFetch").is_some());
        assert!(registry.get("AskUserQuestion").is_some());
        assert!(registry.get("EnterPlanMode").is_some());
        assert!(registry.get("ExitPlanMode").is_some());
        assert!(registry.get("ImageRead").is_some());
        assert!(registry.get("TaskCreate").is_some());
        assert!(registry.get("TaskList").is_some());
        assert!(registry.get("TaskUpdate").is_some());
        assert!(registry.get("TaskGet").is_some());
        assert!(registry.get("EnterWorktree").is_some());
        assert!(registry.get("ExitWorktree").is_some());
        assert!(registry.get("TeamCreate").is_some());
        assert!(registry.get("TeamDelete").is_some());
        assert!(registry.get("SendMessage").is_some());
        assert!(registry.get("TaskStop").is_some());
        assert!(registry.get("TaskOutput").is_some());
        assert!(registry.get("CronCreate").is_some());
        assert!(registry.get("CronDelete").is_some());
        assert!(registry.get("CronList").is_some());
        assert!(registry.get("RemoteTrigger").is_some());

        // P1 tools
        assert!(registry.get("Config").is_some());
        assert!(registry.get("Brief").is_some());
        assert!(registry.get("Sleep").is_some());
        assert!(registry.get("Snip").is_some());
        assert!(registry.get("TodoWrite").is_some());
        assert!(registry.get("ToolSearch").is_some());
        assert!(registry.get("VerifyPlanExecution").is_some());
        assert!(registry.get("ListMcpResources").is_some());
        assert!(registry.get("ReadMcpResource").is_some());
        assert!(registry.get("McpAuth").is_some());

        // PowerShell tool — only on Windows
        if cfg!(windows) {
            assert!(registry.get("PowerShell").is_some());
        }

        // P2 tools
        assert!(registry.get("WebBrowser").is_some());
        assert!(registry.get("Workflow").is_some());
        assert!(registry.get("Monitor").is_some());
        assert!(registry.get("SendUserFile").is_some());
    }

    #[test]
    fn default_registry_has_expected_tool_count() {
        let registry = create_default_registry();
        let expected = if cfg!(windows) { 46 } else { 45 };
        assert_eq!(registry.len(), expected);
    }

    #[test]
    fn all_tools_have_schemas() {
        let registry = create_default_registry();
        let schemas = registry.tool_schemas();
        let expected = if cfg!(windows) { 46 } else { 45 };
        assert_eq!(schemas.len(), expected);
        for schema in &schemas {
            assert!(schema.get("name").is_some());
            assert!(schema.get("description").is_some());
            assert!(schema.get("input_schema").is_some());
        }
    }
}
