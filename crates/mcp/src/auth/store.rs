//! Persistent token store for MCP OAuth tokens.
//!
//! Each MCP server's token lives in its own JSON file under a directory
//! (typically `~/.crab/mcp/tokens/`). This keeps a bad write from
//! corrupting tokens for other servers, makes manual inspection easy,
//! and allows per-file filesystem permissions.
//!
//! Concurrency: writes take an advisory `fd-lock` on the destination file
//! so two crab processes can't clobber one another's saves. Reads are
//! lock-free — workers check `AuthToken::is_expired` after `get()`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::types::AuthToken;

/// In-memory token cache backed by per-server JSON files on disk.
#[derive(Debug, Default)]
pub struct TokenStore {
    tokens: HashMap<String, AuthToken>,
}

impl TokenStore {
    /// Create an empty store. Use [`Self::load_from_disk`] to seed from
    /// on-disk state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load all tokens from `dir`. Non-existent directory yields an empty
    /// store. Corrupt individual files are skipped with a `tracing::warn`
    /// so one bad file does not prevent other servers from authenticating.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if the directory cannot be opened (permission
    /// denied, is a regular file, etc.). Individual file parse errors
    /// degrade to a warning + skip.
    pub fn load_from_disk(dir: &Path) -> crab_core::Result<Self> {
        let mut store = Self::new();
        if !dir.exists() {
            return Ok(store);
        }
        let entries = fs::read_dir(dir).map_err(|e| {
            crab_core::Error::Other(format!("failed to read token dir {}: {e}", dir.display()))
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(server_name) = path.file_stem().and_then(|s| s.to_str()).map(String::from)
            else {
                tracing::warn!(path = %path.display(), "token file has unreadable name");
                continue;
            };
            match fs::read_to_string(&path) {
                Ok(body) => match serde_json::from_str::<AuthToken>(&body) {
                    Ok(tok) => {
                        store.tokens.insert(server_name, tok);
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "skipping corrupt token file",
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "cannot read token file",
                    );
                }
            }
        }
        Ok(store)
    }

    /// Persist every cached token as a separate JSON file under `dir`.
    ///
    /// Creates `dir` if missing. On unix, file mode is restricted to 0o600
    /// so other users on the system cannot read the bearer token.
    ///
    /// # Errors
    ///
    /// Returns `Err` on directory creation failure or any individual file
    /// write failure. Successfully-written files are not rolled back.
    pub fn save_to_disk(&self, dir: &Path) -> crab_core::Result<()> {
        fs::create_dir_all(dir).map_err(|e| {
            crab_core::Error::Other(format!("failed to create token dir {}: {e}", dir.display()))
        })?;
        for (name, tok) in &self.tokens {
            let path = dir.join(format!("{name}.json"));
            let body = serde_json::to_string_pretty(tok).map_err(|e| {
                crab_core::Error::Other(format!("failed to serialize token for {name}: {e}"))
            })?;
            write_token_file(&path, &body)?;
        }
        Ok(())
    }

    /// Look up a token by server name. Does not check expiry — callers
    /// should use [`AuthToken::is_expired`] to decide on refresh.
    #[must_use]
    pub fn get(&self, server_name: &str) -> Option<&AuthToken> {
        self.tokens.get(server_name)
    }

    /// Insert or overwrite a token for a server.
    pub fn insert(&mut self, server_name: impl Into<String>, token: AuthToken) {
        self.tokens.insert(server_name.into(), token);
    }

    /// Remove a server's token from the in-memory cache.
    ///
    /// Does not delete the corresponding on-disk file — the caller is
    /// responsible for `fs::remove_file` on the matching path if desired.
    pub fn remove(&mut self, server_name: &str) -> Option<AuthToken> {
        self.tokens.remove(server_name)
    }

    /// List server names with a cached token.
    pub fn server_names(&self) -> impl Iterator<Item = &str> {
        self.tokens.keys().map(String::as_str)
    }

    /// Number of cached tokens.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Is the store empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

/// Write `body` to `path`, applying restrictive permissions on unix.
///
/// Uses `fd-lock` exclusive advisory lock for write-safety under concurrent
/// crab processes. The lock is scoped to the file, so different token
/// files do not contend.
fn write_token_file(path: &Path, body: &str) -> crab_core::Result<()> {
    use std::io::Write;

    let f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| crab_core::Error::Other(format!("failed to open {}: {e}", path.display())))?;

    // Apply 0o600 on unix before writing anything.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms).map_err(|e| {
            crab_core::Error::Other(format!("failed to set 0o600 on {}: {e}", path.display()))
        })?;
    }

    let mut lock = fd_lock::RwLock::new(f);
    let mut guard = lock
        .write()
        .map_err(|e| crab_core::Error::Other(format!("fd-lock failed: {e}")))?;
    guard
        .write_all(body.as_bytes())
        .map_err(|e| crab_core::Error::Other(format!("write failed: {e}")))?;
    Ok(())
}

