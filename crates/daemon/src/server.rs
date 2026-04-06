//! Daemon server — IPC listener + request handling.
//!
//! Listens on a Unix socket (Linux/macOS) or named pipe (Windows)
//! and dispatches incoming requests to the session pool.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::protocol::{DaemonRequest, DaemonResponse, encode_message};
use crate::session_pool::SessionPool;

/// Default port for TCP-based IPC fallback.
const DEFAULT_IPC_PORT: u16 = 19836;
/// Interval between idle session reaping sweeps.
const REAP_INTERVAL: Duration = Duration::from_secs(60);

/// Daemon server configuration.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Port to listen on for IPC connections.
    pub port: u16,
    /// Path to PID file for single-instance check.
    pub pid_file: PathBuf,
    /// Log directory for daemon logs.
    pub log_dir: PathBuf,
    /// Maximum concurrent sessions.
    pub max_sessions: usize,
    /// Idle timeout for detached sessions.
    pub idle_timeout: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let data_dir = directories::ProjectDirs::from("", "", "crab-code").map_or_else(
            || {
                crab_common::path::home_dir()
                    .join(".local")
                    .join("share")
                    .join("crab-code")
            },
            |d| d.data_dir().to_path_buf(),
        );
        Self {
            port: DEFAULT_IPC_PORT,
            pid_file: data_dir.join("daemon.pid"),
            log_dir: data_dir.join("logs"),
            max_sessions: 8,
            idle_timeout: Duration::from_secs(30 * 60),
        }
    }
}

/// Status of the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStatus {
    Starting,
    Running,
    ShuttingDown,
    Stopped,
}

impl std::fmt::Display for DaemonStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::ShuttingDown => write!(f, "shutting_down"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// The daemon server.
pub struct DaemonServer {
    config: DaemonConfig,
    pool: Arc<Mutex<SessionPool>>,
    status: Arc<Mutex<DaemonStatus>>,
}

impl DaemonServer {
    /// Create a new daemon server.
    #[must_use]
    pub fn new(config: DaemonConfig) -> Self {
        let pool = SessionPool::with_config(config.max_sessions, config.idle_timeout);
        Self {
            config,
            pool: Arc::new(Mutex::new(pool)),
            status: Arc::new(Mutex::new(DaemonStatus::Starting)),
        }
    }

    /// Get the current daemon status.
    #[allow(dead_code)]
    pub async fn status(&self) -> DaemonStatus {
        *self.status.lock().await
    }

    /// Get the number of active sessions.
    #[allow(dead_code)]
    pub async fn session_count(&self) -> usize {
        self.pool.lock().await.len()
    }

    /// Write the PID file for single-instance checking.
    fn write_pid_file(&self) -> crab_common::Result<()> {
        if let Some(parent) = self.config.pid_file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crab_common::Error::Other(format!("failed to create PID dir: {e}")))?;
        }
        std::fs::write(&self.config.pid_file, std::process::id().to_string())
            .map_err(|e| crab_common::Error::Other(format!("failed to write PID file: {e}")))
    }

    /// Remove the PID file on shutdown.
    fn remove_pid_file(&self) {
        let _ = std::fs::remove_file(&self.config.pid_file);
    }

    /// Check if another daemon instance is already running.
    #[must_use]
    pub fn is_already_running(&self) -> bool {
        check_pid_file(&self.config.pid_file)
    }

    /// Start the daemon server — listens for IPC connections and processes requests.
    pub async fn run(&self) -> crab_common::Result<()> {
        if self.is_already_running() {
            return Err(crab_common::Error::Other(
                "another daemon instance is already running".into(),
            ));
        }

        self.write_pid_file()?;

        let addr = format!("127.0.0.1:{}", self.config.port);
        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            crab_common::Error::Other(format!("failed to bind IPC listener on {addr}: {e}"))
        })?;

        info!("daemon listening on {addr}");
        *self.status.lock().await = DaemonStatus::Running;

        // Spawn idle reaper task
        let pool_for_reaper = Arc::clone(&self.pool);
        let status_for_reaper = Arc::clone(&self.status);
        tokio::spawn(async move {
            reap_loop(pool_for_reaper, status_for_reaper).await;
        });

        // Accept loop
        loop {
            if *self.status.lock().await == DaemonStatus::ShuttingDown {
                break;
            }

            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer)) => {
                            info!("new IPC connection from {peer}");
                            let pool = Arc::clone(&self.pool);
                            let status = Arc::clone(&self.status);
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, pool, status).await {
                                    error!("connection handler error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            error!("accept error: {e}");
                        }
                    }
                }
            }
        }

        *self.status.lock().await = DaemonStatus::Stopped;
        self.remove_pid_file();
        info!("daemon stopped");
        Ok(())
    }

    /// Request graceful shutdown.
    pub async fn shutdown(&self) {
        *self.status.lock().await = DaemonStatus::ShuttingDown;
    }
}

