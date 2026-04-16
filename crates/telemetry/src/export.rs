//! Local OTLP export: write spans and metrics to local files.
//!
//! All telemetry data stays on-disk. **No remote sending** — this is a
//! fundamental design constraint of Crab Code's telemetry system.
//!
//! Output format is newline-delimited JSON (NDJSON), one record per line,
//! suitable for offline analysis with `jq`, Grafana Loki, or similar tools.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
pub struct LocalExporter {
    output_dir: PathBuf,
}

impl LocalExporter {
    /// Create a new exporter targeting the given directory.
    pub fn new(output_dir: PathBuf) -> Self {
        Self { output_dir }
    }

    /// Export a batch of span records to the spans file.
    pub fn export_spans(&self, spans: &[SpanRecord]) -> crab_common::Result<()> {
        if spans.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.output_dir)?;
        let path = self
            .output_dir
            .join(format!("spans-{}.ndjson", today_str()));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        for span in spans {
            if let Ok(json) = serde_json::to_string(span) {
                writeln!(file, "{json}")?;
            }
        }
        Ok(())
    }

    /// Export a batch of metric records to the metrics file.
    pub fn export_metrics(&self, metrics: &[MetricRecord]) -> crab_common::Result<()> {
        if metrics.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.output_dir)?;
        let path = self
            .output_dir
            .join(format!("metrics-{}.ndjson", today_str()));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        for metric in metrics {
            if let Ok(json) = serde_json::to_string(metric) {
                writeln!(file, "{json}")?;
            }
        }
        Ok(())
    }

    /// Return the output directory path.
    pub fn output_dir(&self) -> &PathBuf {
        &self.output_dir
    }

    /// List all telemetry files in the output directory.
    pub fn list_files(&self) -> crab_common::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let Ok(entries) = fs::read_dir(&self.output_dir) else {
            return Ok(files);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ndjson") {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    /// Delete telemetry files older than the given number of days.
    pub fn cleanup_older_than(&self, days: u32) -> crab_common::Result<u32> {
        let cutoff = SystemTime::now() - Duration::from_secs(u64::from(days) * 86400);
        let mut removed = 0u32;
        let Ok(entries) = fs::read_dir(&self.output_dir) else {
            return Ok(0);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ndjson") {
                continue;
            }
            if let Ok(meta) = path.metadata()
                && let Ok(modified) = meta.modified()
                && modified < cutoff
                && fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Get today's date as YYYY-MM-DD for file naming.
fn today_str() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    // Reuse the Hinnant civil date algorithm
    let z = days.cast_signed() + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
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
        assert!((parsed.value - 1234.0_f64).abs() < f64::EPSILON);
    }

    #[test]
    fn exporter_new_stores_path() {
        let exporter = LocalExporter::new(PathBuf::from("/tmp/telemetry"));
        assert_eq!(exporter.output_dir(), &PathBuf::from("/tmp/telemetry"));
    }

    #[test]
    fn export_empty_is_noop() {
        let tmp = std::env::temp_dir().join("crab_export_test_empty");
        let exporter = LocalExporter::new(tmp.clone());
        exporter.export_spans(&[]).unwrap();
        exporter.export_metrics(&[]).unwrap();
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn export_and_list_spans() {
        let tmp = std::env::temp_dir().join("crab_export_test_spans");
        let _ = fs::remove_dir_all(&tmp);
        let exporter = LocalExporter::new(tmp.clone());

        let span = SpanRecord {
            name: "test".into(),
            duration_ms: 10,
            start_time_ms: 0,
            attributes: HashMap::new(),
            parent_id: None,
            span_id: "s1".into(),
        };
        exporter.export_spans(&[span]).unwrap();

        let files = exporter.list_files().unwrap();
        assert!(!files.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn today_str_format() {
        let s = today_str();
        assert_eq!(s.len(), 10); // YYYY-MM-DD
        assert_eq!(s.as_bytes()[4], b'-');
        assert_eq!(s.as_bytes()[7], b'-');
    }
}
