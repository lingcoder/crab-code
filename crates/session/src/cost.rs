use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crab_core::model::TokenUsage;

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    /// Cost per million input tokens (USD).
    pub input_per_mtok: f64,
    /// Cost per million output tokens (USD).
    pub output_per_mtok: f64,
    /// Cost per million cache-read tokens (USD). 0.0 if not applicable.
    pub cache_read_per_mtok: f64,
    /// Cost per million cache-creation tokens (USD). 0.0 if not applicable.
    pub cache_creation_per_mtok: f64,
}

impl ModelPricing {
    /// Calculate total cost for a given token usage.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn calculate_cost(&self, usage: &TokenUsage) -> f64 {
        let input = usage.input_tokens as f64 * self.input_per_mtok / 1_000_000.0;
        let output = usage.output_tokens as f64 * self.output_per_mtok / 1_000_000.0;
        let cache_read = usage.cache_read_tokens as f64 * self.cache_read_per_mtok / 1_000_000.0;
        let cache_write =
            usage.cache_creation_tokens as f64 * self.cache_creation_per_mtok / 1_000_000.0;
        input + output + cache_read + cache_write
    }
}

/// Built-in pricing table entries: (model prefix, pricing).
/// The model prefix is matched against the start of the model ID,
/// enabling fuzzy matching (e.g. "claude-sonnet-4-20250514" matches "claude-sonnet-4").
///
/// Order matters: longer/more specific prefixes should come first.
const PRICING_TABLE: &[(&str, ModelPricing)] = &[
    (
        "claude-opus-4",
        ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
            cache_read_per_mtok: 1.50,
            cache_creation_per_mtok: 18.75,
        },
    ),
    (
        "claude-sonnet-4",
        ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.30,
            cache_creation_per_mtok: 3.75,
        },
    ),
    (
        "claude-haiku-4",
        ModelPricing {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
            cache_read_per_mtok: 0.08,
            cache_creation_per_mtok: 1.0,
        },
    ),
    (
        "claude-3-5-sonnet",
        ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.30,
            cache_creation_per_mtok: 3.75,
        },
    ),
    (
        "claude-3-5-haiku",
        ModelPricing {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
            cache_read_per_mtok: 0.08,
            cache_creation_per_mtok: 1.0,
        },
    ),
    (
        "gpt-4o-mini",
        ModelPricing {
            input_per_mtok: 0.15,
            output_per_mtok: 0.60,
            cache_read_per_mtok: 0.075,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "gpt-4o",
        ModelPricing {
            input_per_mtok: 2.50,
            output_per_mtok: 10.0,
            cache_read_per_mtok: 1.25,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "gpt-4.1-mini",
        ModelPricing {
            input_per_mtok: 0.40,
            output_per_mtok: 1.60,
            cache_read_per_mtok: 0.10,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "gpt-4.1-nano",
        ModelPricing {
            input_per_mtok: 0.10,
            output_per_mtok: 0.40,
            cache_read_per_mtok: 0.025,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "gpt-4.1",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 8.0,
            cache_read_per_mtok: 0.50,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "o3-mini",
        ModelPricing {
            input_per_mtok: 1.10,
            output_per_mtok: 4.40,
            cache_read_per_mtok: 0.55,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "o3",
        ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 8.0,
            cache_read_per_mtok: 0.50,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "o4-mini",
        ModelPricing {
            input_per_mtok: 1.10,
            output_per_mtok: 4.40,
            cache_read_per_mtok: 0.55,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "deepseek-chat",
        ModelPricing {
            input_per_mtok: 0.27,
            output_per_mtok: 1.10,
            cache_read_per_mtok: 0.07,
            cache_creation_per_mtok: 0.0,
        },
    ),
    (
        "deepseek-reasoner",
        ModelPricing {
            input_per_mtok: 0.55,
            output_per_mtok: 2.19,
            cache_read_per_mtok: 0.14,
            cache_creation_per_mtok: 0.0,
        },
    ),
];

/// Default pricing used when the model is not found in the pricing table.
/// Uses Claude Sonnet 4 pricing as a reasonable default.
const DEFAULT_PRICING: ModelPricing = ModelPricing {
    input_per_mtok: 3.0,
    output_per_mtok: 15.0,
    cache_read_per_mtok: 0.30,
    cache_creation_per_mtok: 3.75,
};

/// Look up pricing for a model ID. Uses prefix matching so that
/// "claude-sonnet-4-20250514" matches the "claude-sonnet-4" entry.
///
/// Returns the default pricing if no match is found.
#[must_use]
pub fn lookup_pricing(model_id: &str) -> ModelPricing {
    for (prefix, pricing) in PRICING_TABLE {
        if model_id.starts_with(prefix) {
            return *pricing;
        }
    }
    DEFAULT_PRICING
}

/// Summary of accumulated costs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_cost_usd: f64,
    pub api_calls: u64,
}

/// Session-level cost accumulator with automatic pricing lookup.
#[derive(Default)]
pub struct CostAccumulator {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cost_usd: f64,
    /// Number of API calls recorded.
    pub api_calls: u64,
}

impl CostAccumulator {
    /// Record a single API response's usage, automatically looking up
    /// pricing from the built-in table.
    pub fn add_usage(&mut self, model: &str, usage: &TokenUsage) {
        let pricing = lookup_pricing(model);
        let cost = pricing.calculate_cost(usage);
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_cache_read_tokens += usage.cache_read_tokens;
        self.total_cache_creation_tokens += usage.cache_creation_tokens;
        self.total_cost_usd += cost;
        self.api_calls += 1;
    }

    /// Record usage with a pre-calculated cost (for callers that already know the cost).
    pub fn record(&mut self, usage: &TokenUsage, cost: f64) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_cache_read_tokens += usage.cache_read_tokens;
        self.total_cache_creation_tokens += usage.cache_creation_tokens;
        self.total_cost_usd += cost;
        self.api_calls += 1;
    }

    pub fn merge(&mut self, other: &Self) {
        self.total_input_tokens += other.total_input_tokens;
        self.total_output_tokens += other.total_output_tokens;
        self.total_cache_read_tokens += other.total_cache_read_tokens;
        self.total_cache_creation_tokens += other.total_cache_creation_tokens;
        self.total_cost_usd += other.total_cost_usd;
        self.api_calls += other.api_calls;
    }

    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }

    pub fn total_cost_usd(&self) -> f64 {
        self.total_cost_usd
    }

    /// Total cache-related tokens (read + creation).
    pub fn total_cache_tokens(&self) -> u64 {
        self.total_cache_read_tokens + self.total_cache_creation_tokens
    }

    /// Return a snapshot summary of accumulated costs.
    #[must_use]
    pub fn summary(&self) -> CostSummary {
        CostSummary {
            input_tokens: self.total_input_tokens,
            output_tokens: self.total_output_tokens,
            cache_read_tokens: self.total_cache_read_tokens,
            cache_creation_tokens: self.total_cache_creation_tokens,
            total_cost_usd: self.total_cost_usd,
            api_calls: self.api_calls,
        }
    }

    /// Format as a compact summary line for TUI display.
    pub fn summary_line(&self) -> String {
        format!(
            "tokens: {}in/{}out | cache: {}r/{}w | cost: ${:.4} | calls: {}",
            self.total_input_tokens,
            self.total_output_tokens,
            self.total_cache_read_tokens,
            self.total_cache_creation_tokens,
            self.total_cost_usd,
            self.api_calls,
        )
    }
}

impl fmt::Display for CostAccumulator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.summary_line())
    }
}

