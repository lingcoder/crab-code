//! Layer 2b — Coordinator Mode.
//!
//! Star-topology overlay on top of Layer 1 [`crate::teams`]: a designated
//! Coordinator agent is stripped of hands-on tools (only `Agent` /
//! `SendMessage` / `TaskStop`), Workers run with an allow-list, and the
//! Coordinator gets an anti-pattern prompt overlay ("understand before
//! delegating").
//!
//! This module is opt-in via `CRAB_COORDINATOR_MODE=1` (see
//! `SessionConfig::coordinator_mode`). The Layer 1 pool ([`crate::teams::WorkerPool`])
//! runs unconditional base infrastructure; Coordinator Mode is additive.
//!
//! Wiring: [`crate::session::AgentSession::new`] calls [`Coordinator::from_config`];
//! if it returns `Some(c)`, `c.apply(&mut registry, &mut system_prompt)` is
//! invoked before the session is handed out.
//!
//! See `docs/architecture.md` § Multi-Agent Three-Layer Architecture.

pub mod gating;
pub mod permission_sync;
pub mod prompt;
pub mod tool_acl;

pub use permission_sync::{PermissionDecisionEvent, PermissionSyncManager};

use crab_tools::registry::ToolRegistry;

use crate::session::SessionConfig;

/// A Coordinator Mode activation. Holding one of these is proof that gating
/// passed; callers can then [`Coordinator::apply`] the registry filter and
/// prompt overlay to a session.
#[derive(Debug, Clone, Copy)]
pub struct Coordinator {
    // Reserved for future per-Coordinator config (e.g. override allow-lists,
    // custom prompt fragments). Kept as a unit-like struct for now so the
    // public API stabilises early.
    _priv: (),
}

impl Coordinator {
    /// Activate Coordinator Mode if gating allows. Returns `None` when the
    /// session is a plain (non-coordinator) session.
    #[must_use]
    pub fn from_config(config: &SessionConfig) -> Option<Self> {
        gating::is_active(config).then_some(Self { _priv: () })
    }

    /// Activate Coordinator Mode directly from the `coordinator_mode` bool,
    /// without needing a [`SessionConfig`] reference.
    ///
    /// Exists for call sites that already have the flag extracted (e.g.
    /// [`crate::session::AgentSession::new`] snapshots it before partial
    /// moves of the config).
    #[must_use]
    pub fn from_flag(coordinator_mode: bool) -> Option<Self> {
        coordinator_mode.then_some(Self { _priv: () })
    }

    /// Reduce a Coordinator's tool registry to the allow-list and append the
    /// anti-pattern prompt overlay to `system_prompt`.
    ///
    /// Idempotent on the registry side (retain is set-based); the prompt
    /// overlay appends unconditionally, so callers should not call `apply`
    /// twice on the same prompt.
    pub fn apply(&self, registry: &mut ToolRegistry, system_prompt: &mut String) {
        registry.retain_names(tool_acl::COORDINATOR_TOOLS);
        prompt::append_to(system_prompt);
    }

