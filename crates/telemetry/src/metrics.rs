//! Structured metrics and performance tracing spans.
//!
//! Provides typed span helpers for tool execution, API calls, and agent loops.
//! All data stays local — never sent to external services.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── Structured log event ──────────────────────────────────────────────

/// A structured log event that can be serialized to JSON.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredEvent {
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// Log level.
    pub level: LogLevel,
    /// Event category.
    pub category: String,
    /// Human-readable message.
    pub message: String,
    /// Structured key-value fields.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub fields: HashMap<String, serde_json::Value>,
}

/// Log level for structured events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trace => f.write_str("trace"),
            Self::Debug => f.write_str("debug"),
            Self::Info => f.write_str("info"),
            Self::Warn => f.write_str("warn"),
            Self::Error => f.write_str("error"),
        }
    }
}

impl StructuredEvent {
    /// Create a new structured event.
    #[must_use]
    pub fn new(level: LogLevel, category: &str, message: &str) -> Self {
        Self {
            timestamp: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            level,
            category: category.to_string(),
            message: message.to_string(),
            fields: HashMap::new(),
        }
    }

    /// Add a field to the event.
    #[must_use]
    pub fn with_field(mut self, key: &str, value: serde_json::Value) -> Self {
        self.fields.insert(key.to_string(), value);
        self
    }

    /// Serialize to JSON string.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| format!("{self:?}"))
    }
}

// ── Performance span timing ───────────────────────────────────────────

/// Process-wide monotonically increasing span ID counter.
static SPAN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_span_id() -> String {
    let n = SPAN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{n:016x}")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A completed timing measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanTiming {
    /// Span name (e.g. `"tool.bash"`, `"api.anthropic"`).
    pub name: String,
    /// Unique span identifier (hex-encoded counter).
    pub span_id: String,
    /// Start timestamp (milliseconds since Unix epoch).
    pub start_time_ms: u64,
    /// Duration in milliseconds.
    pub duration_ms: f64,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Optional metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// An active span that measures elapsed time until dropped or finished.
pub struct ActiveSpan {
    name: String,
    span_id: String,
    start: Instant,
    start_time_ms: u64,
    metadata: HashMap<String, serde_json::Value>,
    finished: bool,
}

impl ActiveSpan {
    /// Start a new timing span.
    #[must_use]
    pub fn start(name: &str) -> Self {
        Self {
            name: name.to_string(),
            span_id: next_span_id(),
            start: Instant::now(),
            start_time_ms: now_unix_ms(),
            metadata: HashMap::new(),
            finished: false,
        }
    }

    /// Span identifier.
    #[allow(dead_code)]
    #[must_use]
    pub fn span_id(&self) -> &str {
        &self.span_id
    }

    /// Start time in milliseconds since Unix epoch.
    #[allow(dead_code)]
    #[must_use]
    pub fn start_time_ms(&self) -> u64 {
        self.start_time_ms
    }

    /// Add metadata to the span.
    pub fn add_metadata(&mut self, key: &str, value: serde_json::Value) {
        self.metadata.insert(key.to_string(), value);
    }

    /// Finish the span and return the timing.
    #[must_use]
    pub fn finish(mut self, success: bool) -> SpanTiming {
        self.finished = true;
        SpanTiming {
            name: self.name.clone(),
            span_id: self.span_id.clone(),
            start_time_ms: self.start_time_ms,
            duration_ms: self.start.elapsed().as_secs_f64() * 1000.0,
            success,
            metadata: self.metadata.clone(),
        }
    }

    /// Get elapsed time so far without finishing the span.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Drop for ActiveSpan {
    fn drop(&mut self) {
        if !self.finished {
            // If dropped without finish(), log a warning via tracing
            tracing::warn!(
                span_name = %self.name,
                elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0,
                "ActiveSpan dropped without finish()"
            );
        }
    }
}

// ── Metrics collector ─────────────────────────────────────────────────

/// Collects span timings and provides aggregate statistics.
/// Thread-safe via internal `Mutex`.
pub struct MetricsCollector {
    timings: Mutex<Vec<SpanTiming>>,
}

impl std::fmt::Debug for MetricsCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.timings.lock().map_or(0, |t| t.len());
        f.debug_struct("MetricsCollector")
            .field("timings_count", &count)
            .finish_non_exhaustive()
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector {
    /// Create a new empty collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            timings: Mutex::new(Vec::new()),
        }
    }

    /// Record a completed span timing.
    pub fn record(&self, timing: SpanTiming) {
        if let Ok(mut timings) = self.timings.lock() {
            timings.push(timing);
        }
    }

    /// Get all recorded timings.
    #[must_use]
    pub fn timings(&self) -> Vec<SpanTiming> {
        self.timings
            .lock()
            .map_or_else(|_| Vec::new(), |t| t.clone())
    }

    /// Get timings filtered by name prefix.
    #[must_use]
    pub fn timings_by_prefix(&self, prefix: &str) -> Vec<SpanTiming> {
        self.timings.lock().map_or_else(
            |_| Vec::new(),
            |t| {
                t.iter()
                    .filter(|s| s.name.starts_with(prefix))
                    .cloned()
                    .collect()
            },
        )
    }

    /// Get aggregate stats for a span name prefix.
    #[must_use]
    pub fn stats(&self, prefix: &str) -> Option<SpanStats> {
        let timings = self.timings_by_prefix(prefix);
        if timings.is_empty() {
            return None;
        }
        let count = timings.len() as u64;
        let success_count = timings.iter().filter(|t| t.success).count() as u64;
        let durations: Vec<f64> = timings.iter().map(|t| t.duration_ms).collect();
        let total: f64 = durations.iter().sum();
        let min = durations.iter().copied().fold(f64::INFINITY, f64::min);
        let max = durations.iter().copied().fold(f64::NEG_INFINITY, f64::max);

        Some(SpanStats {
            count,
            success_count,
            #[allow(clippy::cast_precision_loss)]
            avg_ms: total / count as f64,
            min_ms: min,
            max_ms: max,
            total_ms: total,
        })
    }

    /// Number of recorded timings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.timings.lock().map_or(0, |t| t.len())
    }

    /// Whether there are no recorded timings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all timings.
    pub fn clear(&self) {
        if let Ok(mut timings) = self.timings.lock() {
            timings.clear();
        }
    }
}

