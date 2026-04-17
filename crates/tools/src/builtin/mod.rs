//! Built-in tool implementations that ship with crab.
//!
//! Each sub-module is one tool family (or a closely-related set); the
//! registry-population entry points [`register_all_builtins`] and
//! [`create_default_registry`] live in [`registry`] so this file stays
//! a pure module tree.

pub mod agent;
pub mod ask_user;
pub mod bash;
pub mod bash_classifier;
pub mod bash_security;
pub mod brief;
pub mod config_tool;
pub mod cron;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod image_read;
pub mod lsp;
pub mod mcp_auth;
pub mod mcp_resource;
pub mod mcp_tool;
pub mod monitor;
pub mod notebook;
pub mod plan_approval;
pub mod plan_file;
pub mod plan_mode;
#[cfg(target_os = "windows")]
pub mod powershell;
pub mod read;
pub mod read_enhanced;
pub mod registry;
pub mod remote_trigger;
pub mod send_message;
pub mod send_user_file;
pub mod skill;
pub mod sleep;
pub mod snip;
pub mod structured_output;
pub mod task;
pub mod team;
pub mod todo_write;
pub mod tool_search;
pub mod verify_plan;
pub mod web_browser;
pub mod web_cache;
pub mod web_fetch;
pub mod web_formatter;
pub mod web_search;
pub mod workflow;
pub mod worktree;
pub mod write;

pub use registry::{create_default_registry, register_all_builtins};
