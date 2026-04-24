//! Persistent global state (`~/.crab/state.json`).
//!
//! Separate from `Settings` (user-editable config): `GlobalState` is
//! read/written programmatically to track runtime state such as onboarding
//! completion and per-project trust records.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const STATE_FILE: &str = "state.json";

/// Runtime state persisted across sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct GlobalState {
    pub schema_version: u32,
    pub has_completed_onboarding: bool,
    /// Per-project trust records keyed by canonical project path.
    pub project_trust: HashMap<String, ProjectTrust>,
    /// Version string of the last release whose changelog the user saw. When
    /// this differs from the current binary version, the welcome screen is
    /// shown and this field is refreshed on dismissal. Mirrors CCB's
    /// `lastReleaseNotesSeen` in `~/.claude/config.json`.
    pub last_welcome_version: Option<String>,
}

/// Trust record for a single project directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrust {
    pub accepted: bool,
    /// Hash of the project's `.crab/` configuration directory contents
    /// at the time trust was granted. Re-prompt when this changes.
    pub settings_hash: String,
    /// ISO 8601 timestamp when the user accepted.
    pub accepted_at: String,
}

/// Path to `~/.crab/state.json`.
#[must_use]
pub fn state_path() -> PathBuf {
    crate::settings::global_config_dir().join(STATE_FILE)
}

/// Load global state from disk. Returns `Default` if the file is missing
/// or unparseable (first run).
#[must_use]
pub fn load() -> GlobalState {
    let path = state_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => GlobalState::default(),
    }
}

/// Persist global state to disk. Creates the parent directory if needed.
pub fn save(state: &GlobalState) -> crab_core::Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crab_core::Error::Config(format!(
                "failed to create state directory '{}': {e}",
                parent.display()
            ))
        })?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| crab_core::Error::Config(format!("failed to serialize global state: {e}")))?;
    std::fs::write(&path, json).map_err(|e| {
        crab_core::Error::Config(format!(
            "failed to write state file '{}': {e}",
            path.display()
        ))
    })?;
    Ok(())
}

