use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use crab_process::spawn::{SpawnOptions, run};
use serde_json::Value;

use crate::str_utils::truncate_chars;

/// `PowerShell` command execution tool (Windows + cross-platform via `pwsh`).
///
/// Prefers `pwsh` (`PowerShell` 7+) when available, falls back to
/// `powershell.exe` (Windows `PowerShell` 5.1).
pub const POWERSHELL_TOOL_NAME: &str = "PowerShell";

/// Which `PowerShell` edition is available on this host.
///
/// The two editions differ enough that a one-size description either lies
/// to the model on 5.1 (suggesting `&&`, ternary, null-coalescing — all
/// parser errors) or sells 7+ short. The tool description rendered to the
/// model is keyed on this enum so the syntax guidance matches reality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerShellEdition {
    /// `pwsh` / `pwsh.exe` — `PowerShell` 7+. Supports pipeline chain
    /// operators (`&&`, `||`), ternary, null-coalescing. UTF-8 default.
    Core,
    /// `powershell.exe` — Windows `PowerShell` 5.1. No `&&`/`||`,
    /// no ternary, UTF-16 LE default file encoding.
    Desktop,
    /// Neither `pwsh` nor `powershell` was found on PATH. The description
    /// falls back to conservative 5.1-safe guidance.
    Unknown,
}

pub struct PowerShellTool;

impl Tool for PowerShellTool {
    fn name(&self) -> &'static str {
        POWERSHELL_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        powershell_description()
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // Commands may contain multi-byte UTF-8 (paths, string literals);
        // truncate_chars counts codepoints to avoid panics on non-ASCII input.
        let cmd = input["command"].as_str()?;
        let display = truncate_chars(cmd, 160, "…");
        Some(format!("Run ({display})"))
    }
}

/// Detect which `PowerShell` edition is available on this host.
///
/// Cached after the first call via `OnceLock`. Probes `pwsh` first
/// (`PowerShell` 7+), then `powershell` (Windows `PowerShell` 5.1).
#[must_use]
pub fn detect_powershell_edition() -> PowerShellEdition {
    static EDITION: OnceLock<PowerShellEdition> = OnceLock::new();
    *EDITION.get_or_init(|| {
        if probe_binary("pwsh") {
            PowerShellEdition::Core
        } else if probe_binary("powershell") {
            PowerShellEdition::Desktop
        } else {
            PowerShellEdition::Unknown
        }
    })
}

/// Spawn `<bin> -Version` and return whether the process started successfully.
fn probe_binary(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("-Version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Edition-aware description rendered to the model. Cached after first
/// call so each tool-list build doesn't re-probe `PowerShell`.
fn powershell_description() -> &'static str {
    static DESC: OnceLock<String> = OnceLock::new();
    DESC.get_or_init(|| build_description(detect_powershell_edition()))
        .as_str()
}

fn build_description(edition: PowerShellEdition) -> String {
    let edition_section = match edition {
        PowerShellEdition::Core => {
            "PowerShell edition: PowerShell 7+ (pwsh)\n   - Pipeline chain operators `&&` and `||` ARE available and work like bash. Prefer `cmd1 && cmd2` over `cmd1; cmd2` when cmd2 should only run if cmd1 succeeds.\n   - Ternary (`$cond ? $a : $b`), null-coalescing (`??`), and null-conditional (`?.`) operators are available.\n   - Default file encoding is UTF-8 without BOM."
        }
        PowerShellEdition::Desktop => {
            "PowerShell edition: Windows PowerShell 5.1 (powershell.exe)\n   - Pipeline chain operators `&&` and `||` are NOT available — they cause a parser error. To run B only if A succeeds: `A; if ($?) { B }`. To chain unconditionally: `A; B`.\n   - Ternary (`?:`), null-coalescing (`??`), and null-conditional (`?.`) operators are NOT available. Use `if/else` and explicit `$null -eq` checks instead.\n   - Default file encoding is UTF-16 LE (with BOM). When writing files other tools will read, pass `-Encoding utf8` to `Out-File`/`Set-Content`."
        }
        PowerShellEdition::Unknown => {
            "PowerShell edition: unknown — assume Windows PowerShell 5.1 for compatibility\n   - Do NOT use `&&`, `||`, ternary `?:`, null-coalescing `??`, or null-conditional `?.`. These are PowerShell 7+ only and parser-error on 5.1.\n   - To chain commands conditionally: `A; if ($?) { B }`. Unconditionally: `A; B`."
        }
    };

    format!(
        "Execute a PowerShell command. Uses pwsh (PowerShell 7+) when available, otherwise falls back to powershell.exe (5.1). Returns stdout and stderr combined. On non-zero exit the output is marked as an error.\n\n{edition_section}"
    )
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

    match detect_powershell_edition() {
        PowerShellEdition::Core => ("pwsh".to_owned(), args),
        // On Desktop or Unknown we still try `powershell` — on non-Windows
        // hosts without either binary the spawn will fail at `run()`, which
        // surfaces a clear error instead of silently picking the wrong shell.
        PowerShellEdition::Desktop | PowerShellEdition::Unknown => ("powershell".to_owned(), args),
    }
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
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn tool_name_is_powershell() {
        assert_eq!(PowerShellTool.name(), "PowerShell");
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

    #[test]
    fn build_description_core_mentions_pwsh_features() {
        let desc = build_description(PowerShellEdition::Core);
        assert!(desc.contains("PowerShell 7+"));
        assert!(desc.contains("&&"));
        assert!(desc.contains("UTF-8"));
    }

    #[test]
    fn build_description_desktop_warns_against_pwsh7_syntax() {
        let desc = build_description(PowerShellEdition::Desktop);
        assert!(desc.contains("5.1"));
        assert!(desc.contains("NOT available"));
        assert!(desc.contains("UTF-16"));
    }

    #[test]
    fn build_description_unknown_uses_5_1_safe_defaults() {
        let desc = build_description(PowerShellEdition::Unknown);
        assert!(desc.contains("unknown"));
        assert!(desc.contains("5.1"));
        // Unknown defaults to the conservative path: warn the model away
        // from PS-7-only syntax.
        assert!(desc.contains("Do NOT use"));
    }

    #[test]
    fn detect_powershell_edition_is_stable() {
        // The result depends on the host but must be a valid enum value
        // and stable across calls (cache check).
        let first = detect_powershell_edition();
        let second = detect_powershell_edition();
        assert_eq!(first, second);
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
