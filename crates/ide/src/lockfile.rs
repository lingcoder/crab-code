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
//! Filename stem = port (e.g. `12345.lock` ⇒ MCP server listens on `12345`).
//!
//! Search paths (checked in order):
//! 1. `~/.claude/ide/*.lock` — upstream plugin's directory (piggyback path)
//! 2. `~/.crab/ide/*.lock` — our future plugin (self-hosted path)
//!
//! Unparseable / badly-named files are skipped with a `tracing::warn`
//! rather than failing the whole scan: a stale file from an older plugin
//! version shouldn't prevent us from finding a live one.

use serde::Deserialize;
use std::path::{Path, PathBuf};

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
}

/// Directories to scan for `*.lock` files, in priority order.
///
/// Earlier entries take precedence when multiple lockfiles exist on the
/// same port. Missing directories are silently skipped — a non-error.
fn search_dirs() -> Result<Vec<PathBuf>, DiscoverError> {
    let user_dirs = directories::UserDirs::new().ok_or(DiscoverError::NoHome)?;
    let home = user_dirs.home_dir().to_path_buf();
    Ok(vec![
        home.join(".claude").join("ide"),
        home.join(".crab").join("ide"),
    ])
}

/// Scan a single directory for `*.lock` files and append parsed
/// endpoints to `out`.
///
/// Directory-not-found returns `Ok(())` — callers iterate search paths
/// in order and tolerate missing ones. Read errors on the directory
/// itself propagate; per-file parse errors are logged and skipped.
fn scan_dir(dir: &Path, out: &mut Vec<DiscoveredEndpoint>) -> Result<(), DiscoverError> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(DiscoverError::Io {
                path: dir.to_path_buf(),
                source: e,
            });
        }
    };

    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "skipping unreadable dir entry");
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("lock") {
            continue;
        }
        match parse_one(&path) {
            Ok(ep) => out.push(ep),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping malformed lockfile");
            }
        }
    }
    Ok(())
}

/// Parse port from filename and JSON body from the file.
fn parse_one(path: &Path) -> Result<DiscoveredEndpoint, ParseOneError> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or(ParseOneError::BadFilename)?;
    let port: u16 = stem.parse().map_err(|_| ParseOneError::BadFilename)?;

    let bytes = std::fs::read(path).map_err(ParseOneError::Io)?;
    let lockfile: Lockfile = serde_json::from_slice(&bytes).map_err(ParseOneError::Parse)?;

    Ok(DiscoveredEndpoint {
        port,
        lockfile,
        source: path.to_path_buf(),
    })
}

/// Local error for `parse_one` — folded into `tracing::warn` by
/// `scan_dir`, never surfaced to callers.
#[derive(Debug, thiserror::Error)]
enum ParseOneError {
    #[error("filename is not a valid u16 port")]
    BadFilename,
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[source] serde_json::Error),
}

/// Return all IDE endpoints discoverable on this host.
///
/// Empty vec means "no IDE currently exposing a plugin endpoint" — a
/// non-error condition; downstream code should degrade gracefully.
///
/// When multiple `.lock` files exist for the same port across the two
/// search directories, the first-discovered wins (i.e. `~/.claude/ide/`
/// shadows `~/.crab/ide/`). We don't actively dedupe beyond that — a
/// single plugin writing both paths is its own bug.
pub fn discover() -> Result<Vec<DiscoveredEndpoint>, DiscoverError> {
    let mut endpoints = Vec::new();
    for dir in search_dirs()? {
        scan_dir(&dir, &mut endpoints)?;
    }
    Ok(endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lock(dir: &Path, port: u16, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{port}.lock"));
        std::fs::write(path, body).unwrap();
    }

    fn sample_body(ide: &str, token: &str) -> String {
        format!(
            r#"{{
                "pid": 42,
                "workspaceFolders": ["/tmp/ws"],
                "ideName": "{ide}",
                "transport": "ws",
                "authToken": "{token}"
            }}"#
        )
    }

    #[test]
    fn parses_well_formed_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        write_lock(tmp.path(), 12345, &sample_body("IntelliJ IDEA", "tok-abc"));
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].port, 12345);
        assert_eq!(out[0].lockfile.ide_name, "IntelliJ IDEA");
        assert_eq!(out[0].lockfile.auth_token, "tok-abc");
        assert_eq!(out[0].lockfile.transport, "ws");
        assert_eq!(out[0].lockfile.pid, 42);
    }

    #[test]
    fn skips_non_lock_extension() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "hello").unwrap();
        std::fs::write(tmp.path().join("readme.md"), "hi").unwrap();
        write_lock(tmp.path(), 9999, &sample_body("VSCode", "t"));
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].port, 9999);
    }

    #[test]
    fn skips_bad_filename_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        // Non-numeric filename — should be skipped, not fail.
        std::fs::write(tmp.path().join("notaport.lock"), sample_body("VSCode", "t")).unwrap();
        write_lock(tmp.path(), 8080, &sample_body("VSCode", "t"));
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].port, 8080);
    }

    #[test]
    fn skips_malformed_json_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("7070.lock"), "{ not json }").unwrap();
        write_lock(tmp.path(), 8080, &sample_body("VSCode", "t"));
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].port, 8080);
    }

    #[test]
    fn port_out_of_range_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        // 99999 > u16::MAX — filename parse fails, file skipped.
        std::fs::write(tmp.path().join("99999.lock"), sample_body("VSCode", "t")).unwrap();
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn missing_directory_is_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let mut out = Vec::new();
        scan_dir(&missing, &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn workspace_folders_defaults_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let body = r#"{
            "pid": 1,
            "ideName": "VSCode",
            "transport": "ws",
            "authToken": "t"
        }"#;
        write_lock(tmp.path(), 6000, body);
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].lockfile.workspace_folders.is_empty());
    }

    #[test]
    fn multiple_endpoints_in_one_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_lock(tmp.path(), 1111, &sample_body("A", "x"));
        write_lock(tmp.path(), 2222, &sample_body("B", "y"));
        let mut out = Vec::new();
        scan_dir(tmp.path(), &mut out).unwrap();
        assert_eq!(out.len(), 2);
        let mut ports: Vec<u16> = out.iter().map(|e| e.port).collect();
        ports.sort_unstable();
        assert_eq!(ports, vec![1111, 2222]);
    }
}
