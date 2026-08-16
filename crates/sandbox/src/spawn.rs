//! Child process spawning, environment inheritance.

use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::task::JoinHandle;

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
    /// Sandbox policy to enforce on the spawned command. `None` runs the
    /// command with full privileges (the right choice for trusted subprocesses
    /// like hooks, MCP servers, and network fetches). `Some` transforms the
    /// command through [`crate::prepare_command`] before spawning (argv wrap on macOS,
    /// `pre_exec` ruleset on Linux).
    pub sandbox_policy: Option<crate::SandboxPolicy>,
}

/// Captured output from a completed child process.
pub struct SpawnOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

/// Build the `Command`, applying the sandbox policy (if any) before setting the
/// working directory and environment.
///
/// # Errors
///
/// Returns an error when a fail-closed sandbox backend (Linux/macOS) is asked
/// to enforce a policy it cannot provide.
fn build_command(opts: &SpawnOptions) -> crab_core::Result<Command> {
    let mut cmd = if let Some(policy) = &opts.sandbox_policy {
        let cwd = opts
            .working_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        crate::prepare_command(policy, &opts.command, &opts.args, &cwd)?.command
    } else {
        let mut cmd = Command::new(&opts.command);
        cmd.args(&opts.args);
        cmd
    };
    if let Some(ref cwd) = opts.working_dir {
        cmd.current_dir(cwd);
    }
    if opts.clear_env {
        cmd.env_clear();
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    Ok(cmd)
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
            sandbox_policy: None,
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
            sandbox_policy: None,
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

/// Drain an async reader to EOF, returning the bytes read. Spawned as its own
/// task so that stdout and stderr are consumed concurrently with the child's
/// execution — a child that writes more than a pipe buffer (~64KB) would
/// otherwise block on write while the parent waits, producing a false timeout.
fn spawn_drain<R>(reader: Option<R>) -> JoinHandle<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut r) = reader {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf).await;
        }
        buf
    })
}

/// Await a drain task, converting captured bytes to a lossy UTF-8 string.
async fn join_drain(handle: JoinHandle<Vec<u8>>) -> String {
    let buf = handle.await.unwrap_or_default();
    String::from_utf8_lossy(&buf).into_owned()
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
    let mut cmd = build_command(&opts)?;
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if opts.stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }

    let mut child = cmd.spawn()?;

    // Drain stdout/stderr in their own tasks and feed stdin concurrently, so a
    // child that produces (or expects) more than a pipe buffer's worth of data
    // never blocks on write while we wait for it to exit.
    let stdout_handle = spawn_drain(child.stdout.take());
    let stderr_handle = spawn_drain(child.stderr.take());

    if let Some(data) = opts.stdin_data.clone()
        && let Some(mut stdin) = child.stdin.take()
    {
        tokio::spawn(async move {
            let _ = stdin.write_all(data.as_bytes()).await;
            // Dropping `stdin` here signals EOF to the child.
        });
    }

    let result = if let Some(timeout) = opts.timeout {
        if let Ok(status) = tokio::time::timeout(timeout, child.wait()).await {
            let status = status?;
            SpawnOutput {
                stdout: join_drain(stdout_handle).await,
                stderr: join_drain(stderr_handle).await,
                exit_code: status.code().unwrap_or(-1),
                timed_out: false,
            }
        } else {
            // Timeout — graceful shutdown then force kill.
            let grace = opts.kill_grace_period.unwrap_or(DEFAULT_GRACE_PERIOD);
            graceful_kill(&mut child, grace).await;
            SpawnOutput {
                stdout: join_drain(stdout_handle).await,
                stderr: join_drain(stderr_handle).await,
                exit_code: -1,
                timed_out: true,
            }
        }
    } else {
        let status = child.wait().await?;
        SpawnOutput {
            stdout: join_drain(stdout_handle).await,
            stderr: join_drain(stderr_handle).await,
            exit_code: status.code().unwrap_or(-1),
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
    let mut cmd = build_command(&opts)?;
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
            sandbox_policy: None,
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
            sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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
                sandbox_policy: None,
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

    #[tokio::test]
    async fn run_large_output_with_timeout_does_not_deadlock() {
        // A child emitting well past the OS pipe buffer (~64KB) must complete
        // promptly and return the full output even when a timeout is set —
        // this is the regression guard for the read-after-wait deadlock.
        const LINES: usize = 8_000; // ~256KB at ~32 bytes/line
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec![
                    "/C".into(),
                    format!("for /L %i in (1,1,{LINES}) do @echo padding_line_number_%i"),
                ],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(60)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
                sandbox_policy: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec![
                    "-c".into(),
                    format!("for i in $(seq 1 {LINES}); do echo padding_line_number_$i; done"),
                ],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(60)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
                sandbox_policy: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert!(!out.timed_out, "large output falsely timed out");
        assert_eq!(out.exit_code, 0);
        // The child emits well past a pipe buffer (>64KB). A truncated read
        // would lose the final line, so assert the last line survived and that
        // the total comfortably exceeds the buffer.
        assert!(
            out.stdout.len() > 128 * 1024,
            "output truncated: {} bytes",
            out.stdout.len()
        );
        assert!(out.stdout.contains(&format!("padding_line_number_{LINES}")));
    }

    #[tokio::test]
    async fn run_large_stdin_with_large_output_no_deadlock() {
        // Large stdin combined with large stdout would two-way deadlock if
        // stdin were written serially before reading: the child blocks writing
        // its echo (filling the stdout pipe) while we block writing stdin.
        // Many short lines stay within `findstr`'s line-length limit on Windows
        // while still pushing both directions past the pipe buffer.
        const LINES: usize = 8_000;
        let payload = (0..LINES).fold(String::new(), |mut s, i| {
            use std::fmt::Write;
            let _ = writeln!(s, "echo_line_{i}");
            s
        });
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "findstr".into(),
                args: vec!["echo_line".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(60)),
                stdin_data: Some(payload),
                clear_env: false,
                kill_grace_period: None,
                sandbox_policy: None,
            }
        } else {
            SpawnOptions {
                command: "cat".into(),
                args: vec![],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(60)),
                stdin_data: Some(payload),
                clear_env: false,
                kill_grace_period: None,
                sandbox_policy: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert!(!out.timed_out, "large stdin/stdout falsely timed out");
        // Both directions must exceed the pipe buffer; the last echoed line
        // proves nothing was lost to a deadlock-forced truncation.
        assert!(
            out.stdout.len() > 64 * 1024,
            "echo truncated: {} bytes",
            out.stdout.len()
        );
        assert!(out.stdout.contains(&format!("echo_line_{}", LINES - 1)));
    }

    #[tokio::test]
    async fn run_timeout_preserves_partial_output_after_buffer_fill() {
        // A child that emits a marker, then hangs, must surface the partial
        // output captured before the real timeout fired.
        let opts = if cfg!(windows) {
            SpawnOptions {
                command: "cmd".into(),
                args: vec![
                    "/C".into(),
                    "echo PARTIAL_MARKER && ping -n 30 127.0.0.1 >nul".into(),
                ],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(400)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
                sandbox_policy: None,
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo PARTIAL_MARKER; sleep 30".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(400)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: Some(Duration::from_millis(50)),
                sandbox_policy: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert!(out.timed_out);
        assert!(
            out.stdout.contains("PARTIAL_MARKER"),
            "partial output lost on timeout"
        );
    }
}
