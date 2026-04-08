//! Watch skill/config files for changes and trigger hook re-registration.
//!
//! Monitors filesystem paths for modifications and invokes a callback when
//! changes are detected. This allows the hook and skill systems to
//! automatically reload when skill files or configuration are edited.
//!
//! Maps to CCB `hooks/fileChangedWatcher.ts`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

/// Debounce interval: ignore rapid successive events within this window.
const DEBOUNCE_MS: u64 = 500;

/// Polling interval when filesystem notifications are unavailable.
const POLL_INTERVAL_SECS: u64 = 5;

// ─── File watcher ──────────────────────────────────────────────────────

/// Watches a set of filesystem paths and triggers a callback on changes.
pub struct HookFileWatcher {
    /// Paths being monitored for changes.
    watched_paths: Vec<PathBuf>,
    /// Shutdown signal.
    shutdown: Arc<Notify>,
}

impl HookFileWatcher {
    /// Create a new watcher with no paths.
    #[must_use]
    pub fn new() -> Self {
        Self {
            watched_paths: Vec::new(),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Add a path to the watch list.
    ///
    /// The path can be a file or directory. Directories are watched
    /// recursively for any `.md` or `.json` file changes.
    pub fn watch(&mut self, path: PathBuf) {
        if !self.watched_paths.contains(&path) {
            self.watched_paths.push(path);
        }
    }

    /// Remove a path from the watch list.
    ///
    /// Returns `true` if the path was found and removed.
    pub fn unwatch(&mut self, path: &Path) -> bool {
        let before = self.watched_paths.len();
        self.watched_paths.retain(|p| p != path);
        self.watched_paths.len() < before
    }

    /// Get the list of currently watched paths.
    #[must_use]
    pub fn watched_paths(&self) -> &[PathBuf] {
        &self.watched_paths
    }

    /// Get a handle to signal shutdown.
    #[must_use]
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.shutdown)
    }

    /// Start watching and call `on_change` when a watched file is modified.
    ///
    /// Uses a polling approach: periodically checks file mtimes and calls
    /// the callback when changes are detected. The `notify` crate provides
    /// native FS events but requires adding it as a dependency; this polling
    /// approach works everywhere and is sufficient for config/skill files
    /// that change infrequently.
    ///
    /// Runs until the shutdown signal is received.
    pub async fn run<F: Fn(&Path) + Send + 'static>(self, on_change: F) {
        use std::collections::HashMap;

        if self.watched_paths.is_empty() {
            return;
        }

        // Snapshot initial mtimes
        let mut known_mtimes: HashMap<PathBuf, std::time::SystemTime> = HashMap::new();
        for path in &self.watched_paths {
            collect_file_mtimes(path, &mut known_mtimes);
        }

        let poll_interval = Duration::from_secs(POLL_INTERVAL_SECS);
        let debounce = Duration::from_millis(DEBOUNCE_MS);
        let mut last_callback = std::time::Instant::now()
            .checked_sub(debounce)
            .unwrap_or(std::time::Instant::now());

        loop {
            tokio::select! {
                () = self.shutdown.notified() => break,
                () = tokio::time::sleep(poll_interval) => {
                    let mut current_mtimes: HashMap<PathBuf, std::time::SystemTime> = HashMap::new();
                    for path in &self.watched_paths {
                        collect_file_mtimes(path, &mut current_mtimes);
                    }

                    // Find changed files
                    for (path, mtime) in &current_mtimes {
                        let changed = known_mtimes
                            .get(path)
                            .is_none_or(|prev| prev != mtime);

                        if changed && last_callback.elapsed() >= debounce {
                            on_change(path);
                            last_callback = std::time::Instant::now();
                        }
                    }

                    // Check for deleted files
                    for path in known_mtimes.keys() {
                        if !current_mtimes.contains_key(path)
                            && last_callback.elapsed() >= debounce
                        {
                            on_change(path);
                            last_callback = std::time::Instant::now();
                        }
                    }

                    known_mtimes = current_mtimes;
                }
            }
        }
    }
}

impl Default for HookFileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Collect file mtimes from a path (file or directory).
/// For directories, recurse and collect `.md` and `.json` files.
fn collect_file_mtimes(
    path: &Path,
    mtimes: &mut std::collections::HashMap<PathBuf, std::time::SystemTime>,
) {
    if path.is_file() {
        if let Ok(meta) = path.metadata()
            && let Ok(mtime) = meta.modified()
        {
            mtimes.insert(path.to_path_buf(), mtime);
        }
        return;
    }

    if path.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                collect_file_mtimes(&child, mtimes);
            } else if let Some(ext) = child.extension().and_then(|e| e.to_str())
                && (ext == "md" || ext == "json")
                && let Ok(meta) = child.metadata()
                && let Ok(mtime) = meta.modified()
            {
                mtimes.insert(child, mtime);
            }
        }
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_watcher_is_empty() {
        let watcher = HookFileWatcher::new();
        assert!(watcher.watched_paths().is_empty());
    }

    #[test]
    fn watch_adds_path() {
        let mut watcher = HookFileWatcher::new();
        watcher.watch(PathBuf::from("/tmp/skills"));
        assert_eq!(watcher.watched_paths().len(), 1);
        assert_eq!(watcher.watched_paths()[0], Path::new("/tmp/skills"));
    }

    #[test]
    fn watch_deduplicates() {
        let mut watcher = HookFileWatcher::new();
        watcher.watch(PathBuf::from("/tmp/skills"));
        watcher.watch(PathBuf::from("/tmp/skills"));
        assert_eq!(watcher.watched_paths().len(), 1);
    }

    #[test]
    fn unwatch_removes_path() {
        let mut watcher = HookFileWatcher::new();
        watcher.watch(PathBuf::from("/tmp/a"));
        watcher.watch(PathBuf::from("/tmp/b"));
        assert!(watcher.unwatch(Path::new("/tmp/a")));
        assert_eq!(watcher.watched_paths().len(), 1);
        assert_eq!(watcher.watched_paths()[0], Path::new("/tmp/b"));
    }

    #[test]
    fn unwatch_nonexistent_returns_false() {
        let mut watcher = HookFileWatcher::new();
        assert!(!watcher.unwatch(Path::new("/tmp/nope")));
    }

    #[test]
    fn default_watcher_is_empty() {
        let watcher = HookFileWatcher::default();
        assert!(watcher.watched_paths().is_empty());
    }

    #[tokio::test]
    async fn run_empty_paths_returns_immediately() {
        let watcher = HookFileWatcher::new();
        // Should return immediately since no paths to watch
        watcher.run(|_| {}).await;
    }

    #[tokio::test]
    async fn run_can_be_shutdown() {
        let mut watcher = HookFileWatcher::new();
        let temp = std::env::temp_dir().join("crab_watcher_test");
        let _ = std::fs::create_dir_all(&temp);
        watcher.watch(temp.clone());

        let shutdown = watcher.shutdown_handle();

        let handle = tokio::spawn(async move {
            watcher.run(|_| {}).await;
        });

        // Signal shutdown after a brief delay
        tokio::time::sleep(Duration::from_millis(100)).await;
        shutdown.notify_one();

        // Should complete within a reasonable time
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("watcher should shut down")
            .expect("task should not panic");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
