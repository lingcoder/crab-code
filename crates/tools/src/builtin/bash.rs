use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolDisplayResult, ToolDisplayStyle, ToolOutput};
use crab_sandbox::spawn::{SpawnOptions, run};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing;

use crate::executor::StreamingOutput;
use crate::str_utils::truncate_chars;

/// Shell command execution tool.
pub const BASH_TOOL_NAME: &str = "Bash";

/// Default command timeout when the model omits one.
const DEFAULT_BASH_TIMEOUT_MS: u64 = 120_000;

/// Hard ceiling on a model-supplied timeout, enforcing the schema's documented
/// "max 600000" so a runaway value cannot pin the foreground turn.
const MAX_BASH_TIMEOUT_MS: u64 = 600_000;

/// Resolve a model-supplied timeout (in ms), applying the default and clamping
/// to [`MAX_BASH_TIMEOUT_MS`]. Shared by the `PowerShell` tool, which carries
/// the same documented ceiling.
pub(crate) fn resolve_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(
        timeout_ms.map_or(DEFAULT_BASH_TIMEOUT_MS, |ms| ms.min(MAX_BASH_TIMEOUT_MS)),
    )
}

/// Error message used when no suitable POSIX shell is on the host.
const NO_SHELL_ERROR: &str = "No suitable shell found. Bash tool requires a \
POSIX shell (bash or zsh). On Windows, install Git Bash \
(https://git-scm.com/) or WSL. You can also point `CRAB_SHELL` at a \
bash/zsh binary.";

/// Resolved shell binary path (e.g. `/bin/bash`, `C:/Program Files/Git/bin/bash.exe`).
///
/// Cached once per process; returns `None` if no suitable shell is found.
fn resolved_shell() -> Option<&'static String> {
    static SHELL: OnceLock<Option<String>> = OnceLock::new();
    SHELL.get_or_init(find_suitable_shell).as_ref()
}

