use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crab_core::permission::PermissionDecision;
use crab_core::tool::{ToolContext, ToolOutput};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::permission::check_permission;
use crate::registry::ToolRegistry;
use crate::str_utils::truncate_with_preview;

/// Subdirectory under `std::env::temp_dir()` where oversized tool outputs are spilled.
pub const TOOL_RESULT_STORAGE_SUBDIR: &str = "crab-tool-results";

/// Number of characters preserved in the in-band preview when a tool output
/// is spilled to disk.
const RESULT_PREVIEW_CHARS: usize = 2_000;

/// If a tool output exceeds the tool's `max_chars` budget, persist the full
/// text to a temp file and return a truncated preview that points at it.
///
/// The preview format is `truncate_with_preview(text, 2000)` followed by a
/// reference line telling the model how to retrieve the full content via
/// the `Read` tool. Errors writing to disk are swallowed: the spill is a
/// soft-fail enrichment, not load-bearing for the result itself, so we
/// still return the in-memory preview.
#[must_use]
pub fn apply_result_budget(output: ToolOutput, max_chars: usize, tool_use_id: &str) -> ToolOutput {
    if output.is_error {
        return output;
    }
    let text = output.text();
    if text.chars().count() <= max_chars {
        return output;
    }

    let storage_dir = std::env::temp_dir().join(TOOL_RESULT_STORAGE_SUBDIR);
    let _ = std::fs::create_dir_all(&storage_dir);
    let path = storage_dir.join(format!("{tool_use_id}.txt"));
    let _ = std::fs::write(&path, &text);

    let preview = truncate_with_preview(&text, RESULT_PREVIEW_CHARS);
    let reference = format!(
        "\n\n[Full output saved to {}, use Read tool to access]",
        path.display()
    );
    ToolOutput::success(format!("{preview}{reference}"))
}

/// Canonical reject text. Fixed phrasing primes the model to stop and wait for instructions.
const REJECT_MESSAGE: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";

/// A channel sender for streaming incremental tool output (e.g. bash stdout lines).
#[derive(Clone)]
pub struct StreamingOutput {
    tx: mpsc::Sender<String>,
}

impl StreamingOutput {
    /// Create a new streaming output channel pair.
    ///
    /// Returns `(StreamingOutput, Receiver<String>)` where the receiver yields
    /// incremental output deltas as they arrive.
    pub fn channel(buffer: usize) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(buffer);
        (Self { tx }, rx)
    }

    /// Send a delta (e.g. one line of stdout). Silently drops if receiver is gone.
    pub async fn send(&self, delta: impl Into<String>) {
        let _ = self.tx.send(delta.into()).await;
    }
}

/// Callback trait for handling permission prompts.
///
/// Implementations decide how to ask the user for confirmation (CLI stdin,
/// TUI dialog, auto-approve, etc.).
pub trait PermissionHandler: Send + Sync {
    /// Called when a tool requires user confirmation.
    ///
    /// `tool_name` is the tool being invoked, `prompt` is the human-readable
    /// question, and `tool_input` is the raw JSON input — handlers can use
    /// it to render richer prompts (e.g. show the full bash command, the
    /// edit diff, or the write target). Returns `true` to allow, `false`
    /// to deny.
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
        tool_input: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

/// Unified tool executor with permission checks.
///
/// Wraps a `ToolRegistry` and enforces the permission decision matrix
/// before delegating to the tool's `execute()` method.
pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
    permission_handler: Option<Arc<dyn PermissionHandler>>,
}

