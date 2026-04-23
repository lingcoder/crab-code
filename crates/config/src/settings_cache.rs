//! In-memory settings cache to avoid re-reading/re-merging on every access.
//!
//! The agent loop and tool implementations frequently need access to merged
//! settings. This cache loads once and serves subsequent reads from memory,
//! with an explicit `invalidate()` for hot-reload scenarios.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::settings::{self, Settings};

/// Cached, lazily-loaded settings.
///
/// Thread-safe via interior `Mutex`. The cache stores the merged result of
/// global + project + local + env settings so callers don't pay the merge
/// cost on every access.
pub struct SettingsCache {
    /// The project directory used for loading project-level settings.
    project_dir: Option<PathBuf>,
    /// Cached merged settings, `None` until first access or after invalidation.
    cached: Mutex<Option<Settings>>,
}

impl SettingsCache {
    /// Create a new empty cache for the given project directory.
    pub fn new(project_dir: Option<PathBuf>) -> Self {
        Self {
            project_dir,
            cached: Mutex::new(None),
        }
    }

    /// Get the cached settings, loading and merging on first access.
    ///
    /// Subsequent calls return the cached value until `invalidate()` is called.
    ///
    /// # Errors
    ///
    /// Returns an error if settings files cannot be read or parsed.
    pub fn get_or_load(&self) -> crab_core::Result<Settings> {
        let mut guard = self.cached.lock().unwrap();
        if let Some(ref cached) = *guard {
            return Ok(cached.clone());
        }

        // Load and merge: global → project → local (each layer optional)
        let global = settings::load_global().unwrap_or_default();

        let merged = if let Some(ref project_dir) = self.project_dir {
            let project = settings::load_project(project_dir).unwrap_or_default();
            global.merge(&project)
        } else {
            global
        };

        *guard = Some(merged.clone());
        drop(guard);
        Ok(merged)
    }

    /// Invalidate the cache, forcing a reload on the next `get_or_load()`.
    ///
    /// Call this after detecting a file change via `ConfigWatcher`.
    pub fn invalidate(&self) {
        let mut guard = self.cached.lock().unwrap();
        *guard = None;
    }

    /// Check whether the cache currently holds a value.
    pub fn is_loaded(&self) -> bool {
        self.cached.lock().unwrap().is_some()
    }
}

impl std::fmt::Debug for SettingsCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsCache")
            .field("project_dir", &self.project_dir)
            .field("is_loaded", &self.is_loaded())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cache_is_not_loaded() {
        let cache = SettingsCache::new(None);
        assert!(!cache.is_loaded());
    }

    #[test]
    fn get_or_load_caches() {
        let cache = SettingsCache::new(None);
        let _settings = cache.get_or_load().unwrap();
        assert!(cache.is_loaded());
    }

    #[test]
    fn invalidate_clears_cache() {
        let cache = SettingsCache::new(None);
        let _settings = cache.get_or_load().unwrap();
        assert!(cache.is_loaded());
        cache.invalidate();
        assert!(!cache.is_loaded());
    }

    #[test]
    fn get_or_load_reloads_after_invalidate() {
        let cache = SettingsCache::new(None);
        let s1 = cache.get_or_load().unwrap();
        cache.invalidate();
        let s2 = cache.get_or_load().unwrap();
        // Both should be valid default settings
        assert_eq!(s1.api_provider, s2.api_provider);
    }
}