/// Compute a hash of a project's `.crab/` directory for trust comparison.
///
/// Hashes the sorted list of filenames plus the content of `settings.json`
/// (if present). Returns an empty string if the directory doesn't exist.
#[must_use]
pub fn compute_project_hash(project_dir: &Path) -> String {
    use std::hash::{Hash, Hasher};

    let crab_dir = project_dir.join(".crab");
    if !crab_dir.is_dir() {
        return String::new();
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    // Hash sorted file names
    let mut entries: Vec<String> = std::fs::read_dir(&crab_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    entries.sort();
    for name in &entries {
        name.hash(&mut hasher);
    }

    // Hash settings.json content if present
    if let Ok(content) = std::fs::read_to_string(crab_dir.join("settings.json")) {
        content.hash(&mut hasher);
    }

    // Hash AGENTS.md content if present
    let agents_md = project_dir.join("AGENTS.md");
    if let Ok(content) = std::fs::read_to_string(agents_md) {
        content.hash(&mut hasher);
    }

    format!("{:016x}", hasher.finish())
}

/// Check whether a project directory needs a trust prompt.
///
/// Returns `true` if:
/// - The project has a `.crab/` directory or `AGENTS.md` file, AND
/// - There is no matching trust record, OR the recorded hash differs.
#[must_use]
pub fn needs_trust_prompt(state: &GlobalState, project_dir: &Path) -> bool {
    let has_config = project_dir.join(".crab").is_dir() || project_dir.join("AGENTS.md").exists();
    if !has_config {
        return false;
    }

    let canonical = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let key = canonical.to_string_lossy().to_string();
    let current_hash = compute_project_hash(project_dir);

    !matches!(state.project_trust.get(&key), Some(trust) if trust.accepted && trust.settings_hash == current_hash)
}

/// Whether the welcome screen should be shown for this binary version.
///
/// Returns true when `state.last_welcome_version` is absent or differs from
/// the current binary version. The caller is responsible for refreshing the
/// field (and calling [`save`]) after showing the welcome so subsequent
/// starts silently skip it.
#[must_use]
pub fn should_show_welcome(state: &GlobalState, current_version: &str) -> bool {
    match &state.last_welcome_version {
        Some(seen) => seen != current_version,
        None => true,
    }
}

/// Record that the welcome screen for `version` was shown.
pub fn record_welcome_seen(state: &mut GlobalState, version: &str) {
    state.last_welcome_version = Some(version.to_owned());
}

/// Record that the user accepted trust for a project.
pub fn record_trust(state: &mut GlobalState, project_dir: &Path) {
    let canonical = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let key = canonical.to_string_lossy().to_string();
    let hash = compute_project_hash(project_dir);
    let now = chrono::Utc::now().to_rfc3339();
    state.project_trust.insert(
        key,
        ProjectTrust {
            accepted: true,
            settings_hash: hash,
            accepted_at: now,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("crab-state-{name}-{ts}"))
    }

    #[test]
    fn default_state() {
        let state = GlobalState::default();
        assert_eq!(state.schema_version, 0);
        assert!(!state.has_completed_onboarding);
        assert!(state.project_trust.is_empty());
    }

    #[test]
    fn roundtrip_serialize() {
        let mut state = GlobalState::default();
        state.has_completed_onboarding = true;
        state.project_trust.insert(
            "/home/user/project".into(),
            ProjectTrust {
                accepted: true,
                settings_hash: "abc123".into(),
                accepted_at: "2026-01-01T00:00:00Z".into(),
            },
        );

        let json = serde_json::to_string(&state).unwrap();
        let restored: GlobalState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let state = load();
        assert_eq!(state.schema_version, 0);
    }

    #[test]
    fn compute_hash_no_crab_dir() {
        let dir = temp_dir("no-crab");
        fs::create_dir_all(&dir).unwrap();
        assert!(compute_project_hash(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_hash_with_crab_dir() {
        let dir = temp_dir("with-crab");
        let crab = dir.join(".crab");
        fs::create_dir_all(&crab).unwrap();
        fs::write(crab.join("settings.json"), r#"{"model":"test"}"#).unwrap();

        let hash = compute_project_hash(&dir);
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 16);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_hash_changes_on_content_change() {
        let dir = temp_dir("hash-change");
        let crab = dir.join(".crab");
        fs::create_dir_all(&crab).unwrap();
        fs::write(crab.join("settings.json"), r#"{"model":"v1"}"#).unwrap();

        let hash1 = compute_project_hash(&dir);

        fs::write(crab.join("settings.json"), r#"{"model":"v2"}"#).unwrap();

        let hash2 = compute_project_hash(&dir);
        assert_ne!(hash1, hash2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_trust_no_config() {
        let dir = temp_dir("no-config");
        fs::create_dir_all(&dir).unwrap();
        let state = GlobalState::default();
        assert!(!needs_trust_prompt(&state, &dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_trust_new_project() {
        let dir = temp_dir("new-proj");
        fs::create_dir_all(dir.join(".crab")).unwrap();
        let state = GlobalState::default();
        assert!(needs_trust_prompt(&state, &dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_trust_already_accepted() {
        let dir = temp_dir("accepted");
        let crab = dir.join(".crab");
        fs::create_dir_all(&crab).unwrap();
        fs::write(crab.join("settings.json"), "{}").unwrap();

        let mut state = GlobalState::default();
        record_trust(&mut state, &dir);

        assert!(!needs_trust_prompt(&state, &dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_trust_after_config_change() {
        let dir = temp_dir("changed");
        let crab = dir.join(".crab");
        fs::create_dir_all(&crab).unwrap();
        fs::write(crab.join("settings.json"), r#"{"v":1}"#).unwrap();

        let mut state = GlobalState::default();
        record_trust(&mut state, &dir);

        // Modify config after trust was recorded
        fs::write(crab.join("settings.json"), r#"{"v":2}"#).unwrap();

        assert!(needs_trust_prompt(&state, &dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn welcome_shown_when_field_absent() {
        let state = GlobalState::default();
        assert!(should_show_welcome(&state, "0.1.0"));
    }

    #[test]
    fn welcome_skipped_when_version_matches() {
        let mut state = GlobalState::default();
        record_welcome_seen(&mut state, "0.1.0");
        assert!(!should_show_welcome(&state, "0.1.0"));
    }

    #[test]
    fn welcome_shown_when_version_differs() {
        let mut state = GlobalState::default();
        record_welcome_seen(&mut state, "0.1.0");
        assert!(should_show_welcome(&state, "0.2.0"));
    }

    #[test]
    fn welcome_version_roundtrips_through_serde() {
        let mut state = GlobalState::default();
        record_welcome_seen(&mut state, "0.3.1");
        let json = serde_json::to_string(&state).unwrap();
        let restored: GlobalState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.last_welcome_version.as_deref(), Some("0.3.1"));
    }

    #[test]
    fn needs_trust_agents_md_only() {
        let dir = temp_dir("crab-md");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("AGENTS.md"), "# Instructions").unwrap();
        let state = GlobalState::default();
        assert!(needs_trust_prompt(&state, &dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
