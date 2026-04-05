//! File system watching via [`notify`].
//!
//! Provides a [`FileWatcher`] that monitors a directory tree and delivers
//! [`WatchEvent`]s through a channel. Useful for hot-reloading configuration
//! files (`CRAB.md`, `settings.json`) and detecting workspace changes.
//!
//! Also provides [`WatchConfig`] for event debouncing, pattern-based filtering,
//! and batch aggregation, plus [`EventFilter`] and [`EventDebouncer`] for
//! composing these behaviours.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;

// ── Public types ──────────────────────────────────────────────────────

/// Simplified events emitted by the file watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// A file was created.
    Created(PathBuf),
    /// A file was modified (content or metadata).
    Modified(PathBuf),
    /// A file was removed.
    Removed(PathBuf),
    /// A file was renamed (from, to).
    Renamed { from: PathBuf, to: PathBuf },
}

impl WatchEvent {
    /// The primary path associated with the event.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Created(p) | Self::Modified(p) | Self::Removed(p) => p,
            Self::Renamed { to, .. } => to,
        }
    }
}

// ── Watch configuration ─────────────────────────────────────────────

/// Configuration for watch event processing.
#[derive(Debug, Clone, Serialize)]
pub struct WatchConfig {
    /// Debounce interval in milliseconds. Events for the same file within
    /// this window are collapsed into a single event. Default: 100ms.
    pub debounce_ms: u64,
    /// Path patterns to ignore (matched against any path component).
    /// Default: `.git`, `target`, `node_modules`, `__pycache__`, `.DS_Store`.
    pub ignore_patterns: Vec<String>,
    /// Batch window in milliseconds. After the first event arrives, the
    /// system waits this long to collect more events before delivering
    /// them as a batch. Default: 50ms.
    pub batch_window_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 100,
            ignore_patterns: vec![
                ".git".to_owned(),
                "target".to_owned(),
                "node_modules".to_owned(),
                "__pycache__".to_owned(),
                ".DS_Store".to_owned(),
                "dist".to_owned(),
                "build".to_owned(),
            ],
            batch_window_ms: 50,
        }
    }
}

// ── Event filter ────────────────────────────────────────────────────

/// Filters watch events based on ignore patterns.
#[derive(Debug, Clone)]
pub struct EventFilter {
    ignore_patterns: Vec<String>,
}

impl EventFilter {
    /// Create a filter from the given ignore patterns.
    #[must_use]
    pub fn new(ignore_patterns: Vec<String>) -> Self {
        Self { ignore_patterns }
    }

    /// Create a filter with default ignore patterns.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(WatchConfig::default().ignore_patterns)
    }

    /// Returns `true` if the event should be kept (not ignored).
    #[must_use]
    pub fn should_keep(&self, event: &WatchEvent) -> bool {
        !self.is_ignored(event.path())
    }

    /// Returns `true` if the path matches any ignore pattern.
    #[must_use]
    pub fn is_ignored(&self, path: &Path) -> bool {
        for component in path.components() {
            let s = component.as_os_str().to_string_lossy();
            for pattern in &self.ignore_patterns {
                if s == *pattern {
                    return true;
                }
            }
        }
        false
    }

    /// Filter a batch of events, returning only those not ignored.
    #[must_use]
    pub fn filter_events(&self, events: Vec<WatchEvent>) -> Vec<WatchEvent> {
        events.into_iter().filter(|e| self.should_keep(e)).collect()
    }
}

// ── Event debouncer ─────────────────────────────────────────────────

/// Deduplicates rapid-fire events for the same file path.
///
/// Within the debounce window, only the *last* event for each path is kept.
#[derive(Debug)]
pub struct EventDebouncer {
    debounce: Duration,
    pending: HashMap<PathBuf, (WatchEvent, Instant)>,
}

