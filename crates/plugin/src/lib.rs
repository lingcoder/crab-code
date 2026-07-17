//! Claude Code format plugin loading.
//!
//! Loads CC-layout plugin packages (`plugin.json` + `commands/` +
//! `agents/` + `skills/` + `hooks/hooks.json` + `.mcp.json`) and exposes
//! their components for the composition root to register.

pub mod loader;
pub mod manifest;

pub use loader::{LoadedPlugin, discover_plugins, load_plugin};
pub use manifest::{PluginAuthor, PluginManifest, load_manifest};
