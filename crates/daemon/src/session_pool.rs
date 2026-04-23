//! Session pool — manages multiple concurrent agent sessions.
//!
//! Each session runs in its own tokio task. The pool handles creation,
//! lookup, cleanup of idle sessions, and graceful shutdown.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::protocol::SessionInfo;

/// Maximum number of concurrent sessions (default).
const DEFAULT_MAX_SESSIONS: usize = 8;
/// Default idle timeout before a detached session is reclaimed.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60); // 30 minutes

/// Handle to a running session within the pool.
#[derive(Debug)]
pub struct SessionHandle {
    pub id: String,
    pub working_dir: PathBuf,
    pub created_at: Instant,
    pub last_active: Instant,
    pub attached: bool,
}

impl SessionHandle {
    fn idle_duration(&self) -> Duration {
        self.last_active.elapsed()
    }

    fn to_info(&self, now: Instant) -> SessionInfo {
        let created_secs = now.duration_since(self.created_at).as_secs();
        let idle_secs = self.idle_duration().as_secs();
        SessionInfo {
            id: self.id.clone(),
            working_dir: self.working_dir.clone(),
            attached: self.attached,
            created_at_secs: created_secs,
            idle_secs,
        }
    }
}

/// Pool of active agent sessions.
pub struct SessionPool {
    sessions: HashMap<String, SessionHandle>,
    max_sessions: usize,
    idle_timeout: Duration,
}

impl SessionPool {
    /// Create a new session pool with default limits.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            max_sessions: DEFAULT_MAX_SESSIONS,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// Create a pool with custom limits.
    #[must_use]
    pub fn with_config(max_sessions: usize, idle_timeout: Duration) -> Self {
        Self {
            sessions: HashMap::new(),
            max_sessions,
            idle_timeout,
        }
    }

    /// Create a new session. Returns the session ID, or an error if at capacity.
    pub fn create_session(&mut self, working_dir: PathBuf) -> crab_core::Result<String> {
        if self.sessions.len() >= self.max_sessions {
            return Err(crab_core::Error::Other(format!(
                "session pool full (max {})",
                self.max_sessions
            )));
        }

        let id = crab_core::common::utils::id::new_ulid();
        let now = Instant::now();
        self.sessions.insert(
            id.clone(),
            SessionHandle {
                id: id.clone(),
                working_dir,
                created_at: now,
                last_active: now,
                attached: false,
            },
        );
        Ok(id)
    }

    /// Get a reference to a session handle.
    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<&SessionHandle> {
        self.sessions.get(session_id)
    }

    /// Get a mutable reference to a session handle.
    #[allow(dead_code)]
    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut SessionHandle> {
        self.sessions.get_mut(session_id)
    }

    /// Mark a session as attached (a CLI client is connected).
    pub fn attach(&mut self, session_id: &str) -> bool {
        self.sessions.get_mut(session_id).is_some_and(|h| {
            h.attached = true;
            h.last_active = Instant::now();
            true
        })
    }

    /// Mark a session as detached (CLI client disconnected).
    pub fn detach(&mut self, session_id: &str) -> bool {
        self.sessions.get_mut(session_id).is_some_and(|h| {
            h.attached = false;
            h.last_active = Instant::now();
            true
        })
    }

    /// Touch a session to update its last-active timestamp.
    pub fn touch(&mut self, session_id: &str) {
        if let Some(h) = self.sessions.get_mut(session_id) {
            h.last_active = Instant::now();
        }
    }

    /// Remove a session from the pool.
    pub fn remove(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    /// List all active sessions.
    #[must_use]
    pub fn list(&self) -> Vec<SessionInfo> {
        let now = Instant::now();
        self.sessions.values().map(|h| h.to_info(now)).collect()
    }

    /// Number of active sessions.
    #[must_use]
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Reap idle detached sessions that have exceeded the idle timeout.
    /// Returns the IDs of removed sessions.
    pub fn reap_idle(&mut self) -> Vec<String> {
        let timeout = self.idle_timeout;
        let expired: Vec<String> = self
            .sessions
            .values()
            .filter(|h| !h.attached && h.idle_duration() > timeout)
            .map(|h| h.id.clone())
            .collect();

        for id in &expired {
            self.sessions.remove(id);
        }
        expired
    }

    /// Check if a session exists.
    #[must_use]
    #[allow(dead_code)]
    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }
}