impl EventDebouncer {
    /// Create a debouncer with the given debounce duration.
    #[must_use]
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            pending: HashMap::new(),
        }
    }

    /// Push a new event. If an event for the same path already exists within
    /// the debounce window, it is replaced.
    pub fn push(&mut self, event: WatchEvent) {
        let path = event.path().to_path_buf();
        self.pending.insert(path, (event, Instant::now()));
    }

    /// Push multiple events.
    pub fn push_many(&mut self, events: Vec<WatchEvent>) {
        for event in events {
            self.push(event);
        }
    }

    /// Drain all events that have been pending longer than the debounce window.
    #[must_use]
    pub fn drain_ready(&mut self) -> Vec<WatchEvent> {
        let now = Instant::now();
        let mut ready = Vec::new();
        self.pending.retain(|_, (event, time)| {
            if now.duration_since(*time) >= self.debounce {
                ready.push(event.clone());
                false
            } else {
                true
            }
        });
        ready.sort_by(|a, b| a.path().cmp(b.path()));
        ready
    }

    /// Drain all pending events regardless of timing.
    #[must_use]
    pub fn drain_all(&mut self) -> Vec<WatchEvent> {
        let mut events: Vec<_> = self.pending.drain().map(|(_, (e, _))| e).collect();
        events.sort_by(|a, b| a.path().cmp(b.path()));
        events
    }

    /// Number of pending (not yet drained) events.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

// ── Batch aggregator ────────────────────────────────────────────────

/// Collects events within a time window and delivers them as a batch.
#[derive(Debug)]
pub struct BatchAggregator {
    window: Duration,
    batch_start: Option<Instant>,
    buffer: Vec<WatchEvent>,
}

impl BatchAggregator {
    /// Create an aggregator with the given batch window.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            batch_start: None,
            buffer: Vec::new(),
        }
    }

    /// Add events to the current batch.
    pub fn add(&mut self, events: Vec<WatchEvent>) {
        if self.batch_start.is_none() && !events.is_empty() {
            self.batch_start = Some(Instant::now());
        }
        self.buffer.extend(events);
    }

    /// Check if the batch window has elapsed and return the batch if so.
    #[must_use]
    pub fn try_flush(&mut self) -> Option<Vec<WatchEvent>> {
        let start = self.batch_start?;
        if Instant::now().duration_since(start) >= self.window {
            self.batch_start = None;
            let batch = std::mem::take(&mut self.buffer);
            if batch.is_empty() {
                None
            } else {
                Some(batch)
            }
        } else {
            None
        }
    }

    /// Force-flush the current batch regardless of timing.
    #[must_use]
    pub fn flush(&mut self) -> Vec<WatchEvent> {
        self.batch_start = None;
        std::mem::take(&mut self.buffer)
    }

    /// Number of events in the current batch.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.buffer.len()
    }
}

/// Watches a directory tree for file changes using the OS-native backend
/// (inotify on Linux, `ReadDirectoryChanges` on Windows, `FSEvents` on macOS).
pub struct FileWatcher {
    /// The underlying notify watcher. Kept alive to maintain the OS watch.
    _watcher: RecommendedWatcher,
    /// Channel receiving simplified events.
    receiver: mpsc::Receiver<WatchEvent>,
    /// Root path being watched.
    root: PathBuf,
}

impl FileWatcher {
    /// Start watching `path` recursively for changes.
    ///
    /// Events are buffered in an internal channel; use [`poll`] or
    /// [`poll_timeout`] to drain them.
    ///
    /// # Errors
    ///
    /// Returns an error if the path does not exist or cannot be watched.
    pub fn new(path: &Path) -> crab_common::Result<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                for we in translate_event(&event) {
                    let _ = tx.send(we);
                }
            }
        })
        .map_err(|e| crab_common::Error::Other(format!("failed to create watcher: {e}")))?;

        watcher.watch(path, RecursiveMode::Recursive).map_err(|e| {
            crab_common::Error::Other(format!("failed to watch {}: {e}", path.display()))
        })?;

        Ok(Self {
            _watcher: watcher,
            receiver: rx,
            root: path.to_path_buf(),
        })
    }

    /// The root directory being watched.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Drain all currently buffered events (non-blocking).
    #[must_use]
    pub fn poll(&self) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// Wait for an event with a timeout.
    ///
    /// Returns `None` if no event arrives within `timeout`.
    #[must_use]
    pub fn poll_timeout(&self, timeout: Duration) -> Option<WatchEvent> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Stop watching. After this call, no more events will be received.
    /// The watcher is also stopped automatically on drop.
    pub fn stop(self) {
        // Dropping `self._watcher` stops the OS-level watch.
        drop(self);
    }
}

