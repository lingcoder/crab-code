use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use crab_process::spawn::{SpawnOptions, run};
use serde_json::Value;

/// `PowerShell` command execution tool (Windows only).
///
/// Prefers `pwsh` (`PowerShell` 7+) when available, falls back to
/// `powershell.exe` (Windows `PowerShell` 5.1).
pub struct PowerShellTool;

impl Tool for PowerShellTool {
    fn name(&self) -> &'static str {
        "powershell"
    }

    fn description(&self) -> &'static str {
        "Execute a PowerShell command. Uses pwsh (PowerShell 7+) when available, \
         otherwise falls back to powershell.exe (5.1). Returns stdout and stderr combined. \
         On non-zero exit the output is marked as an error."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The PowerShell command to execute"
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

            let (prog, args) = resolve_powershell(&command);

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
                    vec![ToolOutputContent::Text {
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

/// Resolve the `PowerShell` executable — prefer `pwsh` (PS 7+), fall back to
/// `powershell.exe` (Windows 5.1).
fn resolve_powershell(command: &str) -> (String, Vec<String>) {
    let args = vec![
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-Command".to_owned(),
        command.to_owned(),
    ];

    // Check if pwsh is available (PowerShell 7+)
    if is_pwsh_available() {
        ("pwsh".to_owned(), args)
    } else {
        ("powershell".to_owned(), args)
    }
}

/// Check if `pwsh` (`PowerShell` 7+) is on the PATH.
/// The result is cached after the first call via `OnceLock`.
fn is_pwsh_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("pwsh")
            .arg("-Version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use crab_core::tool::Tool;
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

    #[test]
    fn tool_name_is_powershell() {
        assert_eq!(PowerShellTool.name(), "powershell");
    }

    #[test]
    fn tool_description_not_empty() {
        assert!(!PowerShellTool.description().is_empty());
    }

    #[test]
    fn tool_requires_confirmation() {
        assert!(PowerShellTool.requires_confirmation());
    }

    #[test]
    fn tool_is_not_read_only() {
        assert!(!PowerShellTool.is_read_only());
    }

    #[test]
    fn input_schema_has_command_field() {
        let schema = PowerShellTool.input_schema();
        assert!(schema["properties"]["command"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("command")));
    }

    #[test]
    fn input_schema_has_timeout_field() {
        let schema = PowerShellTool.input_schema();
        assert!(schema["properties"]["timeout"].is_object());
    }

    #[test]
    fn resolve_powershell_uses_no_profile() {
        let (_prog, args) = resolve_powershell("Get-Process");
        assert!(args.contains(&"-NoProfile".to_owned()));
        assert!(args.contains(&"-NonInteractive".to_owned()));
        assert!(args.contains(&"-Command".to_owned()));
        assert!(args.contains(&"Get-Process".to_owned()));
    }

    #[tokio::test]
    async fn empty_command_returns_error() {
        let out = PowerShellTool
            .execute(serde_json::json!({"command": ""}), &make_ctx())
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn empty_input_returns_error() {
        let out = PowerShellTool
            .execute(serde_json::json!({}), &make_ctx())
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn execute_simple_powershell_command() {
        let out = PowerShellTool
            .execute(
                serde_json::json!({"command": "Write-Output 'hello from powershell'"}),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("hello from powershell"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn execute_powershell_nonzero_exit() {
        let out = PowerShellTool
            .execute(serde_json::json!({"command": "exit 42"}), &make_ctx())
            .await
            .unwrap();
        assert!(out.is_error);
    }
}
