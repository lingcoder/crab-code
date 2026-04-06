use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use crab_process::spawn::{SpawnOptions, run};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::executor::StreamingOutput;

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

impl BashTool {
    /// Execute with streaming output — sends each line of stdout/stderr through
    /// the `StreamingOutput` channel as it arrives.
    ///
    /// The final `ToolOutput` is consistent with non-streaming execution.
    pub async fn execute_streaming(
        &self,
        input: Value,
        ctx: &ToolContext,
        streaming: StreamingOutput,
    ) -> Result<ToolOutput> {
        let command = input["command"].as_str().unwrap_or("").to_owned();
        let timeout_ms = input["timeout"].as_u64();
        let working_dir = ctx.working_dir.clone();

        if command.is_empty() {
            return Ok(ToolOutput::error("command is required"));
        }

        let timeout = timeout_ms
            .map_or(Duration::from_secs(120), Duration::from_millis);

        let (prog, args) = if cfg!(windows) {
            ("cmd", vec!["/C".to_owned(), command])
        } else {
            ("sh", vec!["-c".to_owned(), command])
        };

        let mut child = tokio::process::Command::new(prog)
            .args(&args)
            .current_dir(&working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| crab_common::Error::Other(format!("failed to spawn: {e}")))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cancel = ctx.cancellation_token.clone();

        let mut combined = String::new();

        // Read stdout lines and stream them
        let streaming_clone = streaming.clone();
        let stdout_handle = tokio::spawn(async move {
            let mut lines = Vec::new();
            if let Some(out) = stdout {
                let reader = BufReader::new(out);
                let mut line_stream = reader.lines();
                while let Ok(Some(line)) = line_stream.next_line().await {
                    let delta = format!("{line}\n");
                    streaming_clone.send(&delta).await;
                    lines.push(line);
                }
            }
            lines
        });

        // Read stderr lines
        let stderr_handle = tokio::spawn(async move {
            let mut lines = Vec::new();
            if let Some(err) = stderr {
                let reader = BufReader::new(err);
                let mut line_stream = reader.lines();
                while let Ok(Some(line)) = line_stream.next_line().await {
                    lines.push(line);
                }
            }
            lines
        });

        // Wait for completion with timeout and cancellation support
        let result = tokio::select! {
            status = child.wait() => {
                let stdout_lines = stdout_handle.await.unwrap_or_default();
                let stderr_lines = stderr_handle.await.unwrap_or_default();

                for line in &stdout_lines {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(line);
                }
                for line in &stderr_lines {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(line);
                    // Also stream stderr lines
                    streaming.send(format!("{line}\n")).await;
                }

                match status {
                    Ok(s) if s.success() => Ok(ToolOutput::success(&combined)),
                    Ok(s) => {
                        let code = s.code().unwrap_or(-1);
                        if combined.is_empty() {
                            Ok(ToolOutput::error(format!("Exit code: {code}")))
                        } else {
                            Ok(ToolOutput::with_content(
                                vec![crab_core::tool::ToolOutputContent::Text { text: combined }],
                                true,
                            ))
                        }
                    }
                    Err(e) => Ok(ToolOutput::error(format!("process error: {e}"))),
                }
            }
            () = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                Ok(ToolOutput::error(format!("Command timed out\n{combined}")))
            }
            () = cancel.cancelled() => {
                let _ = child.kill().await;
                Ok(ToolOutput::error(format!("Command cancelled\n{combined}")))
            }
        };

        result
    }
}

// ─── PTY support (feature-gated) ───

#[cfg(feature = "pty")]
mod pty_support {
    use super::*;

    /// Configuration for PTY execution.
    pub struct PtyConfig {
        /// Whether to strip ANSI escape sequences from output.
        pub strip_ansi: bool,
        /// Timeout for the PTY session.
        pub timeout: Duration,
    }

    impl Default for PtyConfig {
        fn default() -> Self {
            Self {
                strip_ansi: true,
                timeout: Duration::from_secs(120),
            }
        }
    }