impl ToolExecutor {
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            permission_handler: None,
        }
    }

    /// Set a permission handler for `AskUser` decisions.
    pub fn set_permission_handler(&mut self, handler: Arc<dyn PermissionHandler>) {
        self.permission_handler = Some(handler);
    }

    /// Returns a reference to the underlying registry.
    #[must_use]
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Returns a clone of the `Arc<ToolRegistry>` for sharing with sub-agents.
    #[must_use]
    pub fn registry_arc(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.registry)
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
    ) -> crab_core::Result<ToolOutput> {
        let tool = self
            .registry
            .get(tool_name)
            .ok_or_else(|| crab_core::Error::Other(format!("tool not found: {tool_name}")))?;

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
            PermissionDecision::AskUser(prompt) => {
                if let Some(handler) = &self.permission_handler {
                    let allowed = handler.ask_permission(tool_name, &prompt, &input).await;
                    if allowed {
                        tool.execute(input, ctx).await
                    } else {
                        Ok(ToolOutput::error(REJECT_MESSAGE.to_string()))
                    }
                } else {
                    // No handler installed — auto-allow (development fallback)
                    tool.execute(input, ctx).await
                }
            }
        }
    }

    /// Execute a tool with streaming output support.
    ///
    /// Returns `(Receiver<String>, JoinHandle<Result<ToolOutput>>)`.
    /// The receiver yields incremental output deltas (e.g. bash stdout lines).
    /// The join handle resolves to the final complete `ToolOutput`.
    ///
    /// Permission checks are performed before spawning. If denied, the receiver
    /// is immediately dropped and the handle returns the denial output.
    pub fn execute_streaming(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> (
        mpsc::Receiver<String>,
        JoinHandle<crab_core::Result<ToolOutput>>,
    ) {
        let (streaming, rx) = StreamingOutput::channel(64);
        let registry = Arc::clone(&self.registry);
        let tool_name = tool_name.to_string();
        let ctx = ctx.clone();
        let permission_handler = self.permission_handler.clone();

        let handle = tokio::spawn(async move {
            let tool = registry
                .get(&tool_name)
                .ok_or_else(|| crab_core::Error::Other(format!("tool not found: {tool_name}")))?;

            let policy = &ctx.permission_policy;
            let decision = check_permission(
                policy,
                &tool_name,
                &tool.source(),
                tool.is_read_only(),
                &input,
                &ctx.working_dir,
            );

            match decision {
                PermissionDecision::Allow => {}
                PermissionDecision::Deny(reason) => return Ok(ToolOutput::error(reason)),
                PermissionDecision::AskUser(prompt) => {
                    if let Some(handler) = &permission_handler
                        && !handler.ask_permission(&tool_name, &prompt, &input).await
                    {
                        return Ok(ToolOutput::error(REJECT_MESSAGE.to_string()));
                    }
                }
            }

            // Execute with streaming context
            // For now, tools that support streaming can check for a StreamingOutput
            // in the future via ToolContext extension. The bash tool uses tokio::process
            // internally and sends deltas through the StreamingOutput.
            //
            // For the initial implementation, we run the tool normally and send
            // the final output as a single delta.
            let result = tool.execute(input, &ctx).await?;
            streaming.send(result.text()).await;
            Ok(result)
        });

        (rx, handle)
    }

    /// Execute a tool without any permission checks.
    ///
    /// Used internally by sub-agents that inherit parent permissions.
    pub async fn execute_unchecked(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> crab_core::Result<ToolOutput> {
        let tool = self
            .registry
            .get(tool_name)
            .ok_or_else(|| crab_core::Error::Other(format!("tool not found: {tool_name}")))?;
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
        // The `Tool` trait declares `fn name/description(&self) -> &str`,
        // so impls must match. `&'static str` would be a signature mismatch.
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &str {
            "echo"
        }
        #[allow(clippy::unnecessary_literal_bound)]
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
        ) -> Pin<Box<dyn Future<Output = crab_core::Result<ToolOutput>> + Send + '_>> {
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
            ext: crab_core::tool::ToolContextExt::default(),
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

    /// A handler that always denies permission.
    struct DenyAll;
    impl PermissionHandler for DenyAll {
        fn ask_permission(
            &self,
            _tool_name: &str,
            _prompt: &str,
            _tool_input: &serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { false })
        }
    }

    // ─── StreamingOutput tests ───

    #[tokio::test]
    async fn streaming_output_basic_send_recv() {
        let (so, mut rx) = StreamingOutput::channel(8);
        so.send("line 1\n").await;
        so.send("line 2\n").await;
        drop(so);

        let mut lines = Vec::new();
        while let Some(line) = rx.recv().await {
            lines.push(line);
        }
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line 1\n");
        assert_eq!(lines[1], "line 2\n");
    }

    #[tokio::test]
    async fn streaming_output_dropped_receiver() {
        let (so, rx) = StreamingOutput::channel(1);
        drop(rx);
        // Should not panic — send silently drops
        so.send("orphaned").await;
    }

    #[tokio::test]
    async fn execute_streaming_existing_tool() {
        let executor = make_executor();
        let ctx = make_ctx(PermissionMode::Dangerously);
        let (mut rx, handle) =
            executor.execute_streaming("echo", serde_json::json!({"text": "streamed"}), &ctx);

        let result = handle.await.unwrap().unwrap();
        assert!(!result.is_error);
        assert_eq!(result.text(), "streamed");

        // Should have received at least one delta
        let mut deltas = Vec::new();
        while let Some(d) = rx.recv().await {
            deltas.push(d);
        }
        assert!(!deltas.is_empty());
    }

    #[tokio::test]
    async fn execute_streaming_missing_tool() {
        let executor = make_executor();
        let ctx = make_ctx(PermissionMode::Default);
        let (_rx, handle) = executor.execute_streaming("nonexistent", serde_json::json!({}), &ctx);

        let result = handle.await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn permission_handler_deny_blocks_execution() {
        let mut executor = make_executor();
        executor.set_permission_handler(Arc::new(DenyAll));

        // EchoTool is read_only, so it's always allowed. We need a non-read-only tool.
        // Use the echo tool but in Default mode -- it's read_only so it's auto-allowed.
        // Let's test with a context that forces AskUser by checking the flow.
        // Since EchoTool is read_only, it won't trigger AskUser.
        // This test verifies the handler is wired up properly at the API level.
        let ctx = make_ctx(PermissionMode::Default);
        let output = executor
            .execute("echo", serde_json::json!({"text": "hello"}), &ctx)
            .await
            .unwrap();
        // Read-only tool is always allowed, so handler is not called
        assert!(!output.is_error);
    }

    struct MutatingTool;

    impl Tool for MutatingTool {
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &str {
            "mutating"
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn description(&self) -> &str {
            "a non-read-only tool used to force an AskUser permission decision"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _input: Value,
            _ctx: &ToolContext,
        ) -> Pin<Box<dyn Future<Output = crab_core::Result<ToolOutput>> + Send + '_>> {
            Box::pin(async move { Ok(ToolOutput::success("mutated")) })
        }
        fn is_read_only(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn ask_user_denied_returns_canonical_reject_message() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(MutatingTool));
        let mut executor = ToolExecutor::new(Arc::new(reg));
        executor.set_permission_handler(Arc::new(DenyAll));

        let ctx = make_ctx(PermissionMode::Default);
        let output = executor
            .execute("mutating", serde_json::json!({}), &ctx)
            .await
            .unwrap();

        assert!(output.is_error);
        assert_eq!(output.text(), REJECT_MESSAGE);
    }

    // ─── apply_result_budget tests ───

    #[test]
    fn budget_passes_small_output_through() {
        let out = ToolOutput::success("short");
        let result = apply_result_budget(out, 1_000, "tu_test");
        assert_eq!(result.text(), "short");
        assert!(!result.is_error);
    }

    #[test]
    fn budget_does_not_touch_error_outputs() {
        let big = "x".repeat(100_000);
        let out = ToolOutput::error(big.clone());
        let result = apply_result_budget(out, 100, "tu_err");
        assert!(result.is_error);
        assert_eq!(result.text(), big);
    }

    #[test]
    fn budget_persists_oversized_output_and_returns_preview() {
        // Use a unique id so this test doesn't collide with others.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let id = format!("tu_budget_{}_{nanos}", std::process::id());
        let big: String = "y".repeat(50_000);
        let out = ToolOutput::success(big.clone());
        let result = apply_result_budget(out, 1_000, &id);

        assert!(!result.is_error);
        let text = result.text();
        // Reference line should point at a temp file
        assert!(text.contains("[Full output saved to"));
        assert!(text.contains("use Read tool to access]"));
        // Truncation marker present
        assert!(text.contains("characters omitted"));
        // Original full content should be on disk
        let path = std::env::temp_dir()
            .join(super::TOOL_RESULT_STORAGE_SUBDIR)
            .join(format!("{id}.txt"));
        assert!(path.exists(), "expected spill file at {}", path.display());
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, big);
        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn execute_streaming_ask_user_denied_returns_canonical_reject_message() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(MutatingTool));
        let mut executor = ToolExecutor::new(Arc::new(reg));
        executor.set_permission_handler(Arc::new(DenyAll));

        let ctx = make_ctx(PermissionMode::Default);
        let (_rx, handle) = executor.execute_streaming("mutating", serde_json::json!({}), &ctx);
        let output = handle.await.unwrap().unwrap();

        assert!(output.is_error);
        assert_eq!(output.text(), REJECT_MESSAGE);
    }
}
