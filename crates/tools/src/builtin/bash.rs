use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use crab_process::spawn::{SpawnOptions, run};
use serde_json::Value;

/// Shell command execution tool.
pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a bash command in the shell. Returns stdout and stderr combined. \
         On non-zero exit the output is marked as an error."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds (max 600000)"
                },
                "description": {
                    "type": "string",
                    "description": "Clear, concise description of what this command does"
                }
            },
            "required": ["command"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let command = input["command"].as_str().unwrap_or("").to_owned();
        let timeout_ms = input["timeout"].as_u64();
        let working_dir = ctx.working_dir.clone();

        Box::pin(async move {
            if command.is_empty() {
                return Ok(ToolOutput::error("command is required"));
            }

            let timeout = timeout_ms
                .map(Duration::from_millis)
                .or(Some(Duration::from_secs(120)));

            // On Windows run via cmd /C; elsewhere use sh -c
            let (prog, args) = if cfg!(windows) {
                ("cmd".to_owned(), vec!["/C".to_owned(), command])
            } else {
                ("sh".to_owned(), vec!["-c".to_owned(), command])
            };

            let opts = SpawnOptions {
                command: prog,
                args,
                working_dir: Some(working_dir),
                env: vec![],
                timeout,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            };

            let output = run(opts).await?;

            // Combine stdout and stderr
            let mut combined = String::new();
            if !output.stdout.is_empty() {
                combined.push_str(&output.stdout);
            }
            if !output.stderr.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&output.stderr);
            }

            if output.timed_out {
                return Ok(ToolOutput::error(format!("Command timed out\n{combined}")));
            }

            if output.exit_code != 0 {
                Ok(ToolOutput::with_content(
                    vec![crab_core::tool::ToolOutputContent::Text {
                        text: if combined.is_empty() {
                            format!("Exit code: {}", output.exit_code)
                        } else {
                            combined
                        },
                    }],
                    true,
                ))
            } else {
                Ok(ToolOutput::success(combined))
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
        }
    }

    #[tokio::test]
    async fn bash_echo() {
        let tool = BashTool;
        let input = serde_json::json!({ "command": "echo hello" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("hello"));
    }

    #[tokio::test]
    async fn bash_nonzero_exit_is_error() {
        let tool = BashTool;
        let cmd = if cfg!(windows) { "exit 1" } else { "exit 1" };
        let input = serde_json::json!({ "command": cmd });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn bash_empty_command_is_error() {
        let tool = BashTool;
        let input = serde_json::json!({ "command": "" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn bash_requires_confirmation() {
        assert!(BashTool.requires_confirmation());
    }

    #[test]
    fn bash_schema_has_required_command() {
        let schema = BashTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "command"));
    }
}
