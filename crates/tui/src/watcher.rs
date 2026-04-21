//! Filesystem watcher for settings and skills hot-reload.
//!
//! Watches `~/.crab/settings.json`, project `.crab/settings.json`, and skill
//! directories for changes. On modification, sends a notification through the
//! provided channel so the TUI can react (reload config, rediscover skills, etc.).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};
use tokio::sync::mpsc;

/// Events emitted by the filesystem watcher.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// A settings file was modified.
    SettingsChanged,
    /// A skill directory contents changed (add/remove/modify).
    SkillsChanged,
}

/// Watches filesystem paths and sends `WatchEvent` notifications.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    /// Create a new watcher monitoring the given settings files and skill directories.
    ///
    /// Returns `None` if the watcher fails to initialize (e.g. on platforms
    /// without inotify/kqueue support, or when all paths are invalid).
    pub fn new(
        settings_paths: &[PathBuf],
        skill_dirs: &[PathBuf],
        tx: mpsc::UnboundedSender<WatchEvent>,
    ) -> Option<Self> {
        let skill_dirs_set: Arc<Vec<PathBuf>> = Arc::new(skill_dirs.to_vec());
        let skill_dirs_for_handler = Arc::clone(&skill_dirs_set);

        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else { return };
                if !matches!(
                    event.kind,
                    notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
                        | notify::EventKind::Create(_)
                        | notify::EventKind::Remove(_)
                ) {
                    return;
                }

                let is_skill = event
                    .paths
                    .iter()
                    .any(|p| skill_dirs_for_handler.iter().any(|d| p.starts_with(d)));

                let watch_event = if is_skill {
                    WatchEvent::SkillsChanged
                } else {
                    WatchEvent::SettingsChanged
                };
                let _ = tx.send(watch_event);
            })
            .ok()?;

        for path in settings_paths {
            if path.exists() {
                let _ = watcher.watch(path, RecursiveMode::NonRecursive);
            }
        }
        for dir in &*skill_dirs_set {
            if dir.exists() {
                let _ = watcher.watch(dir, RecursiveMode::Recursive);
            }
        }

        Some(Self { _watcher: watcher })
    }
}

/// Debounce watch events — collapses rapid-fire changes into a single event.
///
/// Returns a receiver that emits at most one event per `debounce` window.
pub fn debounced_watch(
    mut raw_rx: mpsc::UnboundedReceiver<WatchEvent>,
    debounce: Duration,
) -> mpsc::UnboundedReceiver<WatchEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let Some(event) = raw_rx.recv().await else {
                break;
            };
            // Drain any additional events within the debounce window
            tokio::time::sleep(debounce).await;
            while raw_rx.try_recv().is_ok() {}
            let _ = tx.send(event);
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<WatchEvent>();
    }

    #[test]
    fn watch_event_clone() {
        let e = WatchEvent::SettingsChanged;
        let _ = e.clone();
    }

    #[test]
    fn file_watcher_with_nonexistent_paths() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let watcher = FileWatcher::new(
            &[PathBuf::from("/nonexistent/settings.json")],
            &[PathBuf::from("/nonexistent/skills/")],
            tx,
        );
        // Should succeed even with non-existent paths (just watches nothing)
        assert!(watcher.is_some());
    }

    #[tokio::test]
    async fn debounced_watch_collapses_events() {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let mut debounced_rx = debounced_watch(raw_rx, Duration::from_millis(50));

        // Send multiple events rapidly
        raw_tx.send(WatchEvent::SettingsChanged).unwrap();
        raw_tx.send(WatchEvent::SettingsChanged).unwrap();
        raw_tx.send(WatchEvent::SettingsChanged).unwrap();

        // Should get exactly one event after debounce
        let event = tokio::time::timeout(Duration::from_millis(200), debounced_rx.recv()).await;
        assert!(event.is_ok());

        // No more events pending
        let no_event = tokio::time::timeout(Duration::from_millis(100), debounced_rx.recv()).await;
        assert!(no_event.is_err());
    }
}