impl Default for SessionPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get_session() {
        let mut pool = SessionPool::new();
        let id = pool.create_session(PathBuf::from("/tmp")).unwrap();
        assert!(pool.contains(&id));
        let handle = pool.get(&id).unwrap();
        assert_eq!(handle.working_dir, PathBuf::from("/tmp"));
        assert!(!handle.attached);
    }

    #[test]
    fn create_session_respects_max() {
        let mut pool = SessionPool::with_config(2, Duration::from_secs(60));
        pool.create_session(PathBuf::from("/a")).unwrap();
        pool.create_session(PathBuf::from("/b")).unwrap();
        let result = pool.create_session(PathBuf::from("/c"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pool full"));
    }

    #[test]
    fn attach_and_detach() {
        let mut pool = SessionPool::new();
        let id = pool.create_session(PathBuf::from("/tmp")).unwrap();

        assert!(!pool.get(&id).unwrap().attached);
        assert!(pool.attach(&id));
        assert!(pool.get(&id).unwrap().attached);
        assert!(pool.detach(&id));
        assert!(!pool.get(&id).unwrap().attached);
    }

    #[test]
    fn attach_nonexistent_returns_false() {
        let mut pool = SessionPool::new();
        assert!(!pool.attach("nonexistent"));
        assert!(!pool.detach("nonexistent"));
    }

    #[test]
    fn remove_session() {
        let mut pool = SessionPool::new();
        let id = pool.create_session(PathBuf::from("/tmp")).unwrap();
        assert_eq!(pool.len(), 1);
        assert!(pool.remove(&id));
        assert_eq!(pool.len(), 0);
        assert!(!pool.contains(&id));
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut pool = SessionPool::new();
        assert!(!pool.remove("nonexistent"));
    }

    #[test]
    fn list_sessions() {
        let mut pool = SessionPool::new();
        pool.create_session(PathBuf::from("/a")).unwrap();
        pool.create_session(PathBuf::from("/b")).unwrap();
        let list = pool.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn is_empty_and_len() {
        let mut pool = SessionPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
        pool.create_session(PathBuf::from("/tmp")).unwrap();
        assert!(!pool.is_empty());
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn touch_updates_last_active() {
        let mut pool = SessionPool::new();
        let id = pool.create_session(PathBuf::from("/tmp")).unwrap();
        let before = pool.get(&id).unwrap().last_active;
        // Small sleep to ensure time difference
        std::thread::sleep(Duration::from_millis(10));
        pool.touch(&id);
        let after = pool.get(&id).unwrap().last_active;
        assert!(after > before);
    }

    #[test]
    fn touch_nonexistent_is_noop() {
        let mut pool = SessionPool::new();
        pool.touch("nonexistent"); // should not panic
    }

    #[test]
    fn reap_idle_removes_expired_detached() {
        let mut pool = SessionPool::with_config(8, Duration::from_millis(1));
        let id1 = pool.create_session(PathBuf::from("/a")).unwrap();
        let id2 = pool.create_session(PathBuf::from("/b")).unwrap();

        // Attach one so it won't be reaped
        pool.attach(&id2);

        // Wait for idle timeout
        std::thread::sleep(Duration::from_millis(10));

        let reaped = pool.reap_idle();
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0], id1);
        assert!(!pool.contains(&id1));
        assert!(pool.contains(&id2)); // attached, not reaped
    }

    #[test]
    fn reap_idle_skips_attached() {
        let mut pool = SessionPool::with_config(8, Duration::from_millis(1));
        let id = pool.create_session(PathBuf::from("/tmp")).unwrap();
        pool.attach(&id);
        std::thread::sleep(Duration::from_millis(10));
        let reaped = pool.reap_idle();
        assert!(reaped.is_empty());
        assert!(pool.contains(&id));
    }

    #[test]
    fn session_info_has_correct_fields() {
        let mut pool = SessionPool::new();
        let id = pool.create_session(PathBuf::from("/project")).unwrap();
        pool.attach(&id);
        let list = pool.list();
        let info = &list[0];
        assert_eq!(info.id, id);
        assert_eq!(info.working_dir, PathBuf::from("/project"));
        assert!(info.attached);
    }

    #[test]
    fn default_pool() {
        let pool = SessionPool::default();
        assert!(pool.is_empty());
    }

    #[test]
    fn unique_session_ids() {
        let mut pool = SessionPool::new();
        let id1 = pool.create_session(PathBuf::from("/a")).unwrap();
        let id2 = pool.create_session(PathBuf::from("/b")).unwrap();
        assert_ne!(id1, id2);
    }
}
