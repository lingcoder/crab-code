//! Lifecycle hooks for pre/post tool execution and user prompt submission.
//!
//! Hooks are shell commands configured in settings that run before or after
//! tool invocations (or when the user submits a prompt). They receive context
//! via environment variables and can influence execution — e.g. a pre-tool hook
//! can deny or modify tool input.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ─── Types ──────────────────────────────────────────────────────────────

/// When a hook fires relative to tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookTrigger {
    /// Before a tool is executed.
    PreToolUse,
    /// After a tool completes.
    PostToolUse,
    /// When the user submits a prompt (before it reaches the LLM).
    UserPromptSubmit,
}

/// A single hook definition from settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    /// When this hook fires.
    #[serde(alias = "event")]
    pub trigger: HookTrigger,
    /// Shell command to execute.
    pub command: String,
    /// Optional timeout in seconds (defaults to 10s).
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Only run for these tool names (empty = all tools). Exact match.
    #[serde(default)]
    pub tool_filter: Vec<String>,
    /// Glob pattern to match tool names (e.g. "bash", "mcp__*", "*").
    /// When set, takes precedence over `tool_filter`.
    #[serde(default, alias = "match")]
    pub match_pattern: Option<String>,
}

fn default_timeout_secs() -> u64 {
    10
}

/// Context passed to a hook via environment variables.
#[derive(Debug, Clone)]
pub struct HookContext {
    /// Name of the tool being invoked.
    pub tool_name: String,
    /// JSON-serialized tool input.
    pub tool_input: String,
    /// Working directory for the hook process.
    pub working_dir: Option<PathBuf>,
    /// JSON-serialized tool output (only for post-tool-use).
    pub tool_output: Option<String>,
    /// Tool exit code (only for post-tool-use).
    pub tool_exit_code: Option<i32>,
    /// Current session ID.
    pub session_id: Option<String>,
}

/// Action a hook commands the system to take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookAction {
    /// Allow the operation to proceed as-is.
    #[default]
    Allow,
    /// Block the operation.
    Deny,
    /// Allow but with modified input.
    Modify,
}


/// Structured result parsed from a hook's JSON stdout.
///
/// If a hook prints valid JSON to stdout matching this shape, it is used.
/// Otherwise the system falls back to exit-code semantics (0 = Allow, non-0 = Deny).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuredHookResult {
    /// What the hook wants the system to do.
    #[serde(default)]
    pub action: HookAction,
    /// Optional human-readable message (shown to user or fed back to LLM).
    #[serde(default)]
    pub message: Option<String>,
    /// Modified tool input (only meaningful when `action == Modify`).
    #[serde(default, alias = "modifiedInput")]
    pub modified_input: Option<serde_json::Value>,
}

/// Result of executing one or more hooks for a trigger point.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Structured action resolved from hook output.
    pub action: HookAction,
    /// Optional message from the hook.
    pub message: Option<String>,
    /// Modified tool input (if action is Modify).
    pub modified_input: Option<serde_json::Value>,
    /// Combined hook stdout output.
    pub stdout: String,
    /// Combined hook stderr output.
    pub stderr: String,
    /// Last non-zero hook exit code (or 0).
    pub exit_code: i32,
    /// Whether any hook timed out.
    pub timed_out: bool,
}

impl HookResult {
    /// Whether the hook(s) allow the operation to proceed.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.action != HookAction::Deny
    }
}

// ─── Executor ───────────────────────────────────────────────────────────

/// Executes lifecycle hooks around tool invocations.
pub struct HookExecutor {
    hooks: Vec<HookDef>,
}

impl HookExecutor {
    /// Create executor with no hooks.
    #[must_use]
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Create executor from parsed hook definitions.
    #[must_use]
    pub fn with_hooks(hooks: Vec<HookDef>) -> Self {
        Self { hooks }
    }

    /// Parse hook definitions from the settings `hooks` JSON value.
    ///
    /// Expected format:
    /// ```json
    /// [
    ///   {
    ///     "trigger": "pre_tool_use",
    ///     "command": "echo pre",
    ///     "timeout_secs": 10,
    ///     "match_pattern": "bash"
    ///   }
    /// ]
    /// ```
    pub fn from_settings_value(value: &serde_json::Value) -> crab_common::Result<Self> {
        let hooks: Vec<HookDef> = serde_json::from_value(value.clone())
            .map_err(|e| crab_common::Error::Other(format!("invalid hooks config: {e}")))?;
        Ok(Self::with_hooks(hooks))
    }

