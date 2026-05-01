//! Async hook registry with event broadcasting.
//!
//! Provides a centralized registry for lifecycle hooks that can be registered
//! from multiple sources (settings, plugins, frontmatter, builtin, session).
//! Events are broadcast to all subscribers via `tokio::sync::broadcast`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{RwLock, broadcast};

use crate::types::HookType;

// ─── Events ─────────────────────────────────────────────────────────────

/// An event that can trigger registered hooks.
///
/// Events are broadcast to all subscribers and used to match against
/// registered hooks' event filters.
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// The session has started.
    SessionStart,
    /// The session is ending.
    SessionEnd,
    /// A tool is about to be executed.
    PreToolUse {
        /// Name of the tool.
        tool_name: String,
        /// JSON input being passed to the tool.
        input: serde_json::Value,
    },
    /// A tool has finished executing.
    PostToolUse {
        /// Name of the tool.
        tool_name: String,
        /// JSON output from the tool.
        output: serde_json::Value,
    },
    /// The user has submitted a prompt.
    UserPromptSubmit {
        /// The user's prompt text.
        prompt: String,
    },
    /// The agent loop has stopped.
    Stop,
    /// A file in the workspace has changed.
    FileChanged {
        /// Path to the changed file.
        path: PathBuf,
    },
    /// A notification to display to the user.
    Notification {
        /// Notification message.
        message: String,
    },
}

/// Event type discriminant for filtering (without payload data).
///
/// Used in [`RegisteredHook::event_filter`] to specify which events
/// a hook should respond to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEventType {
    SessionStart,
    SessionEnd,
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    Stop,
    FileChanged,
    Notification,
}

impl HookEvent {
    /// Get the event type discriminant for this event.
    #[must_use]
    pub fn event_type(&self) -> HookEventType {
        match self {
            Self::SessionStart => HookEventType::SessionStart,
            Self::SessionEnd => HookEventType::SessionEnd,
            Self::PreToolUse { .. } => HookEventType::PreToolUse,
            Self::PostToolUse { .. } => HookEventType::PostToolUse,
            Self::UserPromptSubmit { .. } => HookEventType::UserPromptSubmit,
            Self::Stop => HookEventType::Stop,
            Self::FileChanged { .. } => HookEventType::FileChanged,
            Self::Notification { .. } => HookEventType::Notification,
        }
    }
}

// ─── Registered hook ────────────────────────────────────────────────────

/// Where a hook was registered from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSource {
    /// Registered from global or project settings.
    Settings,
    /// Registered by a named plugin.
    Plugin(String),
    /// Registered from skill/prompt frontmatter.
    Frontmatter,
    /// Built-in hooks shipped with the crate.
    Builtin,
    /// Registered dynamically during the session.
    Session,
}

impl std::fmt::Display for HookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings => f.write_str("settings"),
            Self::Plugin(name) => write!(f, "plugin:{name}"),
            Self::Frontmatter => f.write_str("frontmatter"),
            Self::Builtin => f.write_str("builtin"),
            Self::Session => f.write_str("session"),
        }
    }
}

/// A hook registered in the registry with its metadata.
#[derive(Debug, Clone)]
pub struct RegisteredHook {
    /// Unique identifier for this hook registration.
    pub id: String,
    /// Which events this hook responds to (empty = all events).
    pub event_filter: Vec<HookEventType>,
    /// The hook implementation to execute.
    pub hook_type: HookType,
    /// Where this hook was registered from.
    pub source: HookSource,
}

impl RegisteredHook {
    /// Check if this hook matches the given event type.
    ///
    /// An empty `event_filter` matches all events.
    #[must_use]
    pub fn matches_event(&self, event_type: HookEventType) -> bool {
        self.event_filter.is_empty() || self.event_filter.contains(&event_type)
    }
}

// ─── Registry ───────────────────────────────────────────────────────────

/// Default broadcast channel capacity.
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Auto-incrementing counter for generating unique hook IDs.
static HOOK_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique hook ID.
fn next_hook_id() -> String {
    let id = HOOK_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("hook_{id}")
}

/// Async hook registry with event broadcasting.
///
/// Thread-safe via `RwLock` for the hook list and `broadcast::Sender`
/// for event distribution.
///
/// # Usage
///
/// ```ignore
/// let registry = HookRegistry::new();
///
/// // Register a hook
/// let hook = RegisteredHook {
///     id: String::new(), // will be assigned
///     event_filter: vec![HookEventType::PreToolUse],
///     hook_type: HookType::Command(CommandHook { command: "echo pre".into(), timeout_secs: 10 }),
///     source: HookSource::Settings,
/// };
/// let id = registry.register(hook).await;
///
/// // Subscribe to events
/// let mut rx = registry.subscribe();
///
/// // Emit an event
/// registry.emit(HookEvent::PreToolUse {
///     tool_name: "bash".into(),
///     input: serde_json::json!({}),
/// }).await;
/// ```
pub struct HookRegistry {
    /// Registered hooks, protected by an async read-write lock.
    hooks: RwLock<Vec<RegisteredHook>>,
    /// Broadcast sender for event distribution.
    event_tx: broadcast::Sender<HookEvent>,
}

