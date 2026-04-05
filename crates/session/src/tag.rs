//! Session tagging system.
//!
//! Provides [`SessionTagStore`] — a persistent tag index that maps session IDs
//! to user-defined tags. Supports adding/removing tags, filtering sessions by
//! tag, and searching tags.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Tag store ────────────────────────────────────────────────────────

/// Persistent tag index mapping session IDs to their tags.
///
/// Stored on disk as a single JSON file (`tags.json`) alongside session files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTagStore {
    /// Session ID → set of tags.
    #[serde(default)]
    sessions: BTreeMap<String, BTreeSet<String>>,

    /// Path to the tag index file (not serialized).
    #[serde(skip)]
    path: PathBuf,
}

impl SessionTagStore {
    /// Create a new empty tag store that will persist to `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            sessions: BTreeMap::new(),
            path,
        }
    }

    /// Load the tag store from disk. Returns an empty store if the file
    /// doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> crab_common::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }
        let data = std::fs::read_to_string(path)?;
        let mut store: Self = serde_json::from_str(&data)
            .map_err(|e| crab_common::Error::Other(format!("parse tags: {e}")))?;
        store.path = path.to_path_buf();
        Ok(store)
    }

    /// Save the tag store to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save(&self) -> crab_common::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| crab_common::Error::Other(format!("serialize tags: {e}")))?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }

    /// Add a tag to a session.
    pub fn add_tag(&mut self, session_id: &str, tag: &str) {
        self.sessions
            .entry(session_id.to_string())
            .or_default()
            .insert(normalize_tag(tag));
    }

    /// Remove a tag from a session.
    /// Returns `true` if the tag was present and removed.
    pub fn remove_tag(&mut self, session_id: &str, tag: &str) -> bool {
        let normalized = normalize_tag(tag);
        if let Some(tags) = self.sessions.get_mut(session_id) {
            let removed = tags.remove(&normalized);
            if tags.is_empty() {
                self.sessions.remove(session_id);
            }
            removed
        } else {
            false
        }
    }

    /// Get all tags for a session.
    #[must_use]
    pub fn tags_for(&self, session_id: &str) -> Vec<&str> {
        self.sessions
            .get(session_id)
            .map(|tags| tags.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Get all session IDs that have a specific tag.
    #[must_use]
    pub fn sessions_with_tag(&self, tag: &str) -> Vec<&str> {
        let normalized = normalize_tag(tag);
        self.sessions
            .iter()
            .filter(|(_, tags)| tags.contains(&normalized))
            .map(|(sid, _)| sid.as_str())
            .collect()
    }

    /// Get all session IDs that have all of the specified tags.
    #[must_use]
    pub fn sessions_with_all_tags(&self, tags: &[&str]) -> Vec<&str> {
        let normalized: Vec<String> = tags.iter().map(|t| normalize_tag(t)).collect();
        self.sessions
            .iter()
            .filter(|(_, session_tags)| normalized.iter().all(|t| session_tags.contains(t)))
            .map(|(sid, _)| sid.as_str())
            .collect()
    }

    /// Get all session IDs that have any of the specified tags.
    #[must_use]
    pub fn sessions_with_any_tag(&self, tags: &[&str]) -> Vec<&str> {
        let normalized: Vec<String> = tags.iter().map(|t| normalize_tag(t)).collect();
        self.sessions
            .iter()
            .filter(|(_, session_tags)| normalized.iter().any(|t| session_tags.contains(t)))
            .map(|(sid, _)| sid.as_str())
            .collect()
    }

    /// Get all unique tags across all sessions, sorted.
    #[must_use]
    pub fn all_tags(&self) -> Vec<&str> {
        let mut tags: BTreeSet<&str> = BTreeSet::new();
        for session_tags in self.sessions.values() {
            for tag in session_tags {
                tags.insert(tag);
            }
        }
        tags.into_iter().collect()
    }

    /// Search tags by prefix (case-insensitive).
    #[must_use]
    pub fn search_tags(&self, prefix: &str) -> Vec<&str> {
        let lower = prefix.to_lowercase();
        self.all_tags()
            .into_iter()
            .filter(|t| t.starts_with(&lower))
            .collect()
    }

    /// Remove all tags for a session (e.g. when session is deleted).
    pub fn remove_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    /// Number of sessions with at least one tag.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

/// Normalize a tag: trim whitespace, lowercase, replace spaces with hyphens.
fn normalize_tag(tag: &str) -> String {
    tag.trim().to_lowercase().replace(' ', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> SessionTagStore {
        SessionTagStore::new(PathBuf::from("/tmp/tags.json"))
    }

    // ── Basic operations ─────────────────────────────────────────────

    #[test]
    fn add_and_get_tags() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "urgent");
        let tags = store.tags_for("s1");
        assert_eq!(tags, vec!["bugfix", "urgent"]);
    }

    #[test]
    fn tags_for_unknown_session() {
        let store = make_store();
        assert!(store.tags_for("nope").is_empty());
    }

    #[test]
    fn add_duplicate_tag_is_noop() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "bugfix");
        assert_eq!(store.tags_for("s1").len(), 1);
    }

    #[test]
    fn remove_tag() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "feature");
        assert!(store.remove_tag("s1", "bugfix"));
        assert_eq!(store.tags_for("s1"), vec!["feature"]);
    }

    #[test]
    fn remove_last_tag_cleans_session_entry() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.remove_tag("s1", "bugfix");
        assert!(store.is_empty());
    }

    #[test]
    fn remove_nonexistent_tag_returns_false() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        assert!(!store.remove_tag("s1", "nope"));
    }

    #[test]
    fn remove_tag_from_unknown_session() {
        let mut store = make_store();
        assert!(!store.remove_tag("nope", "tag"));
    }

    // ── Normalization ────────────────────────────────────────────────

    #[test]
    fn tag_normalized_lowercase() {
        let mut store = make_store();
        store.add_tag("s1", "BugFix");
        assert_eq!(store.tags_for("s1"), vec!["bugfix"]);
    }

    #[test]
    fn tag_normalized_spaces_to_hyphens() {
        let mut store = make_store();
        store.add_tag("s1", "code review");
        assert_eq!(store.tags_for("s1"), vec!["code-review"]);
    }

    #[test]
    fn tag_normalized_trimmed() {
        let mut store = make_store();
        store.add_tag("s1", "  bugfix  ");
        assert_eq!(store.tags_for("s1"), vec!["bugfix"]);
    }

    // ── Filtering ────────────────────────────────────────────────────

    #[test]
    fn sessions_with_tag() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s2", "feature");
        store.add_tag("s3", "bugfix");

        let results = store.sessions_with_tag("bugfix");
        assert_eq!(results, vec!["s1", "s3"]);
    }

    #[test]
    fn sessions_with_all_tags() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "urgent");
        store.add_tag("s2", "bugfix");
        store.add_tag("s3", "urgent");

        let results = store.sessions_with_all_tags(&["bugfix", "urgent"]);
        assert_eq!(results, vec!["s1"]);
    }

    #[test]
    fn sessions_with_any_tag() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s2", "feature");
        store.add_tag("s3", "research");

        let results = store.sessions_with_any_tag(&["bugfix", "feature"]);
        assert_eq!(results, vec!["s1", "s2"]);
    }

    #[test]
    fn sessions_with_tag_not_found() {
        let store = make_store();
        assert!(store.sessions_with_tag("nope").is_empty());
    }

    // ── All tags / search ────────────────────────────────────────────

    #[test]
    fn all_tags_sorted_unique() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "alpha");
        store.add_tag("s2", "bugfix");
        store.add_tag("s2", "feature");

        assert_eq!(store.all_tags(), vec!["alpha", "bugfix", "feature"]);
    }

    #[test]
    fn search_tags_by_prefix() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "build");
        store.add_tag("s1", "feature");

        let results = store.search_tags("bu");
        assert_eq!(results, vec!["bugfix", "build"]);
    }

    #[test]
    fn search_tags_no_match() {
        let mut store = make_store();
        store.add_tag("s1", "bugfix");
        assert!(store.search_tags("xyz").is_empty());
    }

    // ── Session removal ──────────────────────────────────────────────

    #[test]
    fn remove_session_clears_all_tags() {
        let mut store = make_store();
        store.add_tag("s1", "a");
        store.add_tag("s1", "b");
        store.remove_session("s1");
        assert!(store.tags_for("s1").is_empty());
        assert!(store.is_empty());
    }

    // ── Counts ───────────────────────────────────────────────────────

    #[test]
    fn session_count_and_is_empty() {
        let mut store = make_store();
        assert!(store.is_empty());
        assert_eq!(store.session_count(), 0);

        store.add_tag("s1", "tag");
        assert!(!store.is_empty());
        assert_eq!(store.session_count(), 1);
    }

    // ── Persistence ──────────────────────────────────────────────────

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tags.json");

        let mut store = SessionTagStore::new(path.clone());
        store.add_tag("s1", "bugfix");
        store.add_tag("s1", "urgent");
        store.add_tag("s2", "feature");
        store.save().unwrap();

        let loaded = SessionTagStore::load(&path).unwrap();
        assert_eq!(loaded.tags_for("s1"), vec!["bugfix", "urgent"]);
        assert_eq!(loaded.tags_for("s2"), vec!["feature"]);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let path = PathBuf::from("/tmp/nonexistent-tags-test.json");
        let store = SessionTagStore::load(&path).unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("nested").join("tags.json");

        let mut store = SessionTagStore::new(path.clone());
        store.add_tag("s1", "test");
        store.save().unwrap();

        assert!(path.exists());
    }

    // ── Normalize function ───────────────────────────────────────────

    #[test]
    fn normalize_combined() {
        assert_eq!(normalize_tag("  Code Review  "), "code-review");
        assert_eq!(normalize_tag("BUGFIX"), "bugfix");
        assert_eq!(normalize_tag("a b c"), "a-b-c");
    }
}
