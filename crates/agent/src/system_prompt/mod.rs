//! System prompt construction.
//!
//! The system prompt is assembled by [`builder`] from:
//! - environment info (OS, shell, cwd, date)
//! - git status (via [`git_context`])
//! - available tool descriptions
//! - AGENTS.md project instructions
//! - memory files
//! - optional PR context (via [`pr_context`])
//! - contextual tips (via [`tips`])
//! - custom user instructions
//!
//! Phase 4.2 consolidated `git_context`, `pr_context`, and `tips` here from
//! top-level `crates/agent/src/` since they only serve prompt construction.
//! The former `sections` / `cache` modules (an unused alt-architecture) were
//! dropped in the same phase.

pub mod builder;
pub mod git_context;
pub mod pr_context;
pub mod tips;

pub use builder::{build_system_prompt, build_system_prompt_with_memories};
pub use git_context::GitContext;
pub use pr_context::{ChangedFile, PrContext, PrInfo};
pub use tips::{Tip, TipRegistry};
