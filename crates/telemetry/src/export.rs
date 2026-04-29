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
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

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

    /// Export a batch of span timings to the spans file.
    pub fn export_spans(&self, spans: &[crate::metrics::SpanTiming]) -> crab_core::Result<()> {
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
    pub fn export_metrics(&self, metrics: &[MetricRecord]) -> crab_core::Result<()> {
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
    pub fn list_files(&self) -> crab_core::Result<Vec<PathBuf>> {
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
    pub fn cleanup_older_than(&self, days: u32) -> crab_core::Result<u32> {
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
    chrono::Local::now().format("%Y-%m-%d").to_string()
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
    use crate::metrics::SpanTiming;

    #[test]
    fn span_timing_serde_roundtrip() {
        let span = SpanTiming {
            name: "test_span".into(),
            span_id: "0000000000000001".into(),
            start_time_ms: 1_700_000_000_000,
            duration_ms: 42.0,
            success: true,
            metadata: HashMap::from([("key".into(), serde_json::Value::String("value".into()))]),
        };
        let json = serde_json::to_string(&span).unwrap();
        let parsed: SpanTiming = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test_span");
        assert!((parsed.duration_ms - 42.0).abs() < f64::EPSILON);
        assert_eq!(parsed.span_id, "0000000000000001");
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

        let span = SpanTiming {
            name: "test".into(),
            span_id: "000000000000000a".into(),
            start_time_ms: 0,
            duration_ms: 10.0,
            success: true,
            metadata: HashMap::new(),
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
