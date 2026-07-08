//! Registry-population helpers for the built-in tool set.
//!
//! Lives as a sibling of `mod.rs` so that file stays a thin module tree
//! declaration.

use std::sync::Arc;

use crate::registry::ToolRegistry;

use super::{
    agent, ask_user, bash, brief, config_tool, cron, edit, glob, grep, lsp, mcp_auth, mcp_resource,
    notebook, plan_mode, read, send_message, send_user_file, sleep, snip, task, todo_write,
    tool_search, verify_plan, web_fetch, web_search, worktree, write,
};

#[cfg(target_os = "windows")]
use super::powershell;

/// Whether to expose the `PowerShell` tool to the model.
///
/// Windows-only; opt-in via `CRAB_USE_POWERSHELL_TOOL` (truthy value).
/// Default off for external users.
#[cfg(target_os = "windows")]
fn is_powershell_tool_enabled() -> bool {
    std::env::var("CRAB_USE_POWERSHELL_TOOL")
        .is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false" | "no" | "off"))
}

/// Register all built-in tools with the given registry.
///
/// Accepts an optional shared task store and cron store. When `None`, fresh
/// in-memory ones are created. The runtime passes a durable-reloaded cron
/// store (built via [`cron::shared_cron_store`]) so the cron tools and the
/// runtime's fired-prompt drain share the same store.
pub fn register_all_builtins(
    registry: &mut ToolRegistry,
    task_store: Option<task::SharedTaskStore>,
    cron_store: Option<cron::SharedCronStore>,
) {
    let store = task_store.unwrap_or_else(task::shared_task_store);

    bash::sweep_stale_task_outputs();

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
    registry.register(Arc::new(task::TaskCreateTool::new(Arc::clone(&store))));
    registry.register(Arc::new(task::TaskListTool::new(Arc::clone(&store))));
    registry.register(Arc::new(task::TaskUpdateTool::new(Arc::clone(&store))));
    registry.register(Arc::new(task::TaskGetTool::new(store)));
    registry.register(Arc::new(worktree::EnterWorktreeTool));
    registry.register(Arc::new(worktree::ExitWorktreeTool));
    registry.register(Arc::new(send_message::SendMessageTool));
    registry.register(Arc::new(task::TaskStopTool));
    registry.register(Arc::new(task::TaskOutputTool));
    registry.register_alias("TaskStop", "KillShell");
    registry.register_alias("TaskOutput", "BashOutput");
    registry.register_alias("TaskOutput", "AgentOutput");

    let cron_store = cron_store.unwrap_or_else(cron::shared_cron_store_empty);
    registry.register(Arc::new(cron::CronCreateTool::new(Arc::clone(&cron_store))));
    registry.register(Arc::new(cron::CronDeleteTool::new(Arc::clone(&cron_store))));
    registry.register(Arc::new(cron::CronListTool::new(cron_store)));

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

    registry.register(Arc::new(send_user_file::SendUserFileTool));

    // PowerShell tool — Windows only, opt-in via CRAB_USE_POWERSHELL_TOOL
    #[cfg(target_os = "windows")]
    if is_powershell_tool_enabled() {
        registry.register(Arc::new(powershell::PowerShellTool));
    }
}

/// Create a `ToolRegistry` pre-populated with all built-in tools.
#[must_use]
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_all_builtins(&mut registry, None, None);
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
        assert!(registry.get("TaskCreate").is_some());
        assert!(registry.get("TaskList").is_some());
        assert!(registry.get("TaskUpdate").is_some());
        assert!(registry.get("TaskGet").is_some());
        assert!(registry.get("EnterWorktree").is_some());
        assert!(registry.get("ExitWorktree").is_some());
        assert!(registry.get("SendMessage").is_some());
        assert!(registry.get("TaskStop").is_some());
        assert!(registry.get("TaskOutput").is_some());
        assert!(registry.get("CronCreate").is_some());
        assert!(registry.get("CronDelete").is_some());
        assert!(registry.get("CronList").is_some());

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

        assert!(registry.get("SendUserFile").is_some());
    }

    #[test]
    fn default_registry_has_expected_tool_count() {
        let registry = create_default_registry();
        // PowerShell tool is opt-in on Windows via CRAB_USE_POWERSHELL_TOOL.
        let ps_enabled = cfg!(windows)
            && std::env::var("CRAB_USE_POWERSHELL_TOOL")
                .is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false" | "no" | "off"));
        let expected = if ps_enabled { 39 } else { 38 };
        assert_eq!(registry.len(), expected);
    }

    #[test]
    fn all_tools_have_schemas() {
        let registry = create_default_registry();
        let schemas = registry.tool_schemas();
        // PowerShell tool is opt-in on Windows via CRAB_USE_POWERSHELL_TOOL.
        let ps_enabled = cfg!(windows)
            && std::env::var("CRAB_USE_POWERSHELL_TOOL")
                .is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false" | "no" | "off"));
        let expected = if ps_enabled { 39 } else { 38 };
        assert_eq!(schemas.len(), expected);
        for schema in &schemas {
            assert!(schema.get("name").is_some());
            assert!(schema.get("description").is_some());
            assert!(schema.get("input_schema").is_some());
        }
    }
}