/// Find bash/zsh in the order: `CRAB_SHELL` → `$SHELL` → PATH → common paths.
///
/// Only bash and zsh are accepted. `sh` (dash/ash) is rejected because it cannot
/// reliably execute bash syntax.
fn find_suitable_shell() -> Option<String> {
    // 1. Explicit override
    if let Ok(override_path) = std::env::var("CRAB_SHELL")
        && is_acceptable(&override_path)
        && std::path::Path::new(&override_path).is_file()
    {
        return Some(override_path);
    }

    // 2. User's $SHELL (Unix; rarely set on Windows but honor if bash-like)
    if let Ok(shell) = std::env::var("SHELL")
        && is_acceptable(&shell)
        && std::path::Path::new(&shell).is_file()
    {
        return Some(shell);
    }

    // 3. PATH search for bash, then zsh
    for bin in &["bash", "zsh"] {
        if let Some(p) = find_on_path(bin) {
            return Some(p);
        }
    }

    // 4. Common Unix install paths (no-op on Windows unless user copied bash there)
    let fallback_dirs = ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"];
    for dir in fallback_dirs {
        for bin in &["bash", "zsh"] {
            let candidate = std::path::Path::new(dir).join(bin);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    None
}

/// Path name contains "bash" or "zsh".
fn is_acceptable(path: &str) -> bool {
    path.contains("bash") || path.contains("zsh")
}

/// Walk `PATH` and return the first match for `bin` (with Windows extension handling).
fn find_on_path(bin: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exts: &[&str] = if cfg!(windows) { &[".exe", ""] } else { &[""] };
    for dir in path.split(sep) {
        for ext in exts {
            let candidate = std::path::Path::new(dir).join(format!("{bin}{ext}"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Build `(program, args)` for running `command` via the resolved shell, or
/// `None` if no shell is available (caller should return `NO_SHELL_ERROR`).
fn shell_invocation(command: String) -> Option<(String, Vec<String>)> {
    let shell = resolved_shell()?;
    Some((shell.clone(), vec!["-c".to_owned(), command]))
}

/// Filename prefix for background bash task output files in the temp dir.
const TASK_OUTPUT_PREFIX: &str = "crab-task-";
/// Filename suffix for background bash task output files.
const TASK_OUTPUT_SUFFIX: &str = ".output";
/// Age past which an orphaned task output file is swept on init.
const TASK_OUTPUT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Remove stale `crab-task-*.output` files from the temp dir.
///
/// Background bash tasks spill their output to `std::env::temp_dir()` for
/// `TaskOutput` to read back. Files outlive the process that created them, so
/// this sweeps any older than [`TASK_OUTPUT_MAX_AGE`]. Best-effort: I/O errors
/// are logged and skipped, never propagated.
pub fn sweep_stale_task_outputs() {
    let dir = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(TASK_OUTPUT_PREFIX) || !name.ends_with(TASK_OUTPUT_SUFFIX) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > TASK_OUTPUT_MAX_AGE);
        if stale && let Err(e) = std::fs::remove_file(entry.path()) {
            tracing::warn!(path = %entry.path().display(), "failed to sweep stale task output: {e}");
        }
    }
}

/// Derive the sandbox policy for a shell invocation from the permission mode
/// and working directory. `None` disables sandboxing (Dangerously mode, or the
/// global switch off). Shared by the Bash and PowerShell tools.
pub(crate) fn command_sandbox_policy(ctx: &ToolContext) -> Option<crab_sandbox::SandboxPolicy> {
    crab_sandbox::SandboxPolicy::for_mode(ctx.permission_mode, &ctx.working_dir)
}

/// Whether the current platform actually enforces the sandbox. Used to avoid
/// attributing a failure to the sandbox on Windows (fail-open) where a
/// "permission denied" is never a sandbox denial.
const SANDBOX_ENFORCED_PLATFORM: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// If a command ran under an enforcing sandbox and failed in a way that looks
/// like a sandbox denial, return the hint to append so the model/user knows.
pub(crate) fn maybe_sandbox_denial_hint(
    sandboxed: bool,
    exit_code: i32,
    combined: &str,
) -> Option<&'static str> {
    if sandboxed
        && SANDBOX_ENFORCED_PLATFORM
        && crab_sandbox::is_likely_sandbox_denied(exit_code, combined)
    {
        Some(crab_sandbox::SANDBOX_DENIAL_HINT)
    } else {
        None
    }
}

/// Append the denial hint to `text` when one applies.
pub(crate) fn append_denial_hint(mut text: String, sandboxed: bool, exit_code: i32) -> String {
    if let Some(hint) = maybe_sandbox_denial_hint(sandboxed, exit_code, &text) {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(hint);
    }
    text
}

/// Combine stdout and stderr from a process output into a single string.
fn format_output(output: &crab_sandbox::spawn::SpawnOutput) -> String {
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
    combined
}

/// Join the streaming reader tasks and concatenate their captured lines into a
/// single newline-separated string. Used by the timeout/cancel branches to
/// recover whatever output arrived before the child was killed.
/// Terminate a streaming child and any processes it spawned.
///
/// On Unix a simple `bash -c "<cmd>"` execs into the command, so killing the
/// child PID is enough. On Windows the shell launches the command as a separate
/// child that outlives a plain kill and keeps the inherited stdout/stderr pipes
/// open, so terminate the whole tree to let the pipes close promptly.
async fn kill_streaming_child(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output()
            .await;
    }
    let _ = child.start_kill();
}

async fn join_streaming_output(
    stdout_handle: tokio::task::JoinHandle<Vec<String>>,
    stderr_handle: tokio::task::JoinHandle<Vec<String>>,
) -> String {
    // After a kill, the reader tasks finish once the child's pipes close. A
    // grandchild that inherited a pipe (e.g. a command launched via the shell)
    // can keep it open until it exits on its own, so bound the wait and abort
    // the readers rather than block for the full command duration.
    let stdout_abort = stdout_handle.abort_handle();
    let stderr_abort = stderr_handle.abort_handle();
    let collect = async {
        let stdout_lines = stdout_handle.await.unwrap_or_default();
        let stderr_lines = stderr_handle.await.unwrap_or_default();
        let mut combined = String::new();
        for line in stdout_lines.iter().chain(stderr_lines.iter()) {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(line);
        }
        combined
    };
    if let Ok(combined) = tokio::time::timeout(Duration::from_millis(500), collect).await {
        combined
    } else {
        stdout_abort.abort();
        stderr_abort.abort();
        String::new()
    }
}

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        BASH_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Executes a given bash command with optional timeout. Working directory persists \
         between commands; shell state does not. Requires bash or zsh — on Windows, Git Bash \
         is automatically used when available. Returns stdout and stderr combined; non-zero \
         exit marks the result as an error."
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
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Set to true to run this command in the background. Use TaskOutput to read the output later."
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
        let run_in_background = input["run_in_background"].as_bool().unwrap_or(false);
        let task_registry = ctx.task_registry.clone();
        let sandbox_policy = command_sandbox_policy(ctx);
        let sandboxed = sandbox_policy.is_some();

        Box::pin(async move {
            if command.is_empty() {
                return Ok(ToolOutput::error("command is required"));
            }

            let timeout = Some(resolve_timeout(timeout_ms));

            let Some((prog, args)) = shell_invocation(command.clone()) else {
                return Ok(ToolOutput::error(NO_SHELL_ERROR));
            };

            // ── Background mode ──────────────────────────────────────
            if run_in_background {
                let Some(reg) = task_registry else {
                    return Ok(ToolOutput::error(
                        "run_in_background requires a task registry (not available in this context)",
                    ));
                };

                let task_id = crab_core::task::generate_task_id('b');
                let entry = crab_core::task::TaskEntry::new(
                    task_id.clone(),
                    crab_core::task::TaskType::LocalBash,
                    command.clone(),
                );

                if !reg
                    .lock()
                    .unwrap_or_else(|e| {
                        tracing::warn!("task registry mutex poisoned: {e}");
                        e.into_inner()
                    })
                    .register(entry)
                {
                    return Ok(ToolOutput::error("task ID collision — retry"));
                }

                // Set up an output file for TaskOutputTool to read later.
                let output_path = std::env::temp_dir()
                    .join(format!("{TASK_OUTPUT_PREFIX}{task_id}{TASK_OUTPUT_SUFFIX}"));
                let output_path_str = output_path.to_string_lossy().to_string();
                reg.lock()
                    .unwrap_or_else(|e| {
                        tracing::warn!("task registry mutex poisoned: {e}");
                        e.into_inner()
                    })
                    .set_output_path(&task_id, output_path_str);

                // Spawn the command as a detached tokio task.
                let task_id_spawn = task_id.clone();
                let reg_spawn = Arc::clone(&reg);
                tokio::spawn(async move {
                    let opts = SpawnOptions {
                        command: prog,
                        args,
                        working_dir: Some(working_dir),
                        env: vec![],
                        timeout,
                        stdin_data: None,
                        clear_env: false,
                        kill_grace_period: None,
                        sandbox_policy,
                    };

                    reg_spawn
                        .lock()
                        .unwrap_or_else(|e| {
                            tracing::warn!("task registry mutex poisoned: {e}");
                            e.into_inner()
                        })
                        .set_status(&task_id_spawn, crab_core::task::TaskStatus::Running);

                    let result = run(opts).await;

                    // Write combined output to the file.
                    let mut reg = reg_spawn.lock().unwrap_or_else(|e| {
                        tracing::warn!("task registry mutex poisoned: {e}");
                        e.into_inner()
                    });
                    match result {
                        Ok(output) => {
                            let combined = format_output(&output);
                            if let Err(e) = std::fs::write(&output_path, &combined) {
                                tracing::warn!(
                                    task = %task_id_spawn,
                                    path = %output_path.display(),
                                    "failed to write background task output: {e}"
                                );
                            }
                            reg.set_exit_code(&task_id_spawn, output.exit_code);
                        }
                        Err(e) => {
                            if let Err(write_err) = std::fs::write(&output_path, e.to_string()) {
                                tracing::warn!(
                                    task = %task_id_spawn,
                                    path = %output_path.display(),
                                    "failed to write background task error: {write_err}"
                                );
                            }
                            reg.set_error(&task_id_spawn, e.to_string());
                        }
                    }
                });

                return Ok(ToolOutput::success(format!(
                    "Task {task_id} started in the background. Use TaskOutput with task_id=\"{task_id}\" to read output."
                )));
            }

            // ── Foreground mode (original behavior) ──────────────────
            let opts = SpawnOptions {
                command: prog,
                args,
                working_dir: Some(working_dir),
                env: vec![],
                timeout,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
                sandbox_policy,
            };

            let output = run(opts).await?;
            let combined = format_output(&output);

            if output.timed_out {
                return Ok(ToolOutput::error(format!("Command timed out\n{combined}")));
            }

            if output.exit_code != 0 {
                let text = if combined.is_empty() {
                    format!("Exit code: {}", output.exit_code)
                } else {
                    combined
                };
                let text = append_denial_hint(text, sandboxed, output.exit_code);
                Ok(ToolOutput::with_content(
                    vec![crab_core::tool::ToolOutputContent::Text { text }],
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
        let cmd = input["command"].as_str()?;
        let display = truncate_chars(cmd, 160, "…");
        Some(format!("Bash({display})"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let text = output.text();
        // stdout in normal color, stderr in error color.
        // We don't have separate stdout/stderr here, so use is_error flag.
        if text.is_empty() {
            // "(No output)" dimmed when empty
            return Some(ToolDisplayResult {
                lines: vec![ToolDisplayLine::new("(No output)", ToolDisplayStyle::Muted)],
                preview_lines: 1,
            });
        }
        let style = if output.is_error {
            ToolDisplayStyle::Error
        } else {
            ToolDisplayStyle::Normal
        };
        let all_lines: Vec<&str> = text.lines().collect();
        let total = all_lines.len();
        // Show up to 20 lines with count indicator (last 5 used for progress).
        let show = total.min(20);
        let mut lines: Vec<ToolDisplayLine> = all_lines[..show]
            .iter()
            .map(|l| ToolDisplayLine::new(*l, style))
            .collect();
        if total > show {
            lines.push(ToolDisplayLine::new(
                format!("… +{} lines", total - show),
                ToolDisplayStyle::Muted,
            ));
        }
        Some(ToolDisplayResult {
            lines,
            preview_lines: 5,
        })
    }

    fn format_rejected_summary(&self, input: &Value) -> Option<String> {
        input["command"]
            .as_str()
            .map(|cmd| format!("Bash rejected ({cmd})"))
    }

    fn format_rejected(&self, input: &Value) -> Option<ToolDisplayResult> {
        use crab_core::tool::ToolDisplayLine;
        let cmd = input["command"].as_str()?;
        let preview: Vec<&str> = cmd.lines().take(3).collect();
        let mut lines = Vec::new();
        for line in &preview {
            lines.push(ToolDisplayLine::new(*line, ToolDisplayStyle::Highlight));
        }
        if cmd.lines().count() > 3 {
            lines.push(ToolDisplayLine::new(
                format!("... ({} more lines)", cmd.lines().count() - 3),
                ToolDisplayStyle::Muted,
            ));
        }
        Some(ToolDisplayResult {
            lines,
            preview_lines: 3,
        })
    }

    fn supports_streaming_progress(&self) -> bool {
        true
    }

    fn format_error(&self, output: &ToolOutput, input: &Value) -> Option<ToolDisplayResult> {
        use crab_core::tool::ToolDisplayLine;
        let text = output.text();
        let cmd = input["command"]
            .as_str()
            .map(|c| truncate_chars(c, 80, "…"))
            .unwrap_or_default();

        let mut lines = vec![ToolDisplayLine::new(
            format!("Command failed: {cmd}"),
            ToolDisplayStyle::Error,
        )];

        let tail: Vec<&str> = text.lines().rev().take(3).collect();
        for line in tail.into_iter().rev() {
            lines.push(ToolDisplayLine::new(line, ToolDisplayStyle::Muted));
        }

        if text.contains("not found") || text.contains("not recognized") {
            lines.push(ToolDisplayLine::new(
                "Hint: Command not found — check spelling or PATH",
                ToolDisplayStyle::Muted,
            ));
        } else if text.contains("Permission denied") {
            lines.push(ToolDisplayLine::new(
                "Hint: Permission denied — check file permissions",
                ToolDisplayStyle::Muted,
            ));
        }

        Some(ToolDisplayResult {
            lines,
            preview_lines: 2,
        })
    }

    fn display_color(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::Highlight
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
        let sandbox_policy = command_sandbox_policy(ctx);
        let sandboxed = sandbox_policy.is_some();

        if command.is_empty() {
            return Ok(ToolOutput::error("command is required"));
        }

        let timeout = resolve_timeout(timeout_ms);

        let Some((prog, args)) = shell_invocation(command) else {
            return Ok(ToolOutput::error(NO_SHELL_ERROR));
        };

        // Transform the command through the sandbox before spawning. This path
        // keeps its own streaming/cancel loop, so it applies the sandbox in
        // place rather than going through `crab_sandbox::spawn::run_streaming`.
        let mut sandboxed_command = if let Some(policy) = &sandbox_policy {
            crab_sandbox::prepare_command(policy, &prog, &args, &working_dir)?.command
        } else {
            let mut c = tokio::process::Command::new(&prog);
            c.args(&args);
            c
        };
        sandboxed_command
            .current_dir(&working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = sandboxed_command
            .spawn()
            .map_err(|e| crab_core::Error::Other(format!("failed to spawn: {e}")))?;

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
                        let text = if combined.is_empty() {
                            format!("Exit code: {code}")
                        } else {
                            combined
                        };
                        let text = append_denial_hint(text, sandboxed, code);
                        Ok(ToolOutput::with_content(
                            vec![crab_core::tool::ToolOutputContent::Text { text }],
                            true,
                        ))
                    }
                    Err(e) => Ok(ToolOutput::error(format!("process error: {e}"))),
                }
            }
            () = tokio::time::sleep(timeout) => {
                kill_streaming_child(&mut child).await;
                let partial = join_streaming_output(stdout_handle, stderr_handle).await;
                Ok(ToolOutput::error(format!("Command timed out\n{partial}")))
            }
            () = cancel.cancelled() => {
                kill_streaming_child(&mut child).await;
                let partial = join_streaming_output(stdout_handle, stderr_handle).await;
                Ok(ToolOutput::error(format!("Command cancelled\n{partial}")))
            }
        };

        result
    }
}

// ─── PTY support (feature-gated) ───

#[cfg(feature = "pty")]
mod pty_support {
    use super::{BashTool, Duration, NO_SHELL_ERROR, Result, ToolOutput, resolved_shell};

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
            .map_err(|e| crab_core::Error::Other(format!("PTY task failed: {e}")))??;

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

        // `portable-pty` builds its child via its own `CommandBuilder`, which
        // exposes no `pre_exec` hook and takes no `tokio::process::Command`, so
        // the Landlock/Seatbelt transform cannot be applied here. PTY execution
        // therefore runs unsandboxed in this MVP.
        tracing::warn!("PTY execution runs without sandbox isolation");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| crab_core::Error::Other(format!("failed to open PTY: {e}")))?;

        let Some(shell) = resolved_shell() else {
            return Ok(ToolOutput::error(NO_SHELL_ERROR));
        };

        let mut cmd = CommandBuilder::new(shell.clone());
        cmd.arg("-c");
        cmd.arg(command);
        cmd.cwd(working_dir);

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| crab_core::Error::Other(format!("failed to spawn in PTY: {e}")))?;

        // Drop the slave — we read from master
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| crab_core::Error::Other(format!("failed to clone PTY reader: {e}")))?;

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
            .map_err(|e| crab_core::Error::Other(format!("failed to wait for PTY child: {e}")))?;

        let mut text = String::from_utf8_lossy(&output).to_string();

        if strip_ansi {
            text = strip_ansi_escapes::strip_str(&text);
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

    // `Dangerously` disables the sandbox so these tests exercise bash I/O
    // (echo/stream/cancel/background) without coupling to whether the host
    // kernel enforces Landlock. Sandbox behaviour is covered in `crab-sandbox`.
    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            task_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
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
        let cmd = "exit 1";
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

    #[test]
    fn resolve_timeout_applies_default_and_clamp() {
        assert_eq!(
            resolve_timeout(None),
            Duration::from_millis(DEFAULT_BASH_TIMEOUT_MS)
        );
        assert_eq!(resolve_timeout(Some(5_000)), Duration::from_millis(5_000));
        // Oversized values clamp to the documented ceiling.
        assert_eq!(
            resolve_timeout(Some(99_999_999)),
            Duration::from_millis(MAX_BASH_TIMEOUT_MS)
        );
    }

    #[tokio::test]
    async fn bash_streaming_echo() {
        let tool = BashTool;
        let (streaming, mut rx) = crate::executor::StreamingOutput::channel(16);
        let input = serde_json::json!({ "command": "echo hello_stream" });
        let ctx = make_ctx();

        let handle =
            tokio::spawn(async move { tool.execute_streaming(input, &ctx, streaming).await });

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

        let ctx = make_ctx();
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

    #[tokio::test]
    async fn bash_background_registers_task() {
        use crab_core::task::{TaskRegistry, TaskStatus};

        let registry = Arc::new(std::sync::Mutex::new(TaskRegistry::new()));
        let mut ctx = make_ctx();
        ctx.task_registry = Some(Arc::clone(&registry));

        let tool = BashTool;
        let input = serde_json::json!({
            "command": "echo hello",
            "run_in_background": true
        });
        let out = tool.execute(input, &ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("started in the background"));

        // Extract task_id from the response.
        let text = out.text();
        let task_id = text
            .split("Task ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap();
        assert!(task_id.starts_with('b'));

        // Wait a bit for the background task to finish.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let status = registry.lock().unwrap().get(task_id).unwrap().status;
        assert!(
            status == TaskStatus::Completed || status == TaskStatus::Running,
            "unexpected status: {status:?}",
        );
    }

    #[tokio::test]
    async fn bash_background_no_registry_is_error() {
        let ctx = make_ctx(); // task_registry is None
        let tool = BashTool;
        let input = serde_json::json!({
            "command": "echo hello",
            "run_in_background": true
        });
        let out = tool.execute(input, &ctx).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("task registry"));
    }
}