    /// The tool allow-list used by this coordinator. Exposed so workers
    /// and tests can verify consistency without importing the `tool_acl`
    /// module directly.
    #[must_use]
    pub const fn allowed_tools(&self) -> &'static [&'static str] {
        tool_acl::COORDINATOR_TOOLS
    }

    /// The tools a worker spawned by this coordinator must not use.
    #[must_use]
    pub const fn worker_denied_tools(&self) -> &'static [&'static str] {
        tool_acl::WORKER_DENIED_TOOLS
    }

    /// Build a fresh worker tool registry: the default crab registry minus
    /// [`tool_acl::WORKER_DENIED_TOOLS`]. Used by
    /// [`crate::session::AgentSession::handle_spawn_request`] to give each
    /// worker a clean toolset — workers inherit neither the coordinator's
    /// stripped 3-tool registry nor its forbidden team-management tools.
    #[must_use]
    pub fn build_worker_registry(&self) -> ToolRegistry {
        let mut registry = crab_tools::builtin::create_default_registry();
        registry.remove_names(tool_acl::WORKER_DENIED_TOOLS);
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::model::ModelId;
    use crab_core::permission::PermissionPolicy;
    use crab_tools::builtin::create_default_registry;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn config_with(coordinator_mode: bool) -> SessionConfig {
        SessionConfig {
            session_id: "t".into(),
            system_prompt: String::new(),
            model: ModelId::from("test-model"),
            max_tokens: 0,
            temperature: None,
            context_window: 0,
            working_dir: PathBuf::from("/tmp"),
            permission_policy: PermissionPolicy::default(),
            memory_dir: None,
            sessions_dir: None,
            resume_session_id: None,
            effort: None,
            thinking_mode: None,
            additional_dirs: Vec::new(),
            session_name: None,
            max_turns: None,
            max_budget_usd: None,
            fallback_model: None,
            bare_mode: false,
            worktree_name: None,
            fork_session: false,
            from_pr: None,
            custom_session_id: None,
            json_schema: None,
            plugin_dirs: Vec::new(),
            disable_skills: false,
            beta_headers: Vec::new(),
            ide_connect: false,
            coordinator_mode,
            default_shell: "bash".into(),
        }
    }

    #[test]
    fn from_config_returns_none_when_gate_closed() {
        assert!(Coordinator::from_config(&config_with(false)).is_none());
    }

    #[test]
    fn from_config_activates_when_gate_open() {
        assert!(Coordinator::from_config(&config_with(true)).is_some());
    }

    #[test]
    fn apply_shrinks_registry_to_allow_list() {
        let mut registry = create_default_registry();
        let original_len = registry.len();
        assert!(original_len > 3, "default registry must have many tools");

        let coord = Coordinator::from_config(&config_with(true)).unwrap();
        let mut prompt = String::from("Base.");
        coord.apply(&mut registry, &mut prompt);

        assert_eq!(registry.len(), 3);
        assert!(registry.get("Agent").is_some());
        assert!(registry.get("SendMessage").is_some());
        assert!(registry.get("TaskStop").is_some());
        // A random non-coordinator tool should be gone.
        assert!(registry.get("Bash").is_none(), "Bash must be stripped");
        assert!(registry.get("Edit").is_none(), "Edit must be stripped");
    }

    #[test]
    fn apply_appends_prompt_overlay() {
        let mut registry = create_default_registry();
        let mut prompt = String::from("Base prompt.");
        let coord = Coordinator::from_config(&config_with(true)).unwrap();
        coord.apply(&mut registry, &mut prompt);

        assert!(prompt.contains("Base prompt."));
        assert!(prompt.contains("Coordinator Mode"));
        assert!(prompt.contains("Based on your findings"));
    }

    #[test]
    fn allowed_tools_matches_acl_constant() {
        let coord = Coordinator::from_config(&config_with(true)).unwrap();
        assert_eq!(coord.allowed_tools(), tool_acl::COORDINATOR_TOOLS);
        assert_eq!(coord.worker_denied_tools(), tool_acl::WORKER_DENIED_TOOLS);
    }

    #[test]
    fn worker_registry_strips_denied_but_keeps_hands_on_tools() {
        let coord = Coordinator::from_config(&config_with(true)).unwrap();
        let worker_reg = coord.build_worker_registry();

        // Denied tools must be gone.
        for denied in tool_acl::WORKER_DENIED_TOOLS {
            assert!(
                worker_reg.get(denied).is_none(),
                "worker registry must not contain {denied}"
            );
        }
        // Hands-on tools (the whole point workers exist) must remain.
        assert!(worker_reg.get("Bash").is_some(), "worker needs Bash");
        assert!(worker_reg.get("Edit").is_some(), "worker needs Edit");
        assert!(worker_reg.get("Read").is_some(), "worker needs Read");

        // Worker can still nest an Agent call (unlike TeamCreate — CCB
        // INTERNAL_WORKER_TOOLS blocks team management, not delegation).
        assert!(worker_reg.get("Agent").is_some());
    }

    #[test]
    fn worker_registry_is_fresh_not_inherited() {
        // Build two worker registries — they must be independent instances
        // (the filter is applied on a freshly-constructed registry each time).
        let coord = Coordinator::from_config(&config_with(true)).unwrap();
        let a = coord.build_worker_registry();
        let b = coord.build_worker_registry();
        assert_eq!(a.len(), b.len());
    }

    // Hold Arc of a shared type to verify Coordinator is Copy/Clone-safe
    // when stored behind references.
    #[test]
    fn coordinator_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Coordinator>();

        let coord = Coordinator::from_config(&config_with(true)).unwrap();
        let arc = Arc::new(coord);
        assert!(arc.allowed_tools().contains(&"Agent"));
    }
}
