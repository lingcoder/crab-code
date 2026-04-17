//! Skill discovery, loading, and registry for Crab Code.
//!
//! Skills are prompt templates that activate based on user input (slash commands,
//! pattern matches, or manual invocation). They live in `.crab/skills/` directories
//! or are compiled into the binary as bundled skills.
//!
//! # Architecture
//!
//! This crate provides the core skill system:
//!
//! - [`types`] — `Skill`, `SkillTrigger`, `SkillContext`, `SkillSource`
//! - [`frontmatter`] — Parse `.md` files with YAML frontmatter into skills
//! - [`registry`] — `SkillRegistry` for discovery, registration, and lookup
//! - [`matcher`] — Fuzzy matcher for skill names / slash commands (nucleo-matcher)
//! - [`builder`] — Fluent API for constructing skills programmatically
//! - [`bundled`] — Built-in skills shipped with crab-code
//!
//! The skill crate is intentionally decoupled from the hook and MCP systems.
//! Bridges between skill ↔ hook (frontmatter hooks) and skill ↔ MCP live in
//! the `plugin` crate, which depends on both.

pub mod builder;
pub mod bundled;
pub mod frontmatter;
pub mod matcher;
pub mod registry;
pub mod types;

// Re-export primary types at crate root for convenience.
pub use registry::SkillRegistry;
pub use types::{Skill, SkillContext, SkillSource, SkillTrigger};