// ── Persistence ───────────────────────────────────────────────────────

/// Default on-disk path for the persisted cost summary:
/// `~/.crab/sessions/costs.json`.
#[must_use]
pub fn default_cost_path() -> PathBuf {
    crab_common::utils::path::home_dir()
        .join(".crab")
        .join("sessions")
        .join("costs.json")
}

/// Persist a `CostSummary` to disk as pretty-printed JSON.
///
/// Creates parent directories as needed.
pub fn save_cost_summary(path: &Path, summary: &CostSummary) -> crab_core::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crab_core::Error::Other(format!("failed to create {}: {e}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(summary)
        .map_err(|e| crab_core::Error::Other(format!("failed to serialize cost summary: {e}")))?;
    std::fs::write(path, json)
        .map_err(|e| crab_core::Error::Other(format!("failed to write {}: {e}", path.display())))
}

/// Load a `CostSummary` from disk. Returns `Ok(None)` if the file
/// does not exist.
pub fn load_cost_summary(path: &Path) -> crab_core::Result<Option<CostSummary>> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed: CostSummary = serde_json::from_str(&content).map_err(|e| {
                crab_core::Error::Other(format!("failed to parse {}: {e}", path.display()))
            })?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(crab_core::Error::Other(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zero() {
        let acc = CostAccumulator::default();
        assert_eq!(acc.total_tokens(), 0);
        assert!(acc.total_cost_usd().abs() < f64::EPSILON);
        assert_eq!(acc.api_calls, 0);
        assert_eq!(acc.total_cache_tokens(), 0);
    }

    #[test]
    fn record_accumulates() {
        let mut acc = CostAccumulator::default();
        acc.record(
            &TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 20,
                cache_creation_tokens: 10,
            },
            0.01,
        );
        acc.record(
            &TokenUsage {
                input_tokens: 200,
                output_tokens: 100,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            0.02,
        );
        assert_eq!(acc.total_tokens(), 450);
        assert_eq!(acc.api_calls, 2);
        assert_eq!(acc.total_cache_tokens(), 30);
        assert!((acc.total_cost_usd() - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn summary_line_format() {
        let mut acc = CostAccumulator::default();
        acc.record(
            &TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 200,
                cache_creation_tokens: 100,
            },
            0.015,
        );
        let line = acc.summary_line();
        assert!(line.contains("1000in"));
        assert!(line.contains("500out"));
        assert!(line.contains("200r"));
        assert!(line.contains("100w"));
        assert!(line.contains("$0.0150"));
        assert!(line.contains("calls: 1"));
    }

    #[test]
    fn display_trait() {
        let acc = CostAccumulator::default();
        let s = format!("{acc}");
        assert!(s.contains("tokens:"));
    }

    // ── Pricing tests ──────────────────────────────────────────────────

    #[test]
    fn claude_sonnet_4_pricing_exact() {
        let pricing = lookup_pricing("claude-sonnet-4");
        assert!((pricing.input_per_mtok - 3.0).abs() < f64::EPSILON);
        assert!((pricing.output_per_mtok - 15.0).abs() < f64::EPSILON);
        assert!((pricing.cache_read_per_mtok - 0.30).abs() < f64::EPSILON);
    }

    #[test]
    fn claude_opus_4_pricing() {
        let pricing = lookup_pricing("claude-opus-4");
        assert!((pricing.input_per_mtok - 15.0).abs() < f64::EPSILON);
        assert!((pricing.output_per_mtok - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn claude_haiku_pricing() {
        let pricing = lookup_pricing("claude-haiku-4.5-20251001");
        assert!((pricing.input_per_mtok - 0.80).abs() < f64::EPSILON);
        assert!((pricing.output_per_mtok - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gpt_4o_pricing() {
        let pricing = lookup_pricing("gpt-4o");
        assert!((pricing.input_per_mtok - 2.50).abs() < f64::EPSILON);
        assert!((pricing.output_per_mtok - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gpt_4o_mini_pricing() {
        let pricing = lookup_pricing("gpt-4o-mini");
        assert!((pricing.input_per_mtok - 0.15).abs() < f64::EPSILON);
        assert!((pricing.output_per_mtok - 0.60).abs() < f64::EPSILON);
    }

    // ── Fuzzy matching tests ───────────────────────────────────────────

    #[test]
    fn fuzzy_match_claude_sonnet_with_date() {
        let pricing = lookup_pricing("claude-sonnet-4-20250514");
        assert!((pricing.input_per_mtok - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fuzzy_match_claude_opus_with_date() {
        let pricing = lookup_pricing("claude-opus-4-20250514");
        assert!((pricing.input_per_mtok - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fuzzy_match_gpt4o_mini_variant() {
        // "gpt-4o-mini-2024-07-18" should match "gpt-4o-mini"
        let pricing = lookup_pricing("gpt-4o-mini-2024-07-18");
        assert!((pricing.input_per_mtok - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_model_uses_default() {
        let pricing = lookup_pricing("some-unknown-model-v2");
        // Default is claude-sonnet-4 pricing
        assert!((pricing.input_per_mtok - 3.0).abs() < f64::EPSILON);
        assert!((pricing.output_per_mtok - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_model_does_not_panic() {
        let mut acc = CostAccumulator::default();
        acc.add_usage(
            "totally-unknown-model",
            &TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        assert!(acc.total_cost_usd() > 0.0);
        assert_eq!(acc.api_calls, 1);
    }

    // ── Cost calculation tests ─────────────────────────────────────────

    #[test]
    fn cost_calculation_claude_sonnet_4() {
        let pricing = lookup_pricing("claude-sonnet-4");
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        let cost = pricing.calculate_cost(&usage);
        // $3 input + $15 output = $18
        assert!((cost - 18.0).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_with_cache() {
        let pricing = lookup_pricing("claude-sonnet-4");
        let usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 1_000_000,
            cache_creation_tokens: 1_000_000,
        };
        let cost = pricing.calculate_cost(&usage);
        // $0.30 cache read + $3.75 cache creation = $4.05
        assert!((cost - 4.05).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_zero_tokens() {
        let pricing = lookup_pricing("claude-sonnet-4");
        let usage = TokenUsage::default();
        let cost = pricing.calculate_cost(&usage);
        assert!(cost.abs() < f64::EPSILON);
    }

    // ── add_usage integration tests ────────────────────────────────────

    #[test]
    fn add_usage_calculates_cost_automatically() {
        let mut acc = CostAccumulator::default();
        acc.add_usage(
            "claude-sonnet-4-20250514",
            &TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        // 1M input tokens at $3/MTok = $3.00
        assert!((acc.total_cost_usd() - 3.0).abs() < 0.001);
    }

    #[test]
    fn add_usage_accumulates_multiple_calls() {
        let mut acc = CostAccumulator::default();
        let usage = TokenUsage {
            input_tokens: 500,
            output_tokens: 200,
            cache_read_tokens: 100,
            cache_creation_tokens: 0,
        };
        acc.add_usage("claude-sonnet-4", &usage);
        acc.add_usage("claude-sonnet-4", &usage);

        assert_eq!(acc.total_input_tokens, 1000);
        assert_eq!(acc.total_output_tokens, 400);
        assert_eq!(acc.total_cache_read_tokens, 200);
        assert_eq!(acc.api_calls, 2);
        assert!(acc.total_cost_usd() > 0.0);
    }

    #[test]
    fn add_usage_different_models() {
        let mut acc = CostAccumulator::default();
        acc.add_usage(
            "claude-sonnet-4",
            &TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        acc.add_usage(
            "gpt-4o",
            &TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        assert_eq!(acc.total_input_tokens, 2000);
        assert_eq!(acc.total_output_tokens, 1000);
        assert_eq!(acc.api_calls, 2);
    }

    // ── Summary tests ──────────────────────────────────────────────────

    #[test]
    fn summary_returns_correct_snapshot() {
        let mut acc = CostAccumulator::default();
        acc.add_usage(
            "claude-sonnet-4",
            &TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 200,
                cache_creation_tokens: 100,
            },
        );
        let summary = acc.summary();
        assert_eq!(summary.input_tokens, 1000);
        assert_eq!(summary.output_tokens, 500);
        assert_eq!(summary.cache_read_tokens, 200);
        assert_eq!(summary.cache_creation_tokens, 100);
        assert_eq!(summary.api_calls, 1);
        assert!(summary.total_cost_usd > 0.0);
    }

    // ── Prefix ordering (gpt-4o-mini before gpt-4o) ───────────────────

    #[test]
    fn gpt_4o_mini_does_not_match_gpt_4o() {
        let mini = lookup_pricing("gpt-4o-mini");
        let full = lookup_pricing("gpt-4o");
        // mini should be cheaper
        assert!(mini.input_per_mtok < full.input_per_mtok);
    }

    // ── Persistence ────────────────────────────────────────────────────

    fn sample_summary() -> CostSummary {
        CostSummary {
            input_tokens: 1234,
            output_tokens: 567,
            cache_read_tokens: 89,
            cache_creation_tokens: 42,
            total_cost_usd: 0.0153,
            api_calls: 7,
        }
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("costs.json");
        let original = sample_summary();
        save_cost_summary(&path, &original).expect("save");
        let loaded = load_cost_summary(&path)
            .expect("load")
            .expect("file exists");
        assert_eq!(loaded.input_tokens, original.input_tokens);
        assert_eq!(loaded.output_tokens, original.output_tokens);
        assert_eq!(loaded.cache_read_tokens, original.cache_read_tokens);
        assert_eq!(loaded.cache_creation_tokens, original.cache_creation_tokens);
        assert_eq!(loaded.api_calls, original.api_calls);
        assert!((loaded.total_cost_usd - original.total_cost_usd).abs() < f64::EPSILON);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("missing.json");
        let loaded = load_cost_summary(&path).expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("nested").join("deeper").join("costs.json");
        save_cost_summary(&path, &sample_summary()).expect("save");
        assert!(path.exists());
    }

    #[test]
    fn default_cost_path_under_crab_sessions() {
        let path = default_cost_path();
        assert!(path.ends_with("costs.json"));
        let s = path.to_string_lossy();
        assert!(s.contains(".crab"));
        assert!(s.contains("sessions"));
    }

    #[test]
    fn cost_summary_serde_roundtrip() {
        let summary = sample_summary();
        let json = serde_json::to_string(&summary).expect("serialize");
        let parsed: CostSummary = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.input_tokens, summary.input_tokens);
        assert_eq!(parsed.api_calls, summary.api_calls);
    }
}
