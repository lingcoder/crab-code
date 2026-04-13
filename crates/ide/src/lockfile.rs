//! Discovery of IDE plugin MCP endpoints via lockfile.
//!
//! IDE plugins write `<ide-dir>/<port>.lock` on startup with a JSON
//! payload describing how to reach their MCP server:
//!
//! ```json
//! {
//!   "pid": 12345,
//!   "workspaceFolders": ["/home/user/project"],
//!   "ideName": "IntelliJ IDEA",
//!   "transport": "ws",
//!   "authToken": "..."
//! }
//! ```
//!
//! Filename = port (e.g. `12345.lock` ⇒ MCP server listens on `12345`).
//!
//! Search paths (checked in order):
//! 1. `~/.claude/ide/*.lock` — upstream plugin's directory (piggyback path)
//! 2. `~/.crab/ide/*.lock` — our future plugin (self-hosted path)

#![allow(dead_code)] // R1 scaffolding; wired up in R2

use serde::Deserialize;
use std::path::PathBuf;

/// Parsed contents of a single `.lock` file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    pub pid: u32,
    #[serde(default)]
    pub workspace_folders: Vec<PathBuf>,
    pub ide_name: String,
    /// `"ws"` | `"sse"` — MCP transport the plugin is serving.
    pub transport: String,
    pub auth_token: String,
}

/// A discovered endpoint: the port (from filename) plus parsed lockfile.
#[derive(Debug, Clone)]
pub struct DiscoveredEndpoint {
    pub port: u16,
    pub lockfile: Lockfile,
    /// Path that yielded this endpoint, for logging / debug.
    pub source: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("no home directory available")]
    NoHome,
    #[error("io error reading {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {path:?}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Return all IDE endpoints discoverable on this host.
///
/// Empty vec means "no IDE currently exposing a plugin endpoint" — a
/// non-error condition; downstream code should degrade gracefully.
pub fn discover() -> Result<Vec<DiscoveredEndpoint>, DiscoverError> {
    // R2: walk ~/.claude/ide/ and ~/.crab/ide/, parse every *.lock,
    // extract port from filename, deserialize JSON.
    Ok(Vec::new())
}
