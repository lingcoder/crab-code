//! Slash command framework and built-in commands.
//!
//! Provides a registry of `/command` handlers that can be executed
//! from the REPL or TUI. Commands receive a context struct with
//! references to session state and return a result indicating what
//! action (if any) the caller should take.
//!
//! - [`types`] — [`SlashCommandContext`] / [`SlashCommandResult`] /
//!   [`SlashAction`] / [`SlashCommandRegistry`]
//! - [`handlers`] — each built-in `cmd_*` handler + tests

pub mod handlers;
pub mod types;

pub use types::{SlashAction, SlashCommandContext, SlashCommandRegistry, SlashCommandResult};
