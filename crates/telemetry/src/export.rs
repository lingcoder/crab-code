//! Local OTLP export: write spans and metrics to local files.
//!
//! All telemetry data stays on-disk. **No remote sending** — this is a
//! fundamental design constraint of Crab Code's telemetry system.
//!
//! Output format is newline-delimited JSON (NDJSON), one record per line,
//! suitable for offline analysis with `jq`, Grafana Loki, or similar tools.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

/// A completed span record ready for export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanRecord {
    /// Span name (e.g., `"tool_execute"`, `"llm_request"`).
    pub name: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Start timestamp (milliseconds since Unix epoch).
    pub start_time_ms: u64,
    /// Arbitrary key-value attributes.
    pub attributes: HashMap<String, String>,
    /// Optional parent span ID for hierarchical tracing.
    pub parent_id: Option<String>,
    /// Unique span ID.
    pub span_id: String,
}

/// A metric data point ready for export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRecord {
    /// Metric name (e.g., `"tokens_used"`, `"ttft_ms"`).
    pub name: String,
    /// Metric value.
    pub value: f64,
    /// Timestamp (milliseconds since Unix epoch).
    pub timestamp: u64,
    /// Optional labels for dimensional aggregation.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Local exporter
// ---------------------------------------------------------------------------

/// Writes telemetry data to local files. Never sends data over the network.
///
/// Creates separate files for spans and metrics within the output directory:
/// - `spans-{date}.ndjson`
/// - `metrics-{date}.ndjson`
pub struct LocalExporter {
    /// Directory where telemetry files are written.
    output_dir: PathBuf,
}

impl LocalExporter {
    /// Create a new exporter targeting the given directory.
    ///
    /// The directory is created if it does not exist.
    pub fn new(output_dir: PathBuf) -> Self {
        Self { output_dir }
    }

    /// Export a batch of span records to the spans file.
    ///
    /// Appends to the current day's file. Each record is serialized as a
    /// single JSON line.
    ///
    /// # Errors
    ///
    /// Returns an error if the output directory cannot be created or the
    /// file cannot be written.
    pub fn export_spans(&self, _spans: &[SpanRecord]) -> crab_common::Result<()> {
        todo!()
    }

    /// Export a batch of metric records to the metrics file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn export_metrics(&self, _metrics: &[MetricRecord]) -> crab_common::Result<()> {
        todo!()
    }

    /// Return the output directory path.
    pub fn output_dir(&self) -> &PathBuf {
        &self.output_dir
    }

    /// List all telemetry files in the output directory.
    pub fn list_files(&self) -> crab_common::Result<Vec<PathBuf>> {
        todo!()
    }

    /// Delete telemetry files older than the given number of days.
    pub fn cleanup_older_than(&self, _days: u32) -> crab_common::Result<u32> {
        todo!()
    }
}

impl std::fmt::Debug for LocalExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalExporter")
            .field("output_dir", &self.output_dir)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_record_serde_roundtrip() {
        let span = SpanRecord {
            name: "test_span".into(),
            duration_ms: 42,
            start_time_ms: 1_700_000_000_000,
            attributes: HashMap::from([("key".into(), "value".into())]),
            parent_id: None,
            span_id: "span-001".into(),
        };
        let json = serde_json::to_string(&span).unwrap();
        let parsed: SpanRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test_span");
        assert_eq!(parsed.duration_ms, 42);
    }

    #[test]
    fn metric_record_serde_roundtrip() {
        let metric = MetricRecord {
            name: "tokens_used".into(),
            value: 1234.0,
            timestamp: 1_700_000_000_000,
            labels: HashMap::new(),
        };
        let json = serde_json::to_string(&metric).unwrap();
        let parsed: MetricRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "tokens_used");
        assert_eq!(parsed.value, 1234.0);
    }

    #[test]
    fn exporter_new_stores_path() {
        let exporter = LocalExporter::new(PathBuf::from("/tmp/telemetry"));
        assert_eq!(exporter.output_dir(), &PathBuf::from("/tmp/telemetry"));
    }
}
