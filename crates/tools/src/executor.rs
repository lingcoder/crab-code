use std::sync::Arc;

use crab_core::permission::PermissionDecision;
use crab_core::tool::{ToolContext, ToolOutput};

use crate::permission::check_permission;
use crate::registry::ToolRegistry;

/// Unified tool executor with permission checks.
///
/// Wraps a `ToolRegistry` and enforces the permission decision matrix
/// before delegating to the tool's `execute()` method.
pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
}

impl ToolExecutor {
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Returns a reference to the underlying registry.
    #[must_use]
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Execute a tool by name with full permission checks.
    ///
    /// Permission decision matrix (mode x `tool_type` x `path_scope`):
    ///
    /// | PermissionMode | read_only | write(project) | write(outside) | dangerous | mcp_external | denied_list |
    /// |----------------|-----------|----------------|----------------|-----------|--------------|-------------|
    /// | Default        | Allow     | Prompt         | Prompt         | Prompt    | Prompt       | Deny        |
    /// | TrustProject   | Allow     | Allow          | Prompt         | Prompt    | Prompt       | Deny        |
    /// | Dangerously    | Allow     | Allow          | Allow          | Allow     | Allow        | Deny        |
    pub async fn execute(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> crab_common::Result<ToolOutput> {
        let tool = self.registry.get(tool_name).ok_or_else(|| {
            crab_common::Error::Other(format!("tool not found: {tool_name}"))
        })?;

        let decision = check_permission(
            &ctx.permission_policy,
            tool_name,
            &tool.source(),
            tool.is_read_only(),
            &input,
            &ctx.working_dir,
        );

        match decision {
            PermissionDecision::Allow => tool.execute(input, ctx).await,
            PermissionDecision::Deny(reason) => Ok(ToolOutput::error(reason)),
            PermissionDecision::AskUser(_prompt) => {
                // TODO: send PermissionRequest event via channel, await user response.
                // Will be wired to TUI permission dialog in a future milestone.
                // For now, auto-allow to unblock development.
                tool.execute(input, ctx).await
            }
        }
    }

    /// Execute a tool without any permission checks.
    ///
    /// Used internally by sub-agents that inherit parent permissions.
    pub async fn execute_unchecked(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> crab_common::Result<ToolOutput> {
        let tool = self.registry.get(tool_name).ok_or_else(|| {
            crab_common::Error::Other(format!("tool not found: {tool_name}"))
        })?;
        tool.execute(input, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use crab_core::tool::{Tool, ToolOutput};
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;
    use tokio_util::sync::CancellationToken;

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes input"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            input: Value,
            _ctx: &ToolContext,
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<ToolOutput>> + Send + '_>> {
            Box::pin(async move {
                let text = input
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no input");
                Ok(ToolOutput::success(text))
            })
        }
        fn is_read_only(&self) -> bool {
            true
        }
    }

    fn make_ctx(mode: PermissionMode) -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: mode,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
        }
    }

    fn make_executor() -> ToolExecutor {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(EchoTool));
        ToolExecutor::new(Arc::new(reg))
    }

    #[tokio::test]
    async fn execute_existing_tool() {
        let executor = make_executor();
        let ctx = make_ctx(PermissionMode::Default);
        let input = serde_json::json!({"text": "hello"});
        let output = executor.execute("echo", input, &ctx).await.unwrap();
        assert!(!output.is_error);
        assert_eq!(output.text(), "hello");
    }

    #[tokio::test]
    async fn execute_missing_tool() {
        let executor = make_executor();
        let ctx = make_ctx(PermissionMode::Default);
        let result = executor
            .execute("nonexistent", serde_json::json!({}), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn denied_tool_blocked() {
        let executor = make_executor();
        let mut ctx = make_ctx(PermissionMode::Dangerously);
        ctx.permission_policy.denied_tools = vec!["echo".into()];
        let output = executor
            .execute("echo", serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("denied"));
    }

    #[tokio::test]
    async fn dangerously_mode_allows() {
        let executor = make_executor();
        let ctx = make_ctx(PermissionMode::Dangerously);
        let output = executor
            .execute("echo", serde_json::json!({"text": "ok"}), &ctx)
            .await
            .unwrap();
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn execute_unchecked_works() {
        let executor = make_executor();
        let ctx = make_ctx(PermissionMode::Default);
        let output = executor
            .execute_unchecked("echo", serde_json::json!({"text": "raw"}), &ctx)
            .await
            .unwrap();
        assert_eq!(output.text(), "raw");
    }
}
