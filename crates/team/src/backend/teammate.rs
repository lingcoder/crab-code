//! Spawn configuration for a teammate.
//!
//! [`TeammateConfig`] carries everything needed to spawn one agent instance,
//! regardless of lifetime. The teammate value type itself lives in
//! [`crate::roster`] because the roster is what owns it once spawned.

use std::collections::HashSet;
use std::path::PathBuf;

use crab_core::permission::PermissionMode;

use crate::roster::{Capability, Lifetime};

/// Configuration for spawning a new teammate.
#[derive(Debug, Clone)]
pub struct TeammateConfig {
    /// Addressable name for the teammate. Ephemeral spawns leave this empty
    /// and the backend fills in the assigned id.
    pub name: String,
    /// Role description (e.g. "`code_reviewer`", "`test_writer`"), usually the
    /// `subagent_type` the spawn requested.
    pub role: String,
    /// System prompt for the teammate's conversation context.
    pub system_prompt: String,
    /// The first message to seed the teammate with.
    pub seed_task: String,
    /// How long this teammate lives and how its run terminates.
    pub lifetime: Lifetime,
    /// Model override, or `None` to inherit the session's model.
    pub model: Option<String>,
    /// Declared capabilities, used for capability-based assignment.
    pub capabilities: HashSet<Capability>,
    /// Context window for the teammate's conversation.
    pub context_window: u64,
    /// Permission mode the parent ran under. The teammate is restricted to at
    /// most this, so a spawned agent is never more privileged than its parent.
    pub parent_permission_mode: Option<PermissionMode>,
    /// Optional working directory override.
    pub working_dir: Option<PathBuf>,
    /// Extra environment variables to inject.
    pub env_vars: Vec<(String, String)>,
}

/// Default context window for a spawned teammate when the caller does not
/// specify one.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

impl TeammateConfig {
    /// Create a config with a name, role, and lifetime.
    #[must_use]
    pub fn new(name: impl Into<String>, role: impl Into<String>, lifetime: Lifetime) -> Self {
        Self {
            name: name.into(),
            role: role.into(),
            system_prompt: String::new(),
            seed_task: String::new(),
            lifetime,
            model: None,
            capabilities: HashSet::new(),
            context_window: DEFAULT_CONTEXT_WINDOW,
            parent_permission_mode: None,
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

    /// Set the first message the teammate receives.
    #[must_use]
    pub fn with_seed_task(mut self, task: impl Into<String>) -> Self {
        self.seed_task = task.into();
        self
    }

    /// Set the model override.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the context window.
    #[must_use]
    pub fn with_context_window(mut self, window: u64) -> Self {
        self.context_window = window;
        self
    }

    /// Restrict the teammate to at most the parent's permission mode.
    #[must_use]
    pub fn with_parent_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.parent_permission_mode = Some(mode);
        self
    }

    /// Declare a capability.
    #[must_use]
    pub fn with_capability(mut self, cap: Capability) -> Self {
        self.capabilities.insert(cap);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let c = TeammateConfig::new("Alice", "reviewer", Lifetime::Resident);
        assert_eq!(c.name, "Alice");
        assert_eq!(c.role, "reviewer");
        assert_eq!(c.lifetime, Lifetime::Resident);
        assert_eq!(c.context_window, DEFAULT_CONTEXT_WINDOW);
        assert!(c.model.is_none());
        assert!(c.parent_permission_mode.is_none());
        assert!(c.capabilities.is_empty());
    }

    #[test]
    fn config_builder_chain() {
        let config = TeammateConfig::new("Alice", "reviewer", Lifetime::ephemeral())
            .with_system_prompt("You review code.")
            .with_seed_task("Review auth.rs")
            .with_model("claude-opus-5")
            .with_context_window(64_000)
            .with_parent_permission_mode(PermissionMode::Plan)
            .with_capability(Capability::new("code_review"))
            .with_working_dir(PathBuf::from("/tmp/project"))
            .with_env("RUST_LOG", "debug");

        assert_eq!(config.system_prompt, "You review code.");
        assert_eq!(config.seed_task, "Review auth.rs");
        assert_eq!(config.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(config.context_window, 64_000);
        assert_eq!(config.parent_permission_mode, Some(PermissionMode::Plan));
        assert!(
            config
                .capabilities
                .contains(&Capability::new("code_review"))
        );
        assert_eq!(
            config.working_dir.as_deref(),
            Some(std::path::Path::new("/tmp/project"))
        );
        assert_eq!(config.env_vars, vec![("RUST_LOG".into(), "debug".into())]);
    }
}
