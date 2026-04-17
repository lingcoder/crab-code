//! Sub-agent teammate lifecycle management.
//!
//! A [`Teammate`] represents a spawned sub-agent with an identity, role, and
//! lifecycle state. [`TeammateConfig`] carries the parameters needed to spawn
//! one, regardless of the backend (in-process or tmux).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Lifecycle state of a teammate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum TeammateState {
    /// Created but not yet executing work.
    Idle,
    /// Actively processing a task.
    Running,
    /// Terminated (gracefully or forcibly).
    Stopped,
}

impl std::fmt::Display for TeammateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Configuration for spawning a new teammate.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TeammateConfig {
    /// Human-readable name for the teammate.
    pub name: String,
    /// Role description (e.g. "`code_reviewer`", "`test_writer`").
    pub role: String,
    /// System prompt for the teammate's conversation context.
    pub system_prompt: String,
    /// Optional working directory override.
    pub working_dir: Option<PathBuf>,
    /// Extra environment variables to inject.
    pub env_vars: Vec<(String, String)>,
}

impl TeammateConfig {
    /// Create a minimal config with a name and role.
    #[must_use]
    pub fn new(name: impl Into<String>, role: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            role: role.into(),
            system_prompt: String::new(),
            working_dir: None,
            env_vars: Vec::new(),
        }
    }

    /// Set the system prompt.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set the working directory.
    #[must_use]
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = Some(dir);
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }
}

/// A spawned sub-agent teammate.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Teammate {
    /// Unique identifier (generated at spawn time).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Role description.
    pub role: String,
    /// Current lifecycle state.
    pub state: TeammateState,
    /// Tmux pane ID, if running in tmux mode.
    pub pane_id: Option<String>,
    /// When this teammate was created.
    created_at: Instant,
}

#[allow(dead_code)]
impl Teammate {
    /// Create a new teammate in the [`TeammateState::Idle`] state.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, role: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            role: role.into(),
            state: TeammateState::Idle,
            pane_id: None,
            created_at: Instant::now(),
        }
    }

    /// Whether this teammate is actively running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state == TeammateState::Running
    }

    /// Transition to a new state.
    pub fn set_state(&mut self, state: TeammateState) {
        self.state = state;
    }

    /// Wall-clock time since creation.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.created_at.elapsed()
    }
}

impl std::fmt::Display for Teammate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({}, {})", self.name, self.id, self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teammate_new_defaults() {
        let t = Teammate::new("t-1", "Alice", "code_reviewer");
        assert_eq!(t.id, "t-1");
        assert_eq!(t.name, "Alice");
        assert_eq!(t.role, "code_reviewer");
        assert_eq!(t.state, TeammateState::Idle);
        assert!(t.pane_id.is_none());
    }

    #[test]
    fn teammate_state_transitions() {
        let mut t = Teammate::new("t-1", "Alice", "reviewer");
        assert_eq!(t.state, TeammateState::Idle);

        t.set_state(TeammateState::Running);
        assert_eq!(t.state, TeammateState::Running);

        t.set_state(TeammateState::Stopped);
        assert_eq!(t.state, TeammateState::Stopped);
    }

    #[test]
    fn teammate_is_running() {
        let mut t = Teammate::new("t-1", "Bob", "tester");
        assert!(!t.is_running());

        t.set_state(TeammateState::Running);
        assert!(t.is_running());

        t.set_state(TeammateState::Stopped);
        assert!(!t.is_running());
    }

    #[test]
    fn teammate_display() {
        let t = Teammate::new("t-1", "Alice", "reviewer");
        let s = format!("{t}");
        assert!(s.contains("Alice"));
        assert!(s.contains("t-1"));
        assert!(s.contains("idle"));
    }

    #[test]
    fn teammate_state_display() {
        assert_eq!(TeammateState::Idle.to_string(), "idle");
        assert_eq!(TeammateState::Running.to_string(), "running");
        assert_eq!(TeammateState::Stopped.to_string(), "stopped");
    }

    #[test]
    fn teammate_config_builder() {
        let config = TeammateConfig::new("Alice", "reviewer")
            .with_system_prompt("You review code.")
            .with_working_dir(PathBuf::from("/tmp/project"))
            .with_env("RUST_LOG", "debug");

        assert_eq!(config.name, "Alice");
        assert_eq!(config.role, "reviewer");
        assert_eq!(config.system_prompt, "You review code.");
        assert_eq!(
            config.working_dir.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        assert_eq!(config.env_vars.len(), 1);
        assert_eq!(config.env_vars[0], ("RUST_LOG".into(), "debug".into()));
    }

    #[test]
    fn teammate_elapsed_is_nonnegative() {
        let t = Teammate::new("t-1", "Alice", "reviewer");
        // elapsed is monotonic — should always be >= 0
        assert!(t.elapsed().as_nanos() < 1_000_000_000);
    }

    #[test]
    fn teammate_state_serde_roundtrip() {
        let states = [
            TeammateState::Idle,
            TeammateState::Running,
            TeammateState::Stopped,
        ];
        for state in &states {
            let json = serde_json::to_string(state).unwrap();
            let parsed: TeammateState = serde_json::from_str(&json).unwrap();
            assert_eq!(*state, parsed);
        }
    }
}
