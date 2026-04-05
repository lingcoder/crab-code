//! Child process spawning, environment inheritance.

use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Options for spawning a child process.
pub struct SpawnOptions {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    pub stdin_data: Option<String>,
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
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    cmd
}

/// Execute a command and wait for completion.
///
/// If `timeout` is set and the process exceeds it, the process is killed and
/// `SpawnOutput::timed_out` is set to `true`.
///
/// # Errors
///
/// Returns an error if the command cannot be spawned or output cannot be captured.
pub async fn run(opts: SpawnOptions) -> crab_common::Result<SpawnOutput> {
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
            let mut stdout_buf = String::new();
            let mut stderr_buf = String::new();
            if let Some(mut r) = stdout_pipe {
                tokio::io::AsyncReadExt::read_to_string(&mut r, &mut stdout_buf).await?;
            }
            if let Some(mut r) = stderr_pipe {
                tokio::io::AsyncReadExt::read_to_string(&mut r, &mut stderr_buf).await?;
            }
            SpawnOutput {
                stdout: stdout_buf,
                stderr: stderr_buf,
                exit_code: status.code().unwrap_or(-1),
                timed_out: false,
            }
        } else {
            // Timeout — kill and drain remaining output.
            let _ = child.kill().await;
            let _ = child.wait().await;
            let mut stdout_buf = String::new();
            let mut stderr_buf = String::new();
            if let Some(mut r) = stdout_pipe {
                let _ = tokio::io::AsyncReadExt::read_to_string(&mut r, &mut stdout_buf).await;
            }
            if let Some(mut r) = stderr_pipe {
                let _ = tokio::io::AsyncReadExt::read_to_string(&mut r, &mut stderr_buf).await;
            }
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
) -> crab_common::Result<i32> {
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
            }
        } else {
            SpawnOptions {
                command: "echo".into(),
                args: vec![msg.into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
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
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "exit 42".into()],
                working_dir: None,
                env: vec![],
                timeout: None,
                stdin_data: None,
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
            }
        } else {
            SpawnOptions {
                command: "sleep".into(),
                args: vec!["10".into()],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_millis(100)),
                stdin_data: None,
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
            }
        } else {
            SpawnOptions {
                command: "sh".into(),
                args: vec!["-c".into(), "echo $MY_TEST_VAR".into()],
                working_dir: None,
                env: vec![("MY_TEST_VAR".into(), "crab_value".into())],
                timeout: None,
                stdin_data: None,
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
            }
        } else {
            SpawnOptions {
                command: "pwd".into(),
                args: vec![],
                working_dir: Some(tmp.clone()),
                env: vec![],
                timeout: None,
                stdin_data: None,
            }
        };
        let out = run(opts).await.unwrap();
        assert_eq!(out.exit_code, 0);
        // The output should contain the temp dir path
        let expected = crab_common::path::normalize(&tmp)
            .to_string_lossy()
            .to_lowercase();
        assert!(out.stdout.trim().to_lowercase().contains(&expected));
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
            }
        } else {
            SpawnOptions {
                command: "cat".into(),
                args: vec![],
                working_dir: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
                stdin_data: Some("hello from stdin\n".into()),
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
        let lines = collected.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains("streaming_test")));
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
        };
        let result = run(opts).await;
        assert!(result.is_err());
    }
}