    /// Get hooks matching a trigger point and tool name.
    fn matching_hooks(&self, trigger: HookTrigger, tool_name: &str) -> Vec<&HookDef> {
        self.hooks
            .iter()
            .filter(|h| {
                if h.trigger != trigger {
                    return false;
                }
                // UserPromptSubmit hooks don't filter by tool name
                if trigger == HookTrigger::UserPromptSubmit {
                    return true;
                }
                // match_pattern takes precedence over tool_filter
                if let Some(ref pattern) = h.match_pattern {
                    return glob_match_tool(pattern, tool_name);
                }
                h.tool_filter.is_empty() || h.tool_filter.iter().any(|f| f == tool_name)
            })
            .collect()
    }

    /// Run all hooks for a given trigger point.
    ///
    /// For `PreToolUse`, if any hook returns Deny (via structured JSON or non-zero
    /// exit), the result action is `Deny`.
    ///
    /// For `PostToolUse` and `UserPromptSubmit`, hooks are informational.
    ///
    /// Hook stdout is parsed as JSON (`StructuredHookResult`). If parsing fails,
    /// exit code semantics apply: 0 = Allow, non-0 = Deny.
    ///
    /// Timeout (default 10s) is treated as Allow (the operation proceeds).
    #[allow(clippy::too_many_lines)]
    pub async fn run(
        &self,
        trigger: HookTrigger,
        ctx: &HookContext,
    ) -> crab_common::Result<HookResult> {
        let hooks = self.matching_hooks(trigger, &ctx.tool_name);

        if hooks.is_empty() {
            return Ok(HookResult {
                action: HookAction::Allow,
                message: None,
                modified_input: None,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
            });
        }

        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut final_exit_code = 0;
        let mut any_timed_out = false;
        let mut resolved_action = HookAction::Allow;
        let mut resolved_message: Option<String> = None;
        let mut resolved_modified_input: Option<serde_json::Value> = None;

        for hook in hooks {
            let mut env = vec![
                ("CRAB_TOOL_NAME".to_string(), ctx.tool_name.clone()),
                ("CRAB_TOOL_INPUT".to_string(), ctx.tool_input.clone()),
                (
                    "CRAB_HOOK_TRIGGER".to_string(),
                    match trigger {
                        HookTrigger::PreToolUse => "pre_tool_use".to_string(),
                        HookTrigger::PostToolUse => "post_tool_use".to_string(),
                        HookTrigger::UserPromptSubmit => "user_prompt_submit".to_string(),
                    },
                ),
            ];
            if let Some(ref output) = ctx.tool_output {
                env.push(("CRAB_TOOL_OUTPUT".to_string(), output.clone()));
            }
            if let Some(code) = ctx.tool_exit_code {
                env.push(("CRAB_TOOL_EXIT_CODE".to_string(), code.to_string()));
            }
            if let Some(ref sid) = ctx.session_id {
                env.push(("CRAB_SESSION_ID".to_string(), sid.clone()));
            }

            let (shell, shell_flag) = if cfg!(windows) {
                ("cmd".to_string(), "/C".to_string())
            } else {
                ("sh".to_string(), "-c".to_string())
            };

            let opts = crab_process::spawn::SpawnOptions {
                command: shell,
                args: vec![shell_flag, hook.command.clone()],
                working_dir: ctx.working_dir.clone(),
                env,
                timeout: Some(Duration::from_secs(hook.timeout_secs)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            };

            match crab_process::spawn::run(opts).await {
                Ok(output) => {
                    tracing::debug!(
                        hook_command = hook.command.as_str(),
                        exit_code = output.exit_code,
                        timed_out = output.timed_out,
                        "hook completed"
                    );

                    if !combined_stdout.is_empty() && !output.stdout.is_empty() {
                        combined_stdout.push('\n');
                    }
                    combined_stdout.push_str(&output.stdout);

                    if !combined_stderr.is_empty() && !output.stderr.is_empty() {
                        combined_stderr.push('\n');
                    }
                    combined_stderr.push_str(&output.stderr);

                    if output.timed_out {
                        any_timed_out = true;
                        // Timeout is treated as Allow — the operation proceeds
                        continue;
                    }

                    // Try to parse stdout as structured JSON result
                    let structured = parse_hook_stdout(&output.stdout);

                    match structured {
                        Some(sr) => {
                            // Structured result takes priority
                            if sr.action == HookAction::Deny {
                                resolved_action = HookAction::Deny;
                                resolved_message = sr.message.or(resolved_message);
                            } else if sr.action == HookAction::Modify
                                && resolved_action != HookAction::Deny
                            {
                                resolved_action = HookAction::Modify;
                                resolved_modified_input = sr.modified_input;
                                resolved_message = sr.message.or(resolved_message);
                            } else if let Some(msg) = sr.message {
                                resolved_message = Some(msg);
                            }
                        }
                        None => {
                            // Fallback to exit code semantics
                            if output.exit_code != 0 {
                                final_exit_code = output.exit_code;
                                if trigger == HookTrigger::PreToolUse {
                                    resolved_action = HookAction::Deny;
                                    // Use stderr as message if available
                                    if !output.stderr.is_empty() {
                                        resolved_message = Some(output.stderr.trim().to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        hook_command = hook.command.as_str(),
                        error = %e,
                        "hook execution failed"
                    );
                    let _ = std::fmt::Write::write_fmt(
                        &mut combined_stderr,
                        format_args!("hook error: {e}"),
                    );
                    final_exit_code = -1;
                    if trigger == HookTrigger::PreToolUse {
                        resolved_action = HookAction::Deny;
                    }
                }
            }
        }

        Ok(HookResult {
            action: resolved_action,
            message: resolved_message,
            modified_input: resolved_modified_input,
            stdout: combined_stdout,
            stderr: combined_stderr,
            exit_code: final_exit_code,
            timed_out: any_timed_out,
        })
    }

    /// Number of registered hooks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Whether there are no hooks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

impl Default for HookExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────

/// Try to parse hook stdout as a `StructuredHookResult`.
/// Returns `None` if stdout is empty or not valid JSON matching the schema.
fn parse_hook_stdout(stdout: &str) -> Option<StructuredHookResult> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// Simple glob match for tool name patterns.
/// Supports `*` (match any) and exact match. The `*` wildcard matches all tools.
fn glob_match_tool(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return tool_name.ends_with(suffix);
    }
    pattern == tool_name
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Serde / config tests ────────────────────────────────────────

    #[test]
    fn hook_trigger_serde_roundtrip() {
        let pre = serde_json::to_string(&HookTrigger::PreToolUse).unwrap();
        assert_eq!(pre, "\"pre_tool_use\"");
        let post = serde_json::to_string(&HookTrigger::PostToolUse).unwrap();
        assert_eq!(post, "\"post_tool_use\"");
        let submit = serde_json::to_string(&HookTrigger::UserPromptSubmit).unwrap();
        assert_eq!(submit, "\"user_prompt_submit\"");

        let parsed: HookTrigger = serde_json::from_str(&pre).unwrap();
        assert_eq!(parsed, HookTrigger::PreToolUse);
        let parsed: HookTrigger = serde_json::from_str(&submit).unwrap();
        assert_eq!(parsed, HookTrigger::UserPromptSubmit);
    }

    #[test]
    fn hook_def_deserialize() {
        let json = r#"{
            "trigger": "pre_tool_use",
            "command": "echo check",
            "timeout_secs": 5,
            "tool_filter": ["bash"]
        }"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.trigger, HookTrigger::PreToolUse);
        assert_eq!(hook.command, "echo check");
        assert_eq!(hook.timeout_secs, 5);
        assert_eq!(hook.tool_filter, vec!["bash"]);
    }

    #[test]
    fn hook_def_default_timeout() {
        let json = r#"{"trigger": "post_tool_use", "command": "echo done"}"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.timeout_secs, 10);
        assert!(hook.tool_filter.is_empty());
    }

    #[test]
    fn hook_def_with_match_pattern() {
        let json = r#"{"trigger": "pre_tool_use", "command": "validate", "match_pattern": "mcp__*"}"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.match_pattern.as_deref(), Some("mcp__*"));
    }

    #[test]
    fn hook_def_event_alias() {
        // "event" is an alias for "trigger" in the JSON
        let json = r#"{"event": "user_prompt_submit", "command": "echo hi"}"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.trigger, HookTrigger::UserPromptSubmit);
    }

    #[test]
    fn hook_def_match_alias() {
        // "match" is an alias for "match_pattern"
        let json = r#"{"trigger": "pre_tool_use", "command": "check", "match": "bash"}"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.match_pattern.as_deref(), Some("bash"));
    }

    #[test]
    fn from_settings_value_parses_array() {
        let val = serde_json::json!([
            {"trigger": "pre_tool_use", "command": "echo pre"},
            {"trigger": "post_tool_use", "command": "echo post"},
            {"trigger": "user_prompt_submit", "command": "echo submit"}
        ]);
        let executor = HookExecutor::from_settings_value(&val).unwrap();
        assert_eq!(executor.len(), 3);
    }

    #[test]
    fn from_settings_value_invalid() {
        let val = serde_json::json!("not an array");
        assert!(HookExecutor::from_settings_value(&val).is_err());
    }

    // ── Matching tests ──────────────────────────────────────────────

    #[test]
    fn matching_hooks_filters_correctly() {
        let executor = HookExecutor::with_hooks(vec![
            HookDef {
                trigger: HookTrigger::PreToolUse,
                command: "echo all".into(),
                timeout_secs: 10,
                tool_filter: vec![],
                match_pattern: None,
            },
            HookDef {
                trigger: HookTrigger::PreToolUse,
                command: "echo bash-only".into(),
                timeout_secs: 10,
                tool_filter: vec!["bash".into()],
                match_pattern: None,
            },
            HookDef {
                trigger: HookTrigger::PostToolUse,
                command: "echo post".into(),
                timeout_secs: 10,
                tool_filter: vec![],
                match_pattern: None,
            },
        ]);

        let pre_bash = executor.matching_hooks(HookTrigger::PreToolUse, "bash");
        assert_eq!(pre_bash.len(), 2);

        let pre_read = executor.matching_hooks(HookTrigger::PreToolUse, "read");
        assert_eq!(pre_read.len(), 1);
        assert_eq!(pre_read[0].command, "echo all");

        let post = executor.matching_hooks(HookTrigger::PostToolUse, "bash");
        assert_eq!(post.len(), 1);
    }

    #[test]
    fn matching_hooks_match_pattern_glob() {
        let executor = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "validate".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: Some("mcp__*".into()),
        }]);

        let mcp = executor.matching_hooks(HookTrigger::PreToolUse, "mcp__server__tool");
        assert_eq!(mcp.len(), 1);

        let bash = executor.matching_hooks(HookTrigger::PreToolUse, "bash");
        assert!(bash.is_empty());
    }

    #[test]
    fn matching_hooks_match_pattern_star_matches_all() {
        let executor = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "log".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: Some("*".into()),
        }]);