/// Aggregate statistics for a set of spans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanStats {
    pub count: u64,
    pub success_count: u64,
    pub avg_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub total_ms: f64,
}

// ── Convenience span starters ─────────────────────────────────────────

/// Start a tool execution span.
#[must_use]
pub fn tool_span(tool_name: &str) -> ActiveSpan {
    let mut span = ActiveSpan::start(&format!("tool.{tool_name}"));
    span.add_metadata("tool", serde_json::Value::String(tool_name.to_string()));
    span
}

/// Start an API call span.
#[must_use]
pub fn api_span(provider: &str, model: &str) -> ActiveSpan {
    let name = format!("api.{provider}");
    let mut span = ActiveSpan::start(&name);
    span.add_metadata("provider", serde_json::Value::String(provider.to_string()));
    span.add_metadata("model", serde_json::Value::String(model.to_string()));
    span
}

/// Start an agent loop iteration span.
#[must_use]
pub fn agent_loop_span(iteration: u32) -> ActiveSpan {
    let mut span = ActiveSpan::start("agent.loop");
    span.add_metadata("iteration", serde_json::Value::Number(iteration.into()));
    span
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── LogLevel ──────────────────────────────────────────────────────

    #[test]
    fn log_level_display() {
        assert_eq!(LogLevel::Info.to_string(), "info");
        assert_eq!(LogLevel::Error.to_string(), "error");
    }

    #[test]
    fn log_level_serde_roundtrip() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            let json = serde_json::to_string(&level).unwrap();
            let parsed: LogLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, parsed);
        }
    }

    // ── StructuredEvent ───────────────────────────────────────────────

    #[test]
    fn structured_event_new() {
        let event = StructuredEvent::new(LogLevel::Info, "tool", "bash executed");
        assert_eq!(event.level, LogLevel::Info);
        assert_eq!(event.category, "tool");
        assert_eq!(event.message, "bash executed");
        assert!(event.fields.is_empty());
        // chrono ISO 8601 timestamp: YYYY-MM-DDTHH:MM:SSZ → 20 chars
        assert_eq!(event.timestamp.len(), 20);
        assert!(event.timestamp.ends_with('Z'));
    }

    #[test]
    fn structured_event_with_fields() {
        let event = StructuredEvent::new(LogLevel::Debug, "api", "request sent")
            .with_field("provider", serde_json::json!("anthropic"))
            .with_field("tokens", serde_json::json!(1500));
        assert_eq!(event.fields.len(), 2);
        assert_eq!(event.fields["provider"], serde_json::json!("anthropic"));
    }

    #[test]
    fn structured_event_to_json() {
        let event = StructuredEvent::new(LogLevel::Warn, "system", "high memory");
        let json = event.to_json();
        assert!(json.contains("\"level\":\"warn\""));
        assert!(json.contains("\"category\":\"system\""));
    }

    #[test]
    fn structured_event_serde_roundtrip() {
        let event = StructuredEvent::new(LogLevel::Error, "tool", "failed")
            .with_field("error", serde_json::json!("timeout"));
        let json = serde_json::to_string(&event).unwrap();
        let parsed: StructuredEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn structured_event_empty_fields_skipped() {
        let event = StructuredEvent::new(LogLevel::Info, "test", "msg");
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("fields"));
    }

    // ── ActiveSpan ────────────────────────────────────────────────────

    #[test]
    fn active_span_finish_returns_timing() {
        let span = ActiveSpan::start("test.span");
        std::thread::sleep(Duration::from_millis(5));
        let timing = span.finish(true);

        assert_eq!(timing.name, "test.span");
        assert!(timing.success);
        assert!(timing.duration_ms >= 1.0); // at least 1ms
        assert!(!timing.span_id.is_empty());
        assert!(timing.start_time_ms > 0);
    }

    #[test]
    fn active_span_with_metadata() {
        let mut span = ActiveSpan::start("test");
        span.add_metadata("key", serde_json::json!("value"));
        let timing = span.finish(false);

        assert!(!timing.success);
        assert_eq!(timing.metadata["key"], serde_json::json!("value"));
    }

    #[test]
    fn active_span_elapsed() {
        let span = ActiveSpan::start("test");
        std::thread::sleep(Duration::from_millis(5));
        assert!(span.elapsed() >= Duration::from_millis(1));
        let _ = span.finish(true);
    }

    #[test]
    fn active_span_unique_ids() {
        let a = ActiveSpan::start("a").finish(true);
        let b = ActiveSpan::start("b").finish(true);
        assert_ne!(a.span_id, b.span_id);
        // Hex-encoded counter is 16 chars wide.
        assert_eq!(a.span_id.len(), 16);
    }

    // ── SpanTiming serde ──────────────────────────────────────────────

    #[test]
    fn span_timing_serde_roundtrip() {
        let timing = SpanTiming {
            name: "tool.bash".to_string(),
            span_id: "0000000000000001".to_string(),
            start_time_ms: 1_700_000_000_000,
            duration_ms: 42.5,
            success: true,
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&timing).unwrap();
        let parsed: SpanTiming = serde_json::from_str(&json).unwrap();
        assert_eq!(timing, parsed);
    }

    // ── MetricsCollector ──────────────────────────────────────────────

    fn timing(name: &str, duration_ms: f64, success: bool) -> SpanTiming {
        SpanTiming {
            name: name.to_string(),
            span_id: next_span_id(),
            start_time_ms: now_unix_ms(),
            duration_ms,
            success,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn collector_record_and_query() {
        let collector = MetricsCollector::new();
        collector.record(timing("tool.bash", 100.0, true));
        collector.record(timing("tool.read", 5.0, true));
        collector.record(timing("api.anthropic", 500.0, false));

        assert_eq!(collector.len(), 3);
        assert_eq!(collector.timings_by_prefix("tool.").len(), 2);
        assert_eq!(collector.timings_by_prefix("api.").len(), 1);
    }

    #[test]
    fn collector_stats() {
        let collector = MetricsCollector::new();
        collector.record(timing("tool.bash", 100.0, true));
        collector.record(timing("tool.bash", 200.0, true));
        collector.record(timing("tool.bash", 300.0, false));

        let stats = collector.stats("tool.bash").unwrap();
        assert_eq!(stats.count, 3);
        assert_eq!(stats.success_count, 2);
        assert!((stats.avg_ms - 200.0).abs() < 1e-10);
        assert!((stats.min_ms - 100.0).abs() < 1e-10);
        assert!((stats.max_ms - 300.0).abs() < 1e-10);
        assert!((stats.total_ms - 600.0).abs() < 1e-10);
    }

    #[test]
    fn collector_stats_empty() {
        let collector = MetricsCollector::new();
        assert!(collector.stats("nonexistent").is_none());
    }

    #[test]
    fn collector_clear() {
        let collector = MetricsCollector::new();
        collector.record(timing("test", 1.0, true));
        assert!(!collector.is_empty());
        collector.clear();
        assert!(collector.is_empty());
    }

    #[test]
    fn collector_default() {
        let collector = MetricsCollector::default();
        assert!(collector.is_empty());
    }

    #[test]
    fn collector_debug() {
        let collector = MetricsCollector::new();
        let debug = format!("{collector:?}");
        assert!(debug.contains("MetricsCollector"));
    }

    // ── Convenience span starters ─────────────────────────────────────

    #[test]
    fn tool_span_creates_named_span() {
        let span = tool_span("bash");
        let timing = span.finish(true);
        assert_eq!(timing.name, "tool.bash");
        assert_eq!(timing.metadata["tool"], serde_json::json!("bash"));
    }

    #[test]
    fn api_span_creates_named_span() {
        let span = api_span("anthropic", "claude-sonnet-4-20250514");
        let timing = span.finish(true);
        assert_eq!(timing.name, "api.anthropic");
        assert_eq!(timing.metadata["provider"], serde_json::json!("anthropic"));
        assert_eq!(
            timing.metadata["model"],
            serde_json::json!("claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn agent_loop_span_creates_named_span() {
        let span = agent_loop_span(5);
        let timing = span.finish(true);
        assert_eq!(timing.name, "agent.loop");
        assert_eq!(timing.metadata["iteration"], serde_json::json!(5));
    }

    // ── SpanStats serde ───────────────────────────────────────────────

    #[test]
    fn span_stats_serde_roundtrip() {
        let stats = SpanStats {
            count: 10,
            success_count: 8,
            avg_ms: 150.5,
            min_ms: 10.0,
            max_ms: 500.0,
            total_ms: 1505.0,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let parsed: SpanStats = serde_json::from_str(&json).unwrap();
        assert_eq!(stats, parsed);
    }
}