/// Default token directory under the user's crab config root.
#[must_use]
pub fn default_token_dir() -> PathBuf {
    if let Some(base) = directories::ProjectDirs::from("", "", "crab") {
        base.config_dir().join("mcp").join("tokens")
    } else {
        PathBuf::from(".crab/mcp/tokens")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_token(value: &str) -> AuthToken {
        AuthToken {
            access_token: value.into(),
            token_type: "Bearer".into(),
            expires_at: Some(9_999_999_999),
            refresh_token: Some(format!("r-{value}")),
        }
    }

    #[test]
    fn insert_get_remove_roundtrip() {
        let mut store = TokenStore::new();
        assert!(store.is_empty());
        store.insert("github", sample_token("tok-gh"));
        store.insert("slack", sample_token("tok-slack"));
        assert_eq!(store.len(), 2);
        assert_eq!(store.get("github").unwrap().access_token, "tok-gh");
        let removed = store.remove("github").unwrap();
        assert_eq!(removed.access_token, "tok-gh");
        assert_eq!(store.len(), 1);
        assert!(store.get("github").is_none());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = TokenStore::new();
        store.insert("github", sample_token("tok-gh"));
        store.insert("slack", sample_token("tok-slack"));
        store.save_to_disk(tmp.path()).unwrap();

        let loaded = TokenStore::load_from_disk(tmp.path()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("github").unwrap().access_token, "tok-gh");
        assert_eq!(
            loaded.get("slack").unwrap().refresh_token.as_deref(),
            Some("r-tok-slack")
        );
    }

    #[test]
    fn load_from_nonexistent_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TokenStore::load_from_disk(&tmp.path().join("does-not-exist")).unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn load_skips_non_json_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("note.txt"), "not json").unwrap();
        let store = TokenStore::load_from_disk(tmp.path()).unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn load_skips_corrupt_json_with_warn() {
        let tmp = tempfile::tempdir().unwrap();
        // Valid file
        let mut good = TokenStore::new();
        good.insert("github", sample_token("tok-gh"));
        good.save_to_disk(tmp.path()).unwrap();
        // Corrupt file
        fs::write(tmp.path().join("broken.json"), "{ not valid json").unwrap();

        let loaded = TokenStore::load_from_disk(tmp.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("github").unwrap().access_token, "tok-gh");
        assert!(loaded.get("broken").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_permission_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let mut store = TokenStore::new();
        store.insert("github", sample_token("tok-gh"));
        store.save_to_disk(tmp.path()).unwrap();
        let meta = fs::metadata(tmp.path().join("github.json")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn server_names_lists_all() {
        let mut store = TokenStore::new();
        store.insert("a", sample_token("x"));
        store.insert("b", sample_token("y"));
        let mut names: Vec<&str> = store.server_names().collect();
        names.sort_unstable();
        assert_eq!(names, vec!["a", "b"]);
    }
}
