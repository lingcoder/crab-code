//! Multi-model A/B comparison.
//!
//! `AbCompare` sends the same prompt to two models and produces a
//! `CompareResult` with latency, length, and token usage comparisons.
//! `ComparisonReport` formats the result as a human-readable table.

use std::fmt;
use std::time::Duration;

// ---------------------------------------------------------------------------
// CompareResult
// ---------------------------------------------------------------------------

/// Raw result of an A/B comparison between two models.
#[derive(Debug, Clone)]
pub struct CompareResult {
    /// Model A identifier.
    pub model_a: String,
    /// Model B identifier.
    pub model_b: String,
    /// Model A response text.
    pub response_a: String,
    /// Model B response text.
    pub response_b: String,
    /// Model A latency.
    pub latency_a: Duration,
    /// Model B latency.
    pub latency_b: Duration,
    /// Model A input token count.
    pub input_tokens_a: u64,
    /// Model A output token count.
    pub output_tokens_a: u64,
    /// Model B input token count.
    pub input_tokens_b: u64,
    /// Model B output token count.
    pub output_tokens_b: u64,
}

impl CompareResult {
    /// Which model responded faster.
    #[must_use]
    pub fn faster_model(&self) -> &str {
        if self.latency_a <= self.latency_b {
            &self.model_a
        } else {
            &self.model_b
        }
    }

    /// Which model produced a longer response (by characters).
    #[must_use]
    pub fn longer_response_model(&self) -> &str {
        if self.response_a.len() >= self.response_b.len() {
            &self.model_a
        } else {
            &self.model_b
        }
    }

    /// Which model used fewer total tokens.
    #[must_use]
    pub fn fewer_tokens_model(&self) -> &str {
        let total_a = self.input_tokens_a + self.output_tokens_a;
        let total_b = self.input_tokens_b + self.output_tokens_b;
        if total_a <= total_b {
            &self.model_a
        } else {
            &self.model_b
        }
    }

    /// Latency difference (absolute).
    #[must_use]
    pub fn latency_diff(&self) -> Duration {
        if self.latency_a >= self.latency_b {
            self.latency_a.checked_sub(self.latency_b).unwrap()
        } else {
            self.latency_b.checked_sub(self.latency_a).unwrap()
        }
    }

    /// Generate a formatted comparison report.
    #[must_use]
    pub fn report(&self) -> ComparisonReport<'_> {
        ComparisonReport { result: self }
    }
}

// ---------------------------------------------------------------------------
// ComparisonReport
// ---------------------------------------------------------------------------

/// Formatted comparison report for display.
pub struct ComparisonReport<'a> {
    result: &'a CompareResult,
}

impl fmt::Display for ComparisonReport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = self.result;
        writeln!(f, "=== A/B Comparison Report ===")?;
        writeln!(f)?;
        writeln!(f, "{:<20} {:>15} {:>15}", "Metric", r.model_a, r.model_b)?;
        writeln!(f, "{:-<52}", "")?;
        writeln!(
            f,
            "{:<20} {:>12}ms {:>12}ms",
            "Latency",
            r.latency_a.as_millis(),
            r.latency_b.as_millis()
        )?;
        writeln!(
            f,
            "{:<20} {:>15} {:>15}",
            "Response length",
            r.response_a.len(),
            r.response_b.len()
        )?;
        writeln!(
            f,
            "{:<20} {:>15} {:>15}",
            "Input tokens", r.input_tokens_a, r.input_tokens_b
        )?;
        writeln!(
            f,
            "{:<20} {:>15} {:>15}",
            "Output tokens", r.output_tokens_a, r.output_tokens_b
        )?;
        writeln!(
            f,
            "{:<20} {:>15} {:>15}",
            "Total tokens",
            r.input_tokens_a + r.output_tokens_a,
            r.input_tokens_b + r.output_tokens_b,
        )?;
        writeln!(f)?;
        writeln!(f, "Faster: {}", r.faster_model())?;
        writeln!(f, "Longer response: {}", r.longer_response_model())?;
        write!(f, "Fewer tokens: {}", r.fewer_tokens_model())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AbCompare builder
// ---------------------------------------------------------------------------

/// Builder for constructing and running A/B comparisons.
///
/// In a real system this would take `LlmBackend` references and send requests.
/// Here we provide the structure and allow callers to supply results manually
/// via `compare_with_results` for testing and offline analysis.
#[derive(Debug)]
pub struct AbCompare {
    model_a: String,
    model_b: String,
}

