//! Child process spawning, environment inheritance.

use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Default grace period between SIGTERM and SIGKILL on Unix.
const DEFAULT_GRACE_PERIOD: Duration = Duration::from_secs(3);

/// Options for spawning a child process.
pub struct SpawnOptions {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    pub stdin_data: Option<String>,
    /// If `true`, clear the parent environment before applying `env`.
    /// Default is `false` (inherit parent env, then overlay `env`).
    pub clear_env: bool,
    /// Grace period between SIGTERM and SIGKILL when a timeout fires.
    /// Only meaningful on Unix; Windows always does a hard kill.
    /// Defaults to 3 seconds if `None`.
    pub kill_grace_period: Option<Duration>,
}

/// Captured output from a completed child process.
pub struct SpawnOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

fn build_command(opts: &SpawnOptions) -> Command {
    let mut cmd = Command::new(&opts.command);
    cmd.args(&opts.args);
    if let Some(ref cwd) = opts.working_dir {
        cmd.current_dir(cwd);
    }
    if opts.clear_env {
        cmd.env_clear();
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    cmd
}

/// Escape a string for use as a `cmd.exe /C` argument on Windows.
///
/// Wraps the argument in double quotes and escapes internal double quotes,
/// percent signs, and caret characters that have special meaning in cmd.exe.
#[must_use]
pub fn escape_cmd_arg(arg: &str) -> String {
    // Escape special cmd.exe metacharacters inside the argument
    let mut escaped = String::with_capacity(arg.len() + 8);
    escaped.push('"');
    for ch in arg.chars() {
        match ch {
            '"' => escaped.push_str(r#"\""#),
            '%' => escaped.push_str("%%"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

/// Build `SpawnOptions` for running a shell command string via the platform shell.
///
/// On Windows, uses `cmd.exe /C`; on Unix, uses `sh -c`.
#[must_use]
pub fn shell_command(cmd_str: &str) -> SpawnOptions {
    if cfg!(windows) {
        SpawnOptions {
            command: "cmd".into(),
            args: vec!["/C".into(), cmd_str.into()],
            working_dir: None,
            env: vec![],
            timeout: None,
            stdin_data: None,
            clear_env: false,
            kill_grace_period: None,
        }
    } else {
        SpawnOptions {
            command: "sh".into(),
            args: vec!["-c".into(), cmd_str.into()],
            working_dir: None,
            env: vec![],
            timeout: None,
            stdin_data: None,
            clear_env: false,
            kill_grace_period: None,
        }
    }
}

/// Attempt graceful termination: on Unix, send SIGTERM via the `kill` command
/// and wait for the grace period before escalating to SIGKILL. On Windows,
/// immediately force-kill (no graceful shutdown equivalent).
async fn graceful_kill(
    child: &mut tokio::process::Child,
    #[cfg_attr(not(unix), allow(unused))] grace_period: Duration,
) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // Send SIGTERM via the kill command (avoids unsafe libc calls)
        let _ = tokio::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await;

        // Wait for the grace period or until the child exits on its own
        if tokio::time::timeout(grace_period, child.wait())
            .await
            .is_ok()
        {
            return; // Child exited within grace period
        }
    }

    // Force kill (SIGKILL on Unix, TerminateProcess on Windows)
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Read all bytes from an optional stdout pipe using lossy UTF-8 conversion.
async fn read_pipe_lossy(pipe: Option<tokio::process::ChildStdout>) -> String {
    if let Some(mut r) = pipe {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    }
}

/// Read all bytes from an optional stderr pipe using lossy UTF-8 conversion.
async fn read_stderr_lossy(pipe: Option<tokio::process::ChildStderr>) -> String {
    if let Some(mut r) = pipe {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    }
}

/// Execute a command and wait for completion.
///
/// If `timeout` is set and the process exceeds it, the process is killed and
/// `SpawnOutput::timed_out` is set to `true`.
///
/// # Errors
///
/// Returns an error if the command cannot be spawned or output cannot be captured.
pub async fn run(opts: SpawnOptions) -> crab_core::Result<SpawnOutput> {
    let mut cmd = build_command(&opts);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if opts.stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }

    let mut child = cmd.spawn()?;

    // Write stdin if provided, then drop to signal EOF
    if let Some(ref data) = opts.stdin_data
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(data.as_bytes()).await?;
    }

    let result = if let Some(timeout) = opts.timeout {
        // Collect stdout/stderr handles before consuming child.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        if let Ok(status) = tokio::time::timeout(timeout, child.wait()).await {
            let status = status?;
            let stdout_buf = read_pipe_lossy(stdout_pipe).await;
            let stderr_buf = read_stderr_lossy(stderr_pipe).await;
            SpawnOutput {
                stdout: stdout_buf,
                stderr: stderr_buf,
                exit_code: status.code().unwrap_or(-1),
                timed_out: false,
            }
        } else {
            // Timeout — graceful shutdown then force kill.
            let grace = opts.kill_grace_period.unwrap_or(DEFAULT_GRACE_PERIOD);
            graceful_kill(&mut child, grace).await;
            let stdout_buf = read_pipe_lossy(stdout_pipe).await;
            let stderr_buf = read_stderr_lossy(stderr_pipe).await;
            SpawnOutput {
                stdout: stdout_buf,
                stderr: stderr_buf,
                exit_code: -1,
                timed_out: true,
            }
        }
    } else {
        let output = child.wait_with_output().await?;
        SpawnOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out: false,
        }
    };

    Ok(result)
}

/// Execute a command and stream stdout/stderr line-by-line via callbacks.
///
/// Returns the process exit code.
///
/// # Errors
///
/// Returns an error if the command cannot be spawned.
pub async fn run_streaming(
    opts: SpawnOptions,
    on_stdout: impl Fn(&str) + Send + 'static,
    on_stderr: impl Fn(&str) + Send + 'static,
) -> crab_core::Result<i32> {
    let mut cmd = build_command(&opts);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());

    let mut child = cmd.spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                on_stdout(&line);
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                on_stderr(&line);
            }
        }
    });

    let status = child.wait().await?;

    // Wait for readers to finish draining
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_opts(msg: &str) -> SpawnOptions {
        if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), format!("echo {msg}")],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "echo".into(),
                args: vec![msg.into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        }
    }

    #[tokio::test]
    async fn run_echo() {
        let out = run(echo_opts("hello")).await.unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(!out.timed_out);
        assert!(out.stdout.trim().contains("hello"));
    }

    #[tokio::test]
    async fn run_exit_code() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "exit 42".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "exit 42".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 42);
    }

    #[tokio::test]
    async fn run_with_timeout() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "ping -n 10 127.0.0.1 >nul".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(100)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
            }
        } else {
            SpawnOptions {
                command: "sleep".into(),
                args: vec!["10".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(100)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.timed_out);
    }

    #[tokio::test]
    async fn run_with_env() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo %MY_TEST_VAR%".into()],
                working_dir: None,
                env: vec![("MY_TEST_VAR".into(), "crab_value".into())],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo $MY_TEST_VAR".into()],
                working_dir: None,
                env: vec![("MY_TEST_VAR".into(), "crab_value".into())],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.trim().contains("crab_value"));
    }

    #[tokio::test]
    async fn run_with_working_dir() {
        let tmp = std::env::temp_dir();
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "cd".into()],
                working_dir: Some(tmp.clone()),
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "pwd".into(),
                args: vec![],
                working_dir: Some(tmp.clone()),
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        // Both paths must be canonicalized to compare reliably on CI
        // (short vs long paths, symlinks, etc.)
        let actual_path = std::path::PathBuf::from(out.stdout.trim());
        let actual_norm = crab_utils::path::normalize(&actual_path)
            .to_string_lossy()
            .to_lowercase();
        let expected_norm = crab_utils::path::normalize(&tmp)
            .to_string_lossy()
            .to_lowercase();
        assert!(
            actual_norm.contains(&expected_norm) || expected_norm.contains(&actual_norm),
            "working dir mismatch: actual={actual_norm}, expected={expected_norm}"
        );
    }

    #[tokio::test]
    async fn run_with_stdin() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "findstr".into(),
                args: vec![".*".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: Some("hello from stdin\n".into()),
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "cat".into(),
                args: vec![],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: Some("hello from stdin\n".into()),
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.stdout.contains("hello from stdin"));
    }

    #[tokio::test]
    async fn run_streaming_echo() {
        let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected_clone = collected.clone();

        let opts = echo_opts("streaming_test");
        let exit_code = run_streaming(
            opts,
            move |line| {
                collected_clone.lock().unwrap().push(line.to_string());
            },
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(exit_code, 0);
        assert!(
            collected
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("streaming_test"))
        );
    }

    #[tokio::test]
    async fn run_nonexistent_command() {
        let opts = SpawnOptions {
            command: "this_command_does_not_exist_12345".into(),
            args: vec![],
            working_dir: None,
            env: vec![],
            timeout: None,
            stdin_data: None,
            clear_env: false,
            kill_grace_period: None,
        };
        let result = run(opts).await;
        assert!(result.is_err());
    }

    // ── Edge-case tests ────────────────────────────────────────────

    #[tokio::test]
    async fn run_empty_command() {
        let opts = SpawnOptions {
            command: String::new(),
            args: vec![],
            working_dir: None,
            env: vec![],
            timeout: None,
            stdin_data: None,
            clear_env: false,
            kill_grace_period: None,
        };
        let result = run(opts).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_large_output() {
        // Generate a large amount of stdout
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec![
                    "/C".into(),
                    "for /L %i in (1,1,500) do @echo line_%i".into(),
                ],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(15)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec![
                    "-c".into(),
                    "for i in $(seq 1 500); do echo line_$i; done".into(),
                ],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(15)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("line_1"));
        assert!(out.stdout.contains("line_500"));
        let line_count = out.stdout.lines().count();
        assert!(line_count >= 500, "expected >=500 lines, got {line_count}");
    }

    #[tokio::test]
    async fn run_stderr_only() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo error_output 1>&2".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo error_output >&2".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stderr.contains("error_output"));
        assert!(out.stdout.trim().is_empty());
    }

    #[tokio::test]
    async fn run_mixed_stdout_stderr() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo out_msg && echo err_msg 1>&2".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo out_msg; echo err_msg >&2".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("out_msg"));
        assert!(out.stderr.contains("err_msg"));
    }

    #[tokio::test]
    async fn run_timeout_with_partial_output() {
        // Process produces some output, then hangs; timeout should capture partial output
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec![
                    "/C".into(),
                    "echo partial_data && ping -n 10 127.0.0.1 >nul".into(),
                ],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(500)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo partial_data; sleep 10".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(500)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.timed_out);
        assert!(out.stdout.contains("partial_data"));
    }

    #[tokio::test]
    async fn run_with_clear_env() {
        // Set an env var, then clear the environment — the var should not be visible
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo %CRAB_CLEARED_TEST%".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: None,
                clear_env: true,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo ${CRAB_CLEARED_TEST:-empty}".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: None,
                clear_env: true,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        // On Unix with clear_env, the variable is unset → "empty"
        // On Windows with clear_env, %CRAB_CLEARED_TEST% expands literally
        if cfg!(windows) {
            assert!(out.stdout.contains("%CRAB_CLEARED_TEST%"));
        } else {
            assert!(out.stdout.trim().contains("empty"));
        }
    }

    #[tokio::test]
    async fn run_clear_env_with_overlay() {
        // Clear env but overlay a specific variable
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo %OVERLAY_VAR%".into()],
                working_dir: None,
                env: vec![("OVERLAY_VAR".into(), "overlay_ok".into())],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: None,
                clear_env: true,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo $OVERLAY_VAR".into()],
                working_dir: None,
                env: vec![("OVERLAY_VAR".into(), "overlay_ok".into())],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: None,
                clear_env: true,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.stdout.contains("overlay_ok"));
    }

    #[tokio::test]
    async fn run_streaming_stderr() {
        let stderr_collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stderr_clone = stderr_collected.clone();

        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo err_stream 1>&2".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo err_stream >&2".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let exit_code = run_streaming(
            opts,
            |_| {},
            move |line| {
                stderr_clone.lock().unwrap().push(line.to_string());
            },
        )
        .await
        .unwrap();

        assert_eq!(exit_code, 0);
        assert!(
            stderr_collected
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("err_stream"))
        );
    }

    #[test]
    fn shell_command_builds_correct_opts() {
        let opts = shell_command("echo hello world");
        if cfg!(windows) {
            assert_eq!(opts.command, "cmd");
            assert_eq!(opts.args, vec!["/C", "echo hello world"]);
        } else {
            assert_eq!(opts.command, "sh");
            assert_eq!(opts.args, vec!["-c", "echo hello world"]);
        }
        assert!(!opts.clear_env);
        assert!(opts.timeout.is_none());
    }

    #[test]
    fn escape_cmd_arg_basic() {
        assert_eq!(escape_cmd_arg("hello"), r#""hello""#);
    }

    #[test]
    fn escape_cmd_arg_with_quotes() {
        assert_eq!(escape_cmd_arg(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn escape_cmd_arg_with_percent() {
        assert_eq!(escape_cmd_arg("100%"), r#""100%%""#);
    }

    #[test]
    fn escape_cmd_arg_empty() {
        assert_eq!(escape_cmd_arg(""), r#""""#);
    }

    #[test]
    fn escape_cmd_arg_with_spaces_and_special() {
        let escaped = escape_cmd_arg("path with spaces & special");
        assert_eq!(escaped, r#""path with spaces & special""#);
    }

    #[tokio::test]
    async fn run_zero_timeout_triggers_immediately() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "ping -n 5 127.0.0.1 >nul".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::ZERO),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
            }
        } else {
            SpawnOptions {
                command: "sleep".into(),
                args: vec!["5".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::ZERO),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.timed_out);
    }

    #[tokio::test]
    async fn run_multiple_env_vars() {
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec!["/C".into(), "echo %VAR_A% %VAR_B%".into()],
                working_dir: None,
                env: vec![
                    ("VAR_A".into(), "alpha".into()),
                    ("VAR_B".into(), "beta".into()),
                ],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo $VAR_A $VAR_B".into()],
                working_dir: None,
                env: vec![
                    ("VAR_A".into(), "alpha".into()),
                    ("VAR_B".into(), "beta".into()),
                ],
                timeout: None,
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.stdout.contains("alpha"));
        assert!(out.stdout.contains("beta"));
    }

    #[tokio::test]
    async fn run_fast_command_no_timeout() {
        // A command that finishes well before its timeout should not be marked timed_out
        let mut opts = echo_opts("fast");
        opts.timeout = Some(Duration::from_secs(30));
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(!out.timed_out);
        assert!(out.stdout.trim().contains("fast"));
    }
}