impl HookRegistry {
    /// Create a new empty hook registry.
    #[must_use]
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(DEFAULT_CHANNEL_CAPACITY);
        Self {
            hooks: RwLock::new(Vec::new()),
            event_tx,
        }
    }

    /// Create a registry with a custom broadcast channel capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let (event_tx, _) = broadcast::channel(capacity);
        Self {
            hooks: RwLock::new(Vec::new()),
            event_tx,
        }
    }

    /// Register a hook and return its assigned ID.
    ///
    /// If the hook's `id` field is empty, a unique ID is generated.
    /// If the `id` already exists, the existing hook is replaced.
    pub async fn register(&self, mut hook: RegisteredHook) -> String {
        if hook.id.is_empty() {
            hook.id = next_hook_id();
        }

        let id = hook.id.clone();
        let mut hooks = self.hooks.write().await;

        // Replace existing hook with the same ID.
        if let Some(existing) = hooks.iter_mut().find(|h| h.id == id) {
            *existing = hook;
        } else {
            hooks.push(hook);
        }

        id
    }

    /// Unregister a hook by ID. Returns `true` if the hook was found and removed.
    pub async fn unregister(&self, id: &str) -> bool {
        let mut hooks = self.hooks.write().await;
        let before = hooks.len();
        hooks.retain(|h| h.id != id);
        hooks.len() < before
    }

    /// Get all hooks that match the given event.
    pub async fn get_hooks_for_event(&self, event: &HookEvent) -> Vec<RegisteredHook> {
        let event_type = event.event_type();
        let hooks = self.hooks.read().await;
        hooks
            .iter()
            .filter(|h| h.matches_event(event_type))
            .cloned()
            .collect()
    }

    /// Emit an event to all subscribers and return matching hooks.
    ///
    /// This broadcasts the event on the channel (for passive subscribers)
    /// and returns the list of hooks that match the event (for the caller
    /// to execute).
    ///
    /// Broadcast errors (no active receivers) are silently ignored.
    pub async fn emit(&self, event: HookEvent) -> Vec<RegisteredHook> {
        // Broadcast to subscribers (ignore error if no receivers).
        let _ = self.event_tx.send(event.clone());

        // Return matching hooks for the caller to execute.
        self.get_hooks_for_event(&event).await
    }

    /// Subscribe to the event broadcast channel.
    ///
    /// Returns a `broadcast::Receiver` that receives cloned events.
    /// Lagging receivers will miss events (the channel is bounded).
    pub fn subscribe(&self) -> broadcast::Receiver<HookEvent> {
        self.event_tx.subscribe()
    }

    /// Number of registered hooks.
    pub async fn len(&self) -> usize {
        self.hooks.read().await.len()
    }

    /// Whether the registry has no hooks.
    pub async fn is_empty(&self) -> bool {
        self.hooks.read().await.is_empty()
    }

    /// Remove all hooks registered from a specific source.
    pub async fn remove_by_source(&self, source: &HookSource) -> usize {
        let mut hooks = self.hooks.write().await;
        let before = hooks.len();
        hooks.retain(|h| &h.source != source);
        before - hooks.len()
    }

    /// List all registered hook IDs.
    pub async fn hook_ids(&self) -> Vec<String> {
        let hooks = self.hooks.read().await;
        hooks.iter().map(|h| h.id.clone()).collect()
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CommandHook;

    fn make_hook(event_filter: Vec<HookEventType>, source: HookSource) -> RegisteredHook {
        RegisteredHook {
            id: String::new(),
            event_filter,
            hook_type: HookType::Command(CommandHook {
                command: "echo test".into(),
                timeout_secs: 10,
            }),
            source,
        }
    }

    #[tokio::test]
    async fn new_registry_is_empty() {
        let reg = HookRegistry::new();
        assert!(reg.is_empty().await);
        assert_eq!(reg.len().await, 0);
    }

    #[tokio::test]
    async fn register_and_count() {
        let reg = HookRegistry::new();
        let hook = make_hook(vec![HookEventType::PreToolUse], HookSource::Settings);
        let id = reg.register(hook).await;
        assert!(!id.is_empty());
        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn register_replaces_same_id() {
        let reg = HookRegistry::new();
        let mut hook1 = make_hook(vec![HookEventType::PreToolUse], HookSource::Settings);
        hook1.id = "my-hook".into();
        reg.register(hook1).await;

        let mut hook2 = make_hook(vec![HookEventType::PostToolUse], HookSource::Session);
        hook2.id = "my-hook".into();
        reg.register(hook2).await;

        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn unregister_removes_hook() {
        let reg = HookRegistry::new();
        let hook = make_hook(vec![], HookSource::Settings);
        let id = reg.register(hook).await;
        assert!(reg.unregister(&id).await);
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn unregister_nonexistent_returns_false() {
        let reg = HookRegistry::new();
        assert!(!reg.unregister("nonexistent").await);
    }

    #[tokio::test]
    async fn get_hooks_for_event_filters() {
        let reg = HookRegistry::new();
        reg.register(make_hook(
            vec![HookEventType::PreToolUse],
            HookSource::Settings,
        ))
        .await;
        reg.register(make_hook(
            vec![HookEventType::PostToolUse],
            HookSource::Settings,
        ))
        .await;
        reg.register(make_hook(vec![], HookSource::Session)) // matches all
            .await;

        let event = HookEvent::PreToolUse {
            tool_name: "bash".into(),
            input: serde_json::json!({}),
        };
        let matched = reg.get_hooks_for_event(&event).await;
        assert_eq!(matched.len(), 2); // PreToolUse + catch-all
    }

    #[tokio::test]
    async fn emit_returns_matching_hooks() {
        let reg = HookRegistry::new();
        reg.register(make_hook(
            vec![HookEventType::SessionStart],
            HookSource::Settings,
        ))
        .await;

        let matched = reg.emit(HookEvent::SessionStart).await;
        assert_eq!(matched.len(), 1);

        let matched = reg.emit(HookEvent::SessionEnd).await;
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn subscribe_receives_events() {
        let reg = HookRegistry::new();
        let mut rx = reg.subscribe();

        reg.emit(HookEvent::Notification {
            message: "hello".into(),
        })
        .await;

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, HookEvent::Notification { message } if message == "hello"));
    }

    #[tokio::test]
    async fn remove_by_source() {
        let reg = HookRegistry::new();
        reg.register(make_hook(vec![], HookSource::Settings)).await;
        reg.register(make_hook(vec![], HookSource::Session)).await;
        reg.register(make_hook(vec![], HookSource::Settings)).await;

        let removed = reg.remove_by_source(&HookSource::Settings).await;
        assert_eq!(removed, 2);
        assert_eq!(reg.len().await, 1);
    }

    #[tokio::test]
    async fn hook_ids_returns_all() {
        let reg = HookRegistry::new();
        let id1 = reg.register(make_hook(vec![], HookSource::Settings)).await;
        let id2 = reg.register(make_hook(vec![], HookSource::Session)).await;

        let ids = reg.hook_ids().await;
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn hook_event_type_discriminant() {
        let event = HookEvent::PreToolUse {
            tool_name: "bash".into(),
            input: serde_json::json!({}),
        };
        assert_eq!(event.event_type(), HookEventType::PreToolUse);

        assert_eq!(
            HookEvent::SessionStart.event_type(),
            HookEventType::SessionStart
        );
        assert_eq!(HookEvent::Stop.event_type(), HookEventType::Stop);
    }

    #[test]
    fn registered_hook_matches_event() {
        let hook = RegisteredHook {
            id: "test".into(),
            event_filter: vec![HookEventType::PreToolUse, HookEventType::PostToolUse],
            hook_type: HookType::Command(CommandHook {
                command: "echo".into(),
                timeout_secs: 10,
            }),
            source: HookSource::Settings,
        };

        assert!(hook.matches_event(HookEventType::PreToolUse));
        assert!(hook.matches_event(HookEventType::PostToolUse));
        assert!(!hook.matches_event(HookEventType::SessionStart));
    }

    #[test]
    fn registered_hook_empty_filter_matches_all() {
        let hook = RegisteredHook {
            id: "test".into(),
            event_filter: vec![],
            hook_type: HookType::Command(CommandHook {
                command: "echo".into(),
                timeout_secs: 10,
            }),
            source: HookSource::Session,
        };

        assert!(hook.matches_event(HookEventType::PreToolUse));
        assert!(hook.matches_event(HookEventType::SessionStart));
        assert!(hook.matches_event(HookEventType::Notification));
    }

    #[test]
    fn hook_source_display() {
        assert_eq!(HookSource::Settings.to_string(), "settings");
        assert_eq!(
            HookSource::Plugin("my-plugin".into()).to_string(),
            "plugin:my-plugin"
        );
        assert_eq!(HookSource::Frontmatter.to_string(), "frontmatter");
        assert_eq!(HookSource::Session.to_string(), "session");
    }

    #[test]
    fn default_registry() {
        // Ensure Default impl works (non-async construction).
        let _reg = HookRegistry::default();
    }
}
