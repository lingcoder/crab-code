//! Lifecycle hooks for pre/post tool execution.
//!
//! Hooks are shell commands configured in settings that run before or after
//! tool invocations. They receive context via environment variables and can
//! influence execution (e.g., a pre-tool hook returning non-zero blocks the tool).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// When a hook fires relative to tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookTrigger {
    /// Before a tool is executed. Non-zero exit blocks the tool.
    PreToolUse,
    /// After a tool completes.
    PostToolUse,
}

/// A single hook definition from settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    /// When this hook fires.
    pub trigger: HookTrigger,
    /// Shell command to execute.
    pub command: String,
    /// Optional timeout (defaults to 30s).
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Only run for these tool names (empty = all tools).
    #[serde(default)]
    pub tool_filter: Vec<String>,
}

fn default_timeout_secs() -> u64 {
    30
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
}

/// Result of executing a hook.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Whether the hook allowed the operation to proceed.
    pub allowed: bool,
    /// Hook stdout output.
    pub stdout: String,
    /// Hook stderr output.
    pub stderr: String,
    /// Hook process exit code.
    pub exit_code: i32,
    /// Whether the hook timed out.
    pub timed_out: bool,
}

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
    ///     "tool_filter": ["bash"]
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
                h.trigger == trigger
                    && (h.tool_filter.is_empty() || h.tool_filter.iter().any(|f| f == tool_name))
            })
            .collect()
    }

    /// Run all hooks for a given trigger point.
    ///
    /// For `PreToolUse`, if any hook returns non-zero, the result's `allowed`
    /// field is `false` (the tool should be blocked).
    ///
    /// For `PostToolUse`, hooks are informational — `allowed` is always `true`.
    #[allow(clippy::too_many_lines)]
    pub async fn run(
        &self,
        trigger: HookTrigger,
        ctx: &HookContext,
    ) -> crab_common::Result<HookResult> {
        let hooks = self.matching_hooks(trigger, &ctx.tool_name);

        if hooks.is_empty() {
            return Ok(HookResult {
                allowed: true,
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
        let mut all_allowed = true;

        for hook in hooks {
            let mut env = vec![
                ("CRAB_TOOL_NAME".to_string(), ctx.tool_name.clone()),
                ("CRAB_TOOL_INPUT".to_string(), ctx.tool_input.clone()),
                (
                    "CRAB_HOOK_TRIGGER".to_string(),
                    match trigger {
                        HookTrigger::PreToolUse => "pre_tool_use".to_string(),
                        HookTrigger::PostToolUse => "post_tool_use".to_string(),
                    },
                ),
            ];
            if let Some(ref output) = ctx.tool_output {
                env.push(("CRAB_TOOL_OUTPUT".to_string(), output.clone()));
            }
            if let Some(code) = ctx.tool_exit_code {
                env.push(("CRAB_TOOL_EXIT_CODE".to_string(), code.to_string()));
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

                    if output.exit_code != 0 {
                        final_exit_code = output.exit_code;
                        if trigger == HookTrigger::PreToolUse {
                            all_allowed = false;
                        }
                    }
                    if output.timed_out {
                        any_timed_out = true;
                        if trigger == HookTrigger::PreToolUse {
                            all_allowed = false;
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
                        all_allowed = false;
                    }
                }
            }
        }

        Ok(HookResult {
            allowed: all_allowed,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_trigger_serde_roundtrip() {
        let pre = serde_json::to_string(&HookTrigger::PreToolUse).unwrap();
        assert_eq!(pre, "\"pre_tool_use\"");
        let post = serde_json::to_string(&HookTrigger::PostToolUse).unwrap();
        assert_eq!(post, "\"post_tool_use\"");

        let parsed: HookTrigger = serde_json::from_str(&pre).unwrap();
        assert_eq!(parsed, HookTrigger::PreToolUse);
    }

    #[test]
    fn hook_def_deserialize() {
        let json = r#"{
            "trigger": "pre_tool_use",
            "command": "echo check",
            "timeout_secs": 10,
            "tool_filter": ["bash"]
        }"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.trigger, HookTrigger::PreToolUse);
        assert_eq!(hook.command, "echo check");
        assert_eq!(hook.timeout_secs, 10);
        assert_eq!(hook.tool_filter, vec!["bash"]);
    }

    #[test]
    fn hook_def_default_timeout() {
        let json = r#"{"trigger": "post_tool_use", "command": "echo done"}"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.timeout_secs, 30);
        assert!(hook.tool_filter.is_empty());
    }

    #[test]
    fn from_settings_value_parses_array() {
        let val = serde_json::json!([
            {"trigger": "pre_tool_use", "command": "echo pre"},
            {"trigger": "post_tool_use", "command": "echo post"}
        ]);
        let executor = HookExecutor::from_settings_value(&val).unwrap();
        assert_eq!(executor.len(), 2);
    }

    #[test]
    fn from_settings_value_invalid() {
        let val = serde_json::json!("not an array");
        assert!(HookExecutor::from_settings_value(&val).is_err());
    }

    #[test]
    fn matching_hooks_filters_correctly() {
        let executor = HookExecutor::with_hooks(vec![
            HookDef {
                trigger: HookTrigger::PreToolUse,
                command: "echo all".into(),
                timeout_secs: 30,
                tool_filter: vec![],
            },
            HookDef {
                trigger: HookTrigger::PreToolUse,
                command: "echo bash-only".into(),
                timeout_secs: 30,
                tool_filter: vec!["bash".into()],
            },
            HookDef {
                trigger: HookTrigger::PostToolUse,
                command: "echo post".into(),
                timeout_secs: 30,
                tool_filter: vec![],
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
    fn default_executor_is_empty() {
        let exec = HookExecutor::default();
        assert!(exec.is_empty());
    }

    #[tokio::test]
    async fn run_no_hooks_returns_allowed() {
        let exec = HookExecutor::new();
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(result.allowed);
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn run_pre_hook_success() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: if cfg!(windows) {
                "echo ok".into()
            } else {
                "echo ok".into()
            },
            timeout_secs: 10,
            tool_filter: vec![],
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(result.allowed);
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("ok"));
    }

    #[tokio::test]
    async fn run_pre_hook_blocks_on_failure() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: if cfg!(windows) {
                "exit 1".into()
            } else {
                "exit 1".into()
            },
            timeout_secs: 10,
            tool_filter: vec![],
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(!result.allowed);
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn run_post_hook_always_allowed() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PostToolUse,
            command: if cfg!(windows) {
                "exit 1".into()
            } else {
                "exit 1".into()
            },
            timeout_secs: 10,
            tool_filter: vec![],
        }]);
        let ctx = HookContext {
            tool_name: "bash".into(),
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: Some("result".into()),
            tool_exit_code: Some(0),
        };
        let result = exec.run(HookTrigger::PostToolUse, &ctx).await.unwrap();
        assert!(result.allowed); // post hooks are informational
    }

    #[tokio::test]
    async fn run_hook_with_tool_filter_skip() {
        let exec = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "exit 1".into(),
            timeout_secs: 10,
            tool_filter: vec!["bash".into()],
        }]);
        let ctx = HookContext {
            tool_name: "read".into(), // not "bash"
            tool_input: "{}".into(),
            working_dir: None,
            tool_output: None,
            tool_exit_code: None,
        };
        let result = exec.run(HookTrigger::PreToolUse, &ctx).await.unwrap();
        assert!(result.allowed); // no matching hooks
    }
}