// ── Event translation ─────────────────────────────────────────────────

/// Translate a `notify::Event` into zero or more `WatchEvent`s.
fn translate_event(event: &Event) -> Vec<WatchEvent> {
    let mut result = Vec::new();

    match &event.kind {
        EventKind::Create(_) => {
            for path in &event.paths {
                result.push(WatchEvent::Created(path.clone()));
            }
        }
        EventKind::Modify(_) => {
            for path in &event.paths {
                result.push(WatchEvent::Modified(path.clone()));
            }
        }
        EventKind::Remove(_) => {
            for path in &event.paths {
                result.push(WatchEvent::Removed(path.clone()));
            }
        }
        _ => {
            // Access, Other, etc. — ignored
        }
    }

    result
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn translate_create_event() {
        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from("/tmp/test.txt")],
            attrs: notify::event::EventAttributes::default(),
        };
        let events = translate_event(&event);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            WatchEvent::Created(PathBuf::from("/tmp/test.txt"))
        );
    }

    #[test]
    fn translate_modify_event() {
        let event = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![PathBuf::from("/tmp/test.txt")],
            attrs: notify::event::EventAttributes::default(),
        };
        let events = translate_event(&event);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            WatchEvent::Modified(PathBuf::from("/tmp/test.txt"))
        );
    }

    #[test]
    fn translate_remove_event() {
        let event = Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![PathBuf::from("/tmp/test.txt")],
            attrs: notify::event::EventAttributes::default(),
        };
        let events = translate_event(&event);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            WatchEvent::Removed(PathBuf::from("/tmp/test.txt"))
        );
    }

    #[test]
    fn translate_other_event_ignored() {
        let event = Event {
            kind: EventKind::Other,
            paths: vec![PathBuf::from("/tmp/test.txt")],
            attrs: notify::event::EventAttributes::default(),
        };
        let events = translate_event(&event);
        assert!(events.is_empty());
    }

    #[test]
    fn translate_multi_path_event() {
        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")],
            attrs: notify::event::EventAttributes::default(),
        };
        let events = translate_event(&event);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn watcher_creates_and_detects_file() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::new(dir.path()).unwrap();
        assert_eq!(watcher.root(), dir.path());

        // Create a file — should trigger an event
        let file_path = dir.path().join("new_file.txt");
        fs::write(&file_path, "hello").unwrap();

        // Give the OS a moment to deliver the event
        std::thread::sleep(Duration::from_millis(200));

        let events = watcher.poll();
        // We should have at least one Created or Modified event
        assert!(
            !events.is_empty(),
            "Expected at least one event after file creation"
        );
    }

    #[test]
    fn watcher_detects_modification() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "initial").unwrap();

        // Small delay to avoid conflating create with modify
        std::thread::sleep(Duration::from_millis(100));

        let watcher = FileWatcher::new(dir.path()).unwrap();

        // Modify the file
        fs::write(&file_path, "modified").unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let events = watcher.poll();
        assert!(!events.is_empty());
    }

    #[test]
    fn watcher_detects_removal() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("to_delete.txt");
        fs::write(&file_path, "bye").unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let watcher = FileWatcher::new(dir.path()).unwrap();

        fs::remove_file(&file_path).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let events = watcher.poll();
        assert!(!events.is_empty());
    }

    #[test]
    fn poll_timeout_returns_none_when_no_events() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileWatcher::new(dir.path()).unwrap();
        let result = watcher.poll_timeout(Duration::from_millis(50));
        assert!(result.is_none());
    }

    #[test]
    fn watcher_nonexistent_path_errors() {
        let result = FileWatcher::new(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_err());
    }

    #[test]
    fn watch_event_equality() {
        let a = WatchEvent::Created(PathBuf::from("/a"));
        let b = WatchEvent::Created(PathBuf::from("/a"));
        assert_eq!(a, b);

        let c = WatchEvent::Modified(PathBuf::from("/a"));
        assert_ne!(a, c);
    }

    #[test]
    fn watch_event_renamed_variant() {
        let e = WatchEvent::Renamed {
            from: PathBuf::from("/a"),
            to: PathBuf::from("/b"),
        };
        assert!(matches!(e, WatchEvent::Renamed { .. }));
    }

    // ── WatchEvent::path ────────────────────────────────────────

    #[test]
    fn watch_event_path() {
        let c = WatchEvent::Created(PathBuf::from("/a"));
        assert_eq!(c.path(), Path::new("/a"));

        let m = WatchEvent::Modified(PathBuf::from("/b"));
        assert_eq!(m.path(), Path::new("/b"));

        let r = WatchEvent::Removed(PathBuf::from("/c"));
        assert_eq!(r.path(), Path::new("/c"));

        let rn = WatchEvent::Renamed {
            from: PathBuf::from("/old"),
            to: PathBuf::from("/new"),
        };
        assert_eq!(rn.path(), Path::new("/new"));
    }

    // ── WatchConfig ─────────────────────────────────────────────

    #[test]
    fn watch_config_defaults() {
        let cfg = WatchConfig::default();
        assert_eq!(cfg.debounce_ms, 100);
        assert_eq!(cfg.batch_window_ms, 50);
        assert!(cfg.ignore_patterns.contains(&".git".to_owned()));
        assert!(cfg.ignore_patterns.contains(&"target".to_owned()));
        assert!(cfg.ignore_patterns.contains(&"node_modules".to_owned()));
    }

    #[test]
    fn watch_config_serializes() {
        let cfg = WatchConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("debounce_ms"));
        assert!(json.contains("ignore_patterns"));
        assert!(json.contains("batch_window_ms"));
    }

    // ── EventFilter ─────────────────────────────────────────────

    #[test]
    fn filter_ignores_git() {
        let filter = EventFilter::with_defaults();
        let e = WatchEvent::Modified(PathBuf::from("/project/.git/HEAD"));
        assert!(!filter.should_keep(&e));
    }

    #[test]
    fn filter_ignores_target() {
        let filter = EventFilter::with_defaults();
        let e = WatchEvent::Created(PathBuf::from("/project/target/debug/main"));
        assert!(!filter.should_keep(&e));
    }

    #[test]
    fn filter_ignores_node_modules() {
        let filter = EventFilter::with_defaults();
        let e = WatchEvent::Modified(PathBuf::from("/project/node_modules/pkg/index.js"));
        assert!(!filter.should_keep(&e));
    }

    #[test]
    fn filter_keeps_normal_files() {
        let filter = EventFilter::with_defaults();
        let e = WatchEvent::Modified(PathBuf::from("/project/src/main.rs"));
        assert!(filter.should_keep(&e));
    }

    #[test]
    fn filter_custom_pattern() {
        let filter = EventFilter::new(vec!["vendor".to_owned()]);
        let e1 = WatchEvent::Modified(PathBuf::from("/project/vendor/lib.rs"));
        let e2 = WatchEvent::Modified(PathBuf::from("/project/src/lib.rs"));
        assert!(!filter.should_keep(&e1));
        assert!(filter.should_keep(&e2));
    }

    #[test]
    fn filter_batch() {
        let filter = EventFilter::with_defaults();
        let events = vec![
            WatchEvent::Modified(PathBuf::from("/project/src/main.rs")),
            WatchEvent::Modified(PathBuf::from("/project/.git/index")),
            WatchEvent::Created(PathBuf::from("/project/Cargo.toml")),
        ];
        let kept = filter.filter_events(events);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn filter_is_ignored() {
        let filter = EventFilter::with_defaults();
        assert!(filter.is_ignored(Path::new("/project/.git/HEAD")));
        assert!(!filter.is_ignored(Path::new("/project/src/lib.rs")));
    }

    // ── EventDebouncer ──────────────────────────────────────────

    #[test]
    fn debouncer_deduplicates_same_path() {
        let mut db = EventDebouncer::new(Duration::from_millis(0));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));
        assert_eq!(db.pending_count(), 1);
    }

    #[test]
    fn debouncer_keeps_last_event() {
        let mut db = EventDebouncer::new(Duration::from_millis(0));
        db.push(WatchEvent::Created(PathBuf::from("/a.rs")));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));

        // With 0ms debounce, drain_ready may or may not fire immediately.
        // Use drain_all for deterministic test.
        let events = db.drain_all();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], WatchEvent::Modified(_)));
    }

    #[test]
    fn debouncer_push_many() {
        let mut db = EventDebouncer::new(Duration::from_millis(0));
        db.push_many(vec![
            WatchEvent::Modified(PathBuf::from("/a.rs")),
            WatchEvent::Modified(PathBuf::from("/b.rs")),
        ]);
        assert_eq!(db.pending_count(), 2);
    }

    #[test]
    fn debouncer_drain_all_clears() {
        let mut db = EventDebouncer::new(Duration::from_secs(100));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));
        let events = db.drain_all();
        assert_eq!(events.len(), 1);
        assert_eq!(db.pending_count(), 0);
    }

    #[test]
    fn debouncer_drain_ready_respects_window() {
        let mut db = EventDebouncer::new(Duration::from_secs(100));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));
        // Should not drain yet (100s window)
        let events = db.drain_ready();
        assert!(events.is_empty());
        assert_eq!(db.pending_count(), 1);
    }

    #[test]
    fn debouncer_drain_ready_fires_after_window() {
        let mut db = EventDebouncer::new(Duration::from_millis(0));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));
        // 0ms debounce — sleep a tiny bit to ensure elapsed
        std::thread::sleep(Duration::from_millis(1));
        let events = db.drain_ready();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn debouncer_sorted_output() {
        let mut db = EventDebouncer::new(Duration::from_millis(0));
        db.push(WatchEvent::Modified(PathBuf::from("/z.rs")));
        db.push(WatchEvent::Modified(PathBuf::from("/a.rs")));
        let events = db.drain_all();
        assert_eq!(events[0].path(), Path::new("/a.rs"));
        assert_eq!(events[1].path(), Path::new("/z.rs"));
    }

    // ── BatchAggregator ─────────────────────────────────────────

    #[test]
    fn batch_empty_flush() {
        let mut agg = BatchAggregator::new(Duration::from_millis(50));
        let batch = agg.flush();
        assert!(batch.is_empty());
    }

    #[test]
    fn batch_add_and_flush() {
        let mut agg = BatchAggregator::new(Duration::from_millis(50));
        agg.add(vec![WatchEvent::Modified(PathBuf::from("/a.rs"))]);
        assert_eq!(agg.buffered_count(), 1);
        let batch = agg.flush();
        assert_eq!(batch.len(), 1);
        assert_eq!(agg.buffered_count(), 0);
    }

    #[test]
    fn batch_try_flush_before_window() {
        let mut agg = BatchAggregator::new(Duration::from_secs(100));
        agg.add(vec![WatchEvent::Modified(PathBuf::from("/a.rs"))]);
        let batch = agg.try_flush();
        assert!(batch.is_none());
    }

    #[test]
    fn batch_try_flush_after_window() {
        let mut agg = BatchAggregator::new(Duration::from_millis(0));
        agg.add(vec![WatchEvent::Modified(PathBuf::from("/a.rs"))]);
        std::thread::sleep(Duration::from_millis(1));
        let batch = agg.try_flush();
        assert!(batch.is_some());
        assert_eq!(batch.unwrap().len(), 1);
    }

    #[test]
    fn batch_accumulates_multiple() {
        let mut agg = BatchAggregator::new(Duration::from_secs(100));
        agg.add(vec![WatchEvent::Modified(PathBuf::from("/a.rs"))]);
        agg.add(vec![WatchEvent::Created(PathBuf::from("/b.rs"))]);
        assert_eq!(agg.buffered_count(), 2);
        let batch = agg.flush();
        assert_eq!(batch.len(), 2);
    }

    #[test]
    fn batch_try_flush_empty_returns_none() {
        let mut agg = BatchAggregator::new(Duration::from_millis(0));
        // No events added
        let batch = agg.try_flush();
        assert!(batch.is_none());
    }
}