        assert_eq!(
            executor
                .matching_hooks(HookTrigger::PreToolUse, "anything")
                .len(),
            1
        );
    }

    #[test]
    fn matching_hooks_user_prompt_submit_ignores_tool_name() {
        let executor = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::UserPromptSubmit,
            command: "echo submit".into(),
            timeout_secs: 10,
            tool_filter: vec!["bash".into()], // should be ignored
            match_pattern: None,
        }]);

        let hooks = executor.matching_hooks(HookTrigger::UserPromptSubmit, "anything");
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn default_executor_is_empty() {
        let exec = HookExecutor::default();
        assert!(exec.is_empty());
    }

    // ── Glob match tests ────────────────────────────────────────────

    #[test]
    fn glob_match_tool_exact() {
        assert!(glob_match_tool("bash", "bash"));
        assert!(!glob_match_tool("bash", "read"));
    }

    #[test]
    fn glob_match_tool_star_all() {
        assert!(glob_match_tool("*", "anything"));
        assert!(glob_match_tool("*", ""));
    }

    #[test]
    fn glob_match_tool_prefix_star() {
        assert!(glob_match_tool("mcp__*", "mcp__server"));
        assert!(glob_match_tool("mcp__*", "mcp__"));
        assert!(!glob_match_tool("mcp__*", "bash"));
    }

    #[test]
    fn glob_match_tool_suffix_star() {
        assert!(glob_match_tool("*_tool", "my_tool"));
        assert!(!glob_match_tool("*_tool", "my_thing"));
    }

    // ── Structured result parsing tests ─────────────────────────────

    #[test]
    fn parse_hook_stdout_empty() {
        assert!(parse_hook_stdout("").is_none());
        assert!(parse_hook_stdout("  \n").is_none());
    }

    #[test]
    fn parse_hook_stdout_non_json() {
        assert!(parse_hook_stdout("ok").is_none());
        assert!(parse_hook_stdout("some random text").is_none());
    }

    #[test]
    fn parse_hook_stdout_allow() {
        let json = r#"{"action": "allow"}"#;
        let sr = parse_hook_stdout(json).unwrap();
        assert_eq!(sr.action, HookAction::Allow);
        assert!(sr.message.is_none());
    }

    #[test]
    fn parse_hook_stdout_deny_with_message() {
        let json = r#"{"action": "deny", "message": "blocked by policy"}"#;
        let sr = parse_hook_stdout(json).unwrap();
        assert_eq!(sr.action, HookAction::Deny);
        assert_eq!(sr.message.as_deref(), Some("blocked by policy"));
    }

    #[test]
    fn parse_hook_stdout_modify_with_input() {
        let json = r#"{"action": "modify", "modified_input": {"command": "safe_cmd"}}"#;
        let sr = parse_hook_stdout(json).unwrap();
        assert_eq!(sr.action, HookAction::Modify);
        assert_eq!(
            sr.modified_input.unwrap()["command"],
            serde_json::json!("safe_cmd")
        );
    }

    #[test]
    fn parse_hook_stdout_camel_case_alias() {
        // modifiedInput alias
        let json = r#"{"action": "modify", "modifiedInput": {"key": "val"}}"#;
        let sr = parse_hook_stdout(json).unwrap();
        assert_eq!(sr.action, HookAction::Modify);
        assert!(sr.modified_input.is_some());
    }

    #[test]
    fn hook_action_default_is_allow() {
        assert_eq!(HookAction::default(), HookAction::Allow);
    }

    #[test]
    fn hook_result_is_allowed() {
        let allowed = HookResult {
            action: HookAction::Allow,
            message: None,
            modified_input: None,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        assert!(allowed.is_allowed());

        let denied = HookResult {
            action: HookAction::Deny,
            message: Some("no".into()),
            modified_input: None,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            timed_out: false,
        };
        assert!(!denied.is_allowed());

        let modified = HookResult {
            action: HookAction::Modify,
            message: None,
            modified_input: Some(serde_json::json!({})),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        assert!(modified.is_allowed());
    }

    // ── Async execution tests ───────────────────────────────────────

    #[tokio::test]
    async fn run_no_hooks_returns_allowed() {
        let exec = HookExecutor::new();
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
            session_id: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(result.is_allowed());
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn run_pre_hook_success() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "echo ok".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: None,
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
            session_id: Some("test-session".into()),
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(result.is_allowed());
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("ok"));
    }

    #[tokio::test]
    async fn run_pre_hook_blocks_on_failure() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "exit 1".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: None,
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
            session_id: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(!result.is_allowed());
        assert_eq!(result.action, HookAction::Deny);
    }

    #[tokio::test]
    async fn run_post_hook_always_allowed() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PostToolUse,
            command: "exit 1".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: None,
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: Some("result".into()),
            tool_exit_code: Some(0),
            session_id: None,
        };
        let result = exec.run(HookTrigger::PostToolUse, &ctx).await.unwrap();
        // Post hooks with non-zero exit don't deny — they're informational
        // (exit code semantics only apply to PreToolUse)
        assert_eq!(result.action, HookAction::Allow);
    }

    #[tokio::test]
    async fn run_hook_with_tool_filter_skip() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "exit 1".into(),
            timeout_secs: 10,
            tool_filter: vec!["bash".into()],
            match_pattern: None,
        }]);
        let ctx = HookContext {
            tool_name: "read".into(), // not "bash"
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
            session_id: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(result.is_allowed()); // no matching hooks
    }

    #[tokio::test]
    async fn run_pre_hook_structured_deny() {
        // Hook that outputs structured JSON deny — use a temp script to avoid
        // platform-specific echo quoting issues
        let dir = std::env::temp_dir().join("crab-hook-test-deny");
        let _ = std::fs::create_dir_all(&dir);
        let (_script_path, cmd) = if cfg!(windows) {
            let p = dir.join("deny.cmd");
            std::fs::write(&p, "@echo off\necho {\"action\": \"deny\", \"message\": \"blocked\"}").unwrap();
            (p.clone(), format!("cmd /C {}", p.display()))
        } else {
            let p = dir.join("deny.sh");
            std::fs::write(&p, "#!/bin/sh\necho '{\"action\": \"deny\", \"message\": \"blocked\"}'").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            (p.clone(), format!("sh {}", p.display()))
        };
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: cmd,
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: None,
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
            session_id: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert_eq!(result.action, HookAction::Deny);
        assert_eq!(result.message.as_deref(), Some("blocked"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn run_user_prompt_submit_hook() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::UserPromptSubmit,
            command: "echo ok".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: None,
        }]);
        let ctx = HookContext {
            tool_name: String::new(),
            tool_input: "user said hello".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
            session_id: Some("sess-123".into()),
        };
        let result = exec
            .run(HookTrigger::UserPromptSubmit, &ctx)
            .await
            .unwrap();
        assert!(result.is_allowed());
    }
}