/// Handle a single IPC connection — read requests, dispatch, write responses.
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    pool: Arc<Mutex<SessionPool>>,
    status: Arc<Mutex<DaemonStatus>>,
) -> crab_common::Result<()> {
    let mut buf = vec![0u8; 65536];
    let mut read_buf = Vec::new();

    loop {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| crab_common::Error::Other(format!("IPC read error: {e}")))?;

        if n == 0 {
            // Connection closed
            break;
        }

        read_buf.extend_from_slice(&buf[..n]);

        // Try to decode complete messages
        while let Some((request, consumed)) =
            crate::protocol::decode_message::<DaemonRequest>(&read_buf)?
        {
            read_buf.drain(..consumed);
            let response = dispatch_request(request, &pool, &status).await;
            let encoded = encode_message(&response)?;
            stream
                .write_all(&encoded)
                .await
                .map_err(|e| crab_common::Error::Other(format!("IPC write error: {e}")))?;

            // If shutdown was requested, break the inner loop
            if matches!(response, DaemonResponse::ShuttingDown) {
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Dispatch a single request to the appropriate handler.
async fn dispatch_request(
    request: DaemonRequest,
    pool: &Arc<Mutex<SessionPool>>,
    status: &Arc<Mutex<DaemonStatus>>,
) -> DaemonResponse {
    match request {
        DaemonRequest::Ping => DaemonResponse::Pong,

        DaemonRequest::ListSessions => {
            let pool = pool.lock().await;
            DaemonResponse::Sessions { list: pool.list() }
        }

        DaemonRequest::Attach {
            session_id,
            working_dir,
        } => {
            let mut pool = pool.lock().await;
            let resp = match session_id {
                Some(id) => {
                    if pool.attach(&id) {
                        DaemonResponse::Attached { session_id: id }
                    } else {
                        DaemonResponse::Error {
                            message: format!("session '{id}' not found"),
                        }
                    }
                }
                None => match pool.create_session(working_dir) {
                    Ok(id) => {
                        pool.attach(&id);
                        DaemonResponse::Attached { session_id: id }
                    }
                    Err(e) => DaemonResponse::Error {
                        message: e.to_string(),
                    },
                },
            };
            drop(pool);
            resp
        }

        DaemonRequest::Detach { session_id } => {
            let mut pool = pool.lock().await;
            if pool.detach(&session_id) {
                DaemonResponse::Attached {
                    session_id: session_id.clone(),
                }
            } else {
                DaemonResponse::Error {
                    message: format!("session '{session_id}' not found"),
                }
            }
        }

        DaemonRequest::KillSession { session_id } => {
            let mut pool = pool.lock().await;
            if pool.remove(&session_id) {
                info!("killed session {session_id}");
                DaemonResponse::Sessions { list: pool.list() }
            } else {
                DaemonResponse::Error {
                    message: format!("session '{session_id}' not found"),
                }
            }
        }

        DaemonRequest::UserInput {
            session_id,
            content,
        } => {
            let mut pool = pool.lock().await;
            let resp = if pool.get(&session_id).is_some() {
                pool.touch(&session_id);
                // In a full implementation, this would forward to the agent session.
                // For now, acknowledge receipt.
                DaemonResponse::Event {
                    payload: format!(
                        "{{\"type\":\"ack\",\"session_id\":\"{session_id}\",\"len\":{len}}}",
                        len = content.len()
                    ),
                }
            } else {
                DaemonResponse::Error {
                    message: format!("session '{session_id}' not found"),
                }
            };
            drop(pool);
            resp
        }

        DaemonRequest::Shutdown => {
            info!("shutdown requested via IPC");
            *status.lock().await = DaemonStatus::ShuttingDown;
            DaemonResponse::ShuttingDown
        }
    }
}

/// Background task that periodically reaps idle sessions.
async fn reap_loop(pool: Arc<Mutex<SessionPool>>, status: Arc<Mutex<DaemonStatus>>) {
    loop {
        tokio::time::sleep(REAP_INTERVAL).await;

        if *status.lock().await == DaemonStatus::ShuttingDown {
            break;
        }

        let reaped = pool.lock().await.reap_idle();
        for id in &reaped {
            warn!("reaped idle session {id}");
        }
    }
}

/// Check if a PID file exists and the process is still alive.
fn check_pid_file(pid_file: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(pid_file) else {
        return false;
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return false;
    };
    // Simple liveness check: see if process exists
    // On Unix we'd use kill(pid, 0), on Windows we use a simpler check
    process_is_alive(pid)
}

/// Check if a process with the given PID is alive.
fn process_is_alive(pid: u32) -> bool {
    use sysinfo::{Pid, System};
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
    );
    sys.process(pid).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_config_default() {
        let config = DaemonConfig::default();
        assert_eq!(config.port, DEFAULT_IPC_PORT);
        assert_eq!(config.max_sessions, 8);
        assert!(config.pid_file.to_string_lossy().contains("daemon.pid"));
    }

    #[test]
    fn daemon_status_display() {
        assert_eq!(DaemonStatus::Starting.to_string(), "starting");
        assert_eq!(DaemonStatus::Running.to_string(), "running");
        assert_eq!(DaemonStatus::ShuttingDown.to_string(), "shutting_down");
        assert_eq!(DaemonStatus::Stopped.to_string(), "stopped");
    }

    #[test]
    fn check_pid_file_nonexistent() {
        assert!(!check_pid_file(Path::new("/nonexistent/daemon.pid")));
    }

    #[test]
    fn check_pid_file_invalid_content() {
        let dir = std::env::temp_dir().join("crab-daemon-test-pid");
        let _ = std::fs::create_dir_all(&dir);
        let pid_file = dir.join("daemon.pid");
        std::fs::write(&pid_file, "not-a-number").unwrap();
        assert!(!check_pid_file(&pid_file));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn daemon_server_creation() {
        let config = DaemonConfig::default();
        let server = DaemonServer::new(config);
        assert_eq!(server.status().await, DaemonStatus::Starting);
        assert_eq!(server.session_count().await, 0);
    }

    #[tokio::test]
    async fn daemon_server_shutdown() {
        let config = DaemonConfig::default();
        let server = DaemonServer::new(config);
        server.shutdown().await;
        assert_eq!(server.status().await, DaemonStatus::ShuttingDown);
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(DaemonRequest::Ping, &pool, &status).await;
        assert!(matches!(resp, DaemonResponse::Pong));
    }

    #[tokio::test]
    async fn dispatch_list_sessions_empty() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(DaemonRequest::ListSessions, &pool, &status).await;
        match resp {
            DaemonResponse::Sessions { list } => assert!(list.is_empty()),
            _ => panic!("expected Sessions"),
        }
    }

    #[tokio::test]
    async fn dispatch_attach_new_session() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(
            DaemonRequest::Attach {
                session_id: None,
                working_dir: PathBuf::from("/tmp"),
            },
            &pool,
            &status,
        )
        .await;
        match resp {
            DaemonResponse::Attached { session_id } => {
                assert!(!session_id.is_empty());
                // Session should exist and be attached
                let pool = pool.lock().await;
                assert!(pool.get(&session_id).unwrap().attached);
            }
            _ => panic!("expected Attached"),
        }
    }

    #[tokio::test]
    async fn dispatch_attach_existing_session() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));

        // Create a session first
        let id = pool
            .lock()
            .await
            .create_session(PathBuf::from("/tmp"))
            .unwrap();

        let resp = dispatch_request(
            DaemonRequest::Attach {
                session_id: Some(id.clone()),
                working_dir: PathBuf::from("/tmp"),
            },
            &pool,
            &status,
        )
        .await;
        match resp {
            DaemonResponse::Attached { session_id } => assert_eq!(session_id, id),
            _ => panic!("expected Attached"),
        }
    }

    #[tokio::test]
    async fn dispatch_attach_nonexistent() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(
            DaemonRequest::Attach {
                session_id: Some("nonexistent".into()),
                working_dir: PathBuf::from("/tmp"),
            },
            &pool,
            &status,
        )
        .await;
        assert!(matches!(resp, DaemonResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_detach() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let id = pool
            .lock()
            .await
            .create_session(PathBuf::from("/tmp"))
            .unwrap();
        pool.lock().await.attach(&id);

        let resp = dispatch_request(
            DaemonRequest::Detach {
                session_id: id.clone(),
            },
            &pool,
            &status,
        )
        .await;
        assert!(matches!(resp, DaemonResponse::Attached { .. }));
        assert!(!pool.lock().await.get(&id).unwrap().attached);
    }

    #[tokio::test]
    async fn dispatch_kill_session() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let id = pool
            .lock()
            .await
            .create_session(PathBuf::from("/tmp"))
            .unwrap();

        let resp = dispatch_request(
            DaemonRequest::KillSession {
                session_id: id.clone(),
            },
            &pool,
            &status,
        )
        .await;
        match resp {
            DaemonResponse::Sessions { list } => assert!(list.is_empty()),
            _ => panic!("expected Sessions"),
        }
        assert!(!pool.lock().await.contains(&id));
    }

    #[tokio::test]
    async fn dispatch_user_input() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let id = pool
            .lock()
            .await
            .create_session(PathBuf::from("/tmp"))
            .unwrap();

        let resp = dispatch_request(
            DaemonRequest::UserInput {
                session_id: id,
                content: "hello".into(),
            },
            &pool,
            &status,
        )
        .await;
        assert!(matches!(resp, DaemonResponse::Event { .. }));
    }

    #[tokio::test]
    async fn dispatch_shutdown() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(DaemonRequest::Shutdown, &pool, &status).await;
        assert!(matches!(resp, DaemonResponse::ShuttingDown));
        assert_eq!(*status.lock().await, DaemonStatus::ShuttingDown);
    }

    #[tokio::test]
    async fn dispatch_kill_nonexistent() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(
            DaemonRequest::KillSession {
                session_id: "nope".into(),
            },
            &pool,
            &status,
        )
        .await;
        assert!(matches!(resp, DaemonResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_user_input_nonexistent() {
        let pool = Arc::new(Mutex::new(SessionPool::new()));
        let status = Arc::new(Mutex::new(DaemonStatus::Running));
        let resp = dispatch_request(
            DaemonRequest::UserInput {
                session_id: "nope".into(),
                content: "hello".into(),
            },
            &pool,
            &status,
        )
        .await;
        assert!(matches!(resp, DaemonResponse::Error { .. }));
    }

    #[test]
    fn process_is_alive_detects_current_process() {
        // The current process should be alive
        assert!(process_is_alive(std::process::id()));
    }

    #[test]
    fn process_is_alive_returns_false_for_nonexistent() {
        // A very high PID should not exist
        assert!(!process_is_alive(u32::MAX - 1));
    }
}