    impl BashTool {
        /// Execute a command in a pseudo-terminal.
        ///
        /// Uses `portable-pty` to create a PTY pair, run the command in it,
        /// and capture output. This is useful for commands that detect terminal
        /// presence (e.g. colored output, interactive prompts).
        pub async fn execute_with_pty(
            &self,
            command: &str,
            working_dir: &std::path::Path,
            config: PtyConfig,
        ) -> Result<ToolOutput> {
            if command.is_empty() {
                return Ok(ToolOutput::error("command is required"));
            }

            let command = command.to_owned();
            let working_dir = working_dir.to_path_buf();
            let timeout = config.timeout;
            let strip_ansi = config.strip_ansi;

            // PTY operations are blocking — run in a blocking thread
            let result = tokio::task::spawn_blocking(move || {
                run_in_pty(&command, &working_dir, timeout, strip_ansi)
            })
            .await
            .map_err(|e| {
                crab_common::Error::Other(format!("PTY task failed: {e}"))
            })??;

            Ok(result)
        }
    }

    fn run_in_pty(
        command: &str,
        working_dir: &std::path::Path,
        timeout: Duration,
        strip_ansi: bool,
    ) -> Result<ToolOutput> {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| crab_common::Error::Other(format!("failed to open PTY: {e}")))?;

        let (shell, flag) = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        let mut cmd = CommandBuilder::new(shell);
        cmd.arg(flag);
        cmd.arg(command);
        cmd.cwd(working_dir);

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| crab_common::Error::Other(format!("failed to spawn in PTY: {e}")))?;

        // Drop the slave — we read from master
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()
            .map_err(|e| crab_common::Error::Other(format!("failed to clone PTY reader: {e}")))?;

        // Read output with timeout
        let mut output = Vec::new();
        let start = std::time::Instant::now();
        let mut buf = [0u8; 4096];

        loop {
            if start.elapsed() > timeout {
                let _ = child.kill();
                let text = String::from_utf8_lossy(&output).to_string();
                return Ok(ToolOutput::error(format!("PTY command timed out\n{text}")));
            }

            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => output.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }

        let status = child
            .wait()
            .map_err(|e| crab_common::Error::Other(format!("failed to wait for PTY child: {e}")))?;

        let mut text = String::from_utf8_lossy(&output).to_string();

        if strip_ansi {
            if let Ok(stripped) = strip_ansi_escapes::strip_str(&text) {
                text = stripped;
            }
        }

        if status.success() {
            Ok(ToolOutput::success(text))
        } else {
            Ok(ToolOutput::with_content(
                vec![crab_core::tool::ToolOutputContent::Text { text }],
                true,
            ))
        }
    }
}

#[cfg(feature = "pty")]
pub use pty_support::PtyConfig;

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

    #[tokio::test]
    async fn bash_streaming_echo() {
        let tool = BashTool;
        let (streaming, mut rx) = crate::executor::StreamingOutput::channel(16);
        let input = serde_json::json!({ "command": "echo hello_stream" });
        let ctx = make_ctx();

        let handle = tokio::spawn(async move {
            tool.execute_streaming(input, &ctx, streaming).await
        });

        let result = handle.await.unwrap().unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("hello_stream"));

        // Collect streamed deltas
        let mut deltas = Vec::new();
        while let Some(d) = rx.recv().await {
            deltas.push(d);
        }
        assert!(!deltas.is_empty());
        let all: String = deltas.concat();
        assert!(all.contains("hello_stream"));
    }

    #[tokio::test]
    async fn bash_streaming_empty_command_is_error() {
        let tool = BashTool;
        let (streaming, _rx) = crate::executor::StreamingOutput::channel(1);
        let input = serde_json::json!({ "command": "" });
        let out = tool
            .execute_streaming(input, &make_ctx(), streaming)
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn bash_streaming_cancel() {
        let tool = BashTool;
        let (streaming, _rx) = crate::executor::StreamingOutput::channel(16);
        let cmd = if cfg!(windows) {
            "ping -n 30 127.0.0.1"
        } else {
            "sleep 30"
        };
        let input = serde_json::json!({ "command": cmd });

        let mut ctx = make_ctx();
        let cancel = ctx.cancellation_token.clone();

        // Cancel after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });

        let out = tool
            .execute_streaming(input, &ctx, streaming)
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("cancelled"));
    }
}
