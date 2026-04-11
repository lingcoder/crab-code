//! Event broker — pause/resume crossterm event reading.
//!
//! Required for external editor integration (Ctrl+G): we must exit raw mode
//! and stop reading terminal events while `$EDITOR` runs, then resume.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Controls whether the TUI event loop reads terminal events.
///
/// When paused, the crossterm `EventStream` should be dropped (or its task
/// suspended) so the external process can use the terminal.
#[derive(Clone)]
pub struct EventBroker {
    paused: Arc<AtomicBool>,
}

impl EventBroker {
    /// Create a new event broker (initially active).
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Pause event reading (before launching external editor).
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume event reading (after external editor exits).
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Whether event reading is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

impl Default for EventBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_broker_starts_active() {
        let broker = EventBroker::new();
        assert!(!broker.is_paused());
    }

    #[test]
    fn event_broker_pause_resume() {
        let broker = EventBroker::new();
        broker.pause();
        assert!(broker.is_paused());
        broker.resume();
        assert!(!broker.is_paused());
    }

    #[test]
    fn event_broker_clone_shares_state() {
        let broker = EventBroker::new();
        let clone = broker.clone();
        broker.pause();
        assert!(clone.is_paused());
    }
}
