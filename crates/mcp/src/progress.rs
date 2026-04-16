//! MCP progress reporting.
//!
//! Provides [`ProgressToken`] for identifying long-running operations,
//! [`ProgressNotification`] for reporting progress updates,
//! [`ProgressTracker`] for tracking progress with percentage and ETA,
//! and [`ProgressCallback`] for push-based progress delivery.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Token identifying a long-running operation for progress tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    String(String),
    Number(i64),
}

impl ProgressToken {
    #[must_use]
    pub fn from_string(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }

    #[must_use]
    pub fn from_number(n: i64) -> Self {
        Self::Number(n)
    }
}

impl std::fmt::Display for ProgressToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(s) => write!(f, "{s}"),
            Self::Number(n) => write!(f, "{n}"),
        }
    }
}

/// A progress notification as defined by the MCP protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressNotification {
    pub progress_token: ProgressToken,
    /// Current progress value.
    pub progress: f64,
    /// Optional total value (if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// Optional human-readable message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ProgressNotification {
    #[must_use]
    pub fn new(token: ProgressToken, progress: f64) -> Self {
        Self {
            progress_token: token,
            progress,
            total: None,
            message: None,
        }
    }

    #[must_use]
    pub fn with_total(mut self, total: f64) -> Self {
        self.total = Some(total);
        self
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Calculate percentage if total is known.
    #[must_use]
    pub fn percentage(&self) -> Option<f64> {
        self.total.map(|t| {
            if t <= 0.0 {
                0.0
            } else {
                (self.progress / t * 100.0).min(100.0)
            }
        })
    }
}

/// Callback for receiving progress updates.
pub trait ProgressCallback: Send + Sync {
    fn on_progress(&self, notification: &ProgressNotification);
}

/// A callback that records progress notifications for testing.
pub struct RecordingProgressCallback {
    recorded: Arc<Mutex<Vec<ProgressNotification>>>,
}