impl AbCompare {
    /// Create a new A/B comparison between two models.
    #[must_use]
    pub fn new(model_a: impl Into<String>, model_b: impl Into<String>) -> Self {
        Self {
            model_a: model_a.into(),
            model_b: model_b.into(),
        }
    }

    /// Model A identifier.
    #[must_use]
    pub fn model_a(&self) -> &str {
        &self.model_a
    }

    /// Model B identifier.
    #[must_use]
    pub fn model_b(&self) -> &str {
        &self.model_b
    }

    /// Build a `CompareResult` from pre-collected response data.
    ///
    /// This is the offline/test path — callers who already have both responses
    /// can construct the result directly.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn compare_with_results(
        &self,
        response_a: String,
        response_b: String,
        latency_a: Duration,
        latency_b: Duration,
        input_tokens_a: u64,
        output_tokens_a: u64,
        input_tokens_b: u64,
        output_tokens_b: u64,
    ) -> CompareResult {
        CompareResult {
            model_a: self.model_a.clone(),
            model_b: self.model_b.clone(),
            response_a,
            response_b,
            latency_a,
            latency_b,
            input_tokens_a,
            output_tokens_a,
            input_tokens_b,
            output_tokens_b,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result() -> CompareResult {
        CompareResult {
            model_a: "claude-sonnet".into(),
            model_b: "gpt-4o".into(),
            response_a: "Hello from Claude!".into(),
            response_b: "Hi there from GPT!".into(),
            latency_a: Duration::from_millis(500),
            latency_b: Duration::from_millis(800),
            input_tokens_a: 100,
            output_tokens_a: 50,
            input_tokens_b: 100,
            output_tokens_b: 60,
        }
    }

    #[test]
    fn faster_model() {
        let r = sample_result();
        assert_eq!(r.faster_model(), "claude-sonnet");
    }

    #[test]
    fn longer_response() {
        let r = sample_result();
        // "Hello from Claude!" (18) vs "Hi there from GPT!" (18) — equal, model_a wins
        assert_eq!(r.longer_response_model(), "claude-sonnet");
    }

    #[test]
    fn longer_response_model_b_wins() {
        let mut r = sample_result();
        r.response_b = "This is a much longer response from model B that has more text.".into();
        assert_eq!(r.longer_response_model(), "gpt-4o");
    }

    #[test]
    fn fewer_tokens() {
        let r = sample_result();
        // A: 150, B: 160
        assert_eq!(r.fewer_tokens_model(), "claude-sonnet");
    }

    #[test]
    fn fewer_tokens_model_b_wins() {
        let mut r = sample_result();
        r.output_tokens_a = 200;
        // A: 300, B: 160
        assert_eq!(r.fewer_tokens_model(), "gpt-4o");
    }

    #[test]
    fn latency_diff() {
        let r = sample_result();
        assert_eq!(r.latency_diff(), Duration::from_millis(300));
    }

    #[test]
    fn latency_diff_reversed() {
        let mut r = sample_result();
        r.latency_a = Duration::from_millis(1000);
        r.latency_b = Duration::from_millis(200);
        assert_eq!(r.latency_diff(), Duration::from_millis(800));
    }

    #[test]
    fn report_format() {
        let r = sample_result();
        let report = r.report().to_string();
        assert!(report.contains("A/B Comparison Report"));
        assert!(report.contains("claude-sonnet"));
        assert!(report.contains("gpt-4o"));
        assert!(report.contains("500ms"));
        assert!(report.contains("800ms"));
        assert!(report.contains("Faster: claude-sonnet"));
    }

    #[test]
    fn ab_compare_builder() {
        let ab = AbCompare::new("model-a", "model-b");
        assert_eq!(ab.model_a(), "model-a");
        assert_eq!(ab.model_b(), "model-b");

        let result = ab.compare_with_results(
            "resp a".into(),
            "resp b".into(),
            Duration::from_millis(100),
            Duration::from_millis(200),
            50,
            25,
            50,
            30,
        );
        assert_eq!(result.model_a, "model-a");
        assert_eq!(result.response_a, "resp a");
        assert_eq!(result.latency_a, Duration::from_millis(100));
    }

    #[test]
    fn equal_latency_prefers_model_a() {
        let mut r = sample_result();
        r.latency_a = Duration::from_millis(500);
        r.latency_b = Duration::from_millis(500);
        assert_eq!(r.faster_model(), "claude-sonnet");
    }

    #[test]
    fn equal_tokens_prefers_model_a() {
        let mut r = sample_result();
        r.output_tokens_a = 60; // Now both total 160
        assert_eq!(r.fewer_tokens_model(), "claude-sonnet");
    }
}
