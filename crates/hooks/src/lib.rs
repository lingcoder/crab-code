//! Lifecycle hook system for Crab Code.
//!
//! Hooks are user-defined or built-in interceptors that fire at specific
//! lifecycle points (pre/post tool use, session start/end, prompt submit, etc.).
//! They can allow, deny, modify, or retry operations.

pub mod builtin;
pub mod executor;
pub mod frontmatter;
pub mod registry;
pub mod types;
pub mod watcher;

pub use crab_core::hook::HookTrigger;
pub use executor::{
    DEFAULT_HOOK_TIMEOUT_SECS, HookAction, HookContext, HookDef, HookExecutor, HookResult,
};
pub use registry::{HookEvent, HookEventType, HookRegistry, HookSource, RegisteredHook};
pub use types::{
    AgentHook, CommandHook, HookType, HttpHook, PromptHook, SsrfError, validate_http_hook_url,
};
pub use watcher::HookFileWatcher;