impl RecordingProgressCallback {
    #[must_use]
    pub fn new() -> Self {
        Self {
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[must_use]
    pub fn recorded(&self) -> Vec<ProgressNotification> {
        self.recorded.lock().unwrap().clone()
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.recorded.lock().unwrap().len()
    }
}

impl Default for RecordingProgressCallback {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressCallback for RecordingProgressCallback {
    fn on_progress(&self, notification: &ProgressNotification) {
        self.recorded.lock().unwrap().push(notification.clone());
    }
}

/// Tracks progress of a long-running operation with ETA estimation.
pub struct ProgressTracker {
    token: ProgressToken,
    total: Option<f64>,
    current: f64,
    started_at: Instant,
    callbacks: Vec<Arc<dyn ProgressCallback>>,
    message: Option<String>,
}

impl ProgressTracker {
    /// Create a tracker for an operation with unknown total.
    #[must_use]
    pub fn new(token: ProgressToken) -> Self {
        Self {
            token,
            total: None,
            current: 0.0,
            started_at: Instant::now(),
            callbacks: Vec::new(),
            message: None,
        }
    }

    /// Create a tracker with a known total.
    #[must_use]
    pub fn with_total(token: ProgressToken, total: f64) -> Self {
        Self {
            token,
            total: Some(total),
            current: 0.0,
            started_at: Instant::now(),
            callbacks: Vec::new(),
            message: None,
        }
    }

    /// Register a callback for progress updates.
    pub fn add_callback(&mut self, callback: Arc<dyn ProgressCallback>) {
        self.callbacks.push(callback);
    }

    /// Set the current message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// Update progress and notify callbacks.
    pub fn update(&mut self, progress: f64) {
        self.current = progress;
        let notification = self.build_notification();
        for cb in &self.callbacks {
            cb.on_progress(&notification);
        }
    }

    /// Increment progress by a delta and notify callbacks.
    pub fn increment(&mut self, delta: f64) {
        self.update(self.current + delta);
    }

    /// Mark the operation as complete.
    pub fn complete(&mut self) {
        if let Some(total) = self.total {
            self.update(total);
        } else {
            self.update(self.current);
        }
    }

    /// Current progress value.
    #[must_use]
    pub fn current(&self) -> f64 {
        self.current
    }

    /// Total value, if known.
    #[must_use]
    pub fn total(&self) -> Option<f64> {
        self.total
    }

    /// Calculate percentage completion (0-100), or None if total is unknown.
    #[must_use]
    pub fn percentage(&self) -> Option<f64> {
        self.total.map(|t| {
            if t <= 0.0 {
                0.0
            } else {
                (self.current / t * 100.0).min(100.0)
            }
        })
    }

    /// Estimate remaining duration based on elapsed time and progress.
    /// Returns None if total is unknown or no progress has been made.
    #[must_use]
    pub fn estimated_remaining(&self) -> Option<std::time::Duration> {
        let total = self.total?;
        if self.current <= 0.0 || total <= 0.0 {
            return None;
        }
        let elapsed = self.started_at.elapsed();
        let rate = self.current / elapsed.as_secs_f64();
        let remaining = (total - self.current) / rate;
        if remaining <= 0.0 {
            return Some(std::time::Duration::ZERO);
        }
        Some(std::time::Duration::from_secs_f64(remaining))
    }

    /// Whether the operation is complete (progress >= total).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.total.is_some_and(|t| self.current >= t)
    }

    /// Elapsed time since the tracker was created.
    #[must_use]
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    fn build_notification(&self) -> ProgressNotification {
        let mut n = ProgressNotification::new(self.token.clone(), self.current);
        n.total = self.total;
        n.message.clone_from(&self.message);
        n
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // progress values are f64 but tests use exact integer-equivalent values
mod tests {
    use super::*;

    #[test]
    fn progress_token_string() {
        let t = ProgressToken::from_string("op-1");
        assert_eq!(t.to_string(), "op-1");
    }

    #[test]
    fn progress_token_number() {
        let t = ProgressToken::from_number(42);
        assert_eq!(t.to_string(), "42");
    }

    #[test]
    fn progress_token_serde_roundtrip() {
        let ts = ProgressToken::from_string("abc");
        let json = serde_json::to_string(&ts).unwrap();
        let back: ProgressToken = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ts);

        let tn = ProgressToken::from_number(99);
        let json = serde_json::to_string(&tn).unwrap();
        let back: ProgressToken = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tn);
    }

    #[test]
    fn notification_percentage() {
        let n = ProgressNotification::new(ProgressToken::from_number(1), 30.0).with_total(100.0);
        assert_eq!(n.percentage(), Some(30.0));
    }

    #[test]
    fn notification_percentage_none_without_total() {
        let n = ProgressNotification::new(ProgressToken::from_number(1), 30.0);
        assert_eq!(n.percentage(), None);
    }

    #[test]
    fn notification_percentage_zero_total() {
        let n = ProgressNotification::new(ProgressToken::from_number(1), 10.0).with_total(0.0);
        assert_eq!(n.percentage(), Some(0.0));
    }

    #[test]
    fn notification_serde_roundtrip() {
        let n = ProgressNotification::new(ProgressToken::from_string("t"), 5.0)
            .with_total(10.0)
            .with_message("halfway");
        let json = serde_json::to_string(&n).unwrap();
        let back: ProgressNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(back.progress_token, ProgressToken::from_string("t"));
        assert_eq!(back.progress, 5.0);
        assert_eq!(back.total, Some(10.0));
        assert_eq!(back.message.as_deref(), Some("halfway"));
    }

    #[test]
    fn tracker_with_total() {
        let mut tracker = ProgressTracker::with_total(ProgressToken::from_number(1), 100.0);
        assert_eq!(tracker.current(), 0.0);
        assert_eq!(tracker.total(), Some(100.0));
        assert!(!tracker.is_complete());

        tracker.update(50.0);
        assert_eq!(tracker.percentage(), Some(50.0));

        tracker.update(100.0);
        assert!(tracker.is_complete());
    }

    #[test]
    fn tracker_increment() {
        let mut tracker = ProgressTracker::with_total(ProgressToken::from_number(1), 10.0);
        tracker.increment(3.0);
        tracker.increment(2.0);
        assert_eq!(tracker.current(), 5.0);
        assert_eq!(tracker.percentage(), Some(50.0));
    }

    #[test]
    fn tracker_complete() {
        let mut tracker = ProgressTracker::with_total(ProgressToken::from_number(1), 10.0);
        tracker.update(5.0);
        tracker.complete();
        assert!(tracker.is_complete());
        assert_eq!(tracker.current(), 10.0);
    }

    #[test]
    fn tracker_callbacks() {
        let cb = Arc::new(RecordingProgressCallback::new());
        let mut tracker = ProgressTracker::with_total(ProgressToken::from_number(1), 100.0);
        tracker.add_callback(Arc::clone(&cb) as Arc<dyn ProgressCallback>);

        tracker.update(25.0);
        tracker.update(50.0);
        tracker.update(100.0);

        assert_eq!(cb.count(), 3);
        let recorded = cb.recorded();
        assert_eq!(recorded[0].progress, 25.0);
        assert_eq!(recorded[1].progress, 50.0);
        assert_eq!(recorded[2].progress, 100.0);
    }

    #[test]
    fn tracker_eta_estimation() {
        let mut tracker = ProgressTracker::with_total(ProgressToken::from_number(1), 100.0);
        // No progress yet — no ETA
        assert!(tracker.estimated_remaining().is_none());

        tracker.update(50.0);
        // After some progress, ETA should be available
        let eta = tracker.estimated_remaining();
        assert!(eta.is_some());
    }

    #[test]
    fn tracker_no_total() {
        let mut tracker = ProgressTracker::new(ProgressToken::from_string("op"));
        tracker.update(10.0);
        assert_eq!(tracker.percentage(), None);
        assert!(tracker.estimated_remaining().is_none());
        assert!(!tracker.is_complete());
    }

    #[test]
    fn tracker_elapsed() {
        let tracker = ProgressTracker::new(ProgressToken::from_number(1));
        // Elapsed should be non-negative (just created)
        // elapsed() returns a Duration which is always non-negative
        let _ = tracker.elapsed();
    }

    #[test]
    fn recording_callback_default() {
        let cb = RecordingProgressCallback::default();
        assert_eq!(cb.count(), 0);
    }
}
