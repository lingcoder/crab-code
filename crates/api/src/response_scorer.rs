//! Rule-based response quality scoring.
//!
//! `ResponseScorer` evaluates LLM responses against configurable heuristics
//! and produces a `ResponseScore` with per-dimension ratings.

use std::fmt;

// ---------------------------------------------------------------------------
// ResponseScore
// ---------------------------------------------------------------------------

/// Quality score for a single response (each dimension 0.0–1.0).
#[derive(Debug, Clone)]
pub struct ResponseScore {
    /// How relevant the response is to the prompt (keyword overlap).
    pub relevance: f64,
    /// How complete the response appears (length adequacy, code blocks).
    pub completeness: f64,
    /// How concise the response is (penalizes excessive length).
    pub conciseness: f64,
    /// Weighted overall score.
    pub overall: f64,
}

impl fmt::Display for ResponseScore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "relevance={:.2} completeness={:.2} conciseness={:.2} overall={:.2}",
            self.relevance, self.completeness, self.conciseness, self.overall
        )
    }
}

// ---------------------------------------------------------------------------
// ScoringContext
// ---------------------------------------------------------------------------

/// Context provided alongside a response for scoring.
#[derive(Debug, Clone, Default)]
pub struct ScoringContext {
    /// The original user prompt/question.
    pub prompt: String,
    /// Whether the prompt is asking a code-related question.
    pub expects_code: bool,
    /// Ideal response length range (chars). `None` = use defaults.
    pub ideal_length: Option<(usize, usize)>,
}

impl ScoringContext {
    /// Create a scoring context with just the prompt.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        let prompt = prompt.into();
        let lower = prompt.to_lowercase();
        let expects_code = lower.contains("code")
            || lower.contains("implement")
            || lower.contains("function")
            || lower.contains("write a")
            || lower.contains("fix the")
            || lower.contains("bug");
        Self {
            prompt,
            expects_code,
            ideal_length: None,
        }
    }

    /// Override the `expects_code` flag.
    #[must_use]
    pub fn with_expects_code(mut self, expects: bool) -> Self {
        self.expects_code = expects;
        self
    }

    /// Set the ideal response length range (in characters).
    #[must_use]
    pub fn with_ideal_length(mut self, min: usize, max: usize) -> Self {
        self.ideal_length = Some((min, max));
        self
    }
}

// ---------------------------------------------------------------------------
// ResponseScorer
// ---------------------------------------------------------------------------

/// Rule-based scorer that evaluates response quality.
#[derive(Debug)]
#[allow(clippy::struct_field_names)]
pub struct ResponseScorer {
    /// Weight for relevance in overall score.
    relevance_weight: f64,
    /// Weight for completeness in overall score.
    completeness_weight: f64,
    /// Weight for conciseness in overall score.
    conciseness_weight: f64,
}

impl Default for ResponseScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponseScorer {
    /// Create a scorer with default weights (0.4 relevance, 0.35 completeness, 0.25 conciseness).
    #[must_use]
    pub fn new() -> Self {
        Self {
            relevance_weight: 0.40,
            completeness_weight: 0.35,
            conciseness_weight: 0.25,
        }
    }

    /// Set custom weights. They are normalized to sum to 1.0.
    #[must_use]
    pub fn with_weights(mut self, relevance: f64, completeness: f64, conciseness: f64) -> Self {
        let total = relevance + completeness + conciseness;
        if total > 0.0 {
            self.relevance_weight = relevance / total;
            self.completeness_weight = completeness / total;
            self.conciseness_weight = conciseness / total;
        }
        self
    }

    /// Score a response given its context.
    #[must_use]
    pub fn score(&self, response: &str, context: &ScoringContext) -> ResponseScore {
        let relevance = self.score_relevance(response, context);
        let completeness = self.score_completeness(response, context);
        let conciseness = self.score_conciseness(response, context);
        let overall = conciseness.mul_add(
            self.conciseness_weight,
            relevance.mul_add(
                self.relevance_weight,
                completeness * self.completeness_weight,
            ),
        );

        ResponseScore {
            relevance,
            completeness,
            conciseness,
            overall,
        }
    }

    /// Relevance: keyword overlap between prompt and response.
    #[allow(clippy::unused_self)]
    fn score_relevance(&self, response: &str, context: &ScoringContext) -> f64 {
        if context.prompt.is_empty() || response.is_empty() {
            return 0.0;
        }

        let prompt_lower = context.prompt.to_lowercase();
        let response_lower = response.to_lowercase();

        // Extract significant words from prompt (3+ chars, skip stop words).
        let keywords: Vec<&str> = prompt_lower
            .split_whitespace()
            .filter(|w| w.len() >= 3 && !is_stop_word(w))
            .collect();

        if keywords.is_empty() {
            return 0.5; // No meaningful keywords — neutral score.
        }

        let matched = keywords
            .iter()
            .filter(|kw| response_lower.contains(**kw))
            .count();

        #[allow(clippy::cast_precision_loss)]
        let ratio = matched as f64 / keywords.len() as f64;
        ratio.min(1.0)
    }

    /// Completeness: length adequacy + code block presence when expected.
    #[allow(clippy::unused_self)]
    fn score_completeness(&self, response: &str, context: &ScoringContext) -> f64 {
        let len = response.len();
        if len == 0 {
            return 0.0;
        }

        // Length score: ramp up to ideal range, then plateau.
        let (min_len, max_len) = context.ideal_length.unwrap_or((50, 2000));
        let length_score = if len < min_len {
            #[allow(clippy::cast_precision_loss)]
            {
                len as f64 / min_len as f64
            }
        } else if len <= max_len {
            1.0
        } else {
            // Slight penalty for very long responses, but never below 0.7
            #[allow(clippy::cast_precision_loss)]
            {
                (1.0 - (len - max_len) as f64 / (max_len * 2) as f64).max(0.7)
            }
        };

        // Code block bonus when expected.
        let code_score = if context.expects_code {
            if response.contains("```") {
                1.0
            } else if response.contains("    ") || response.contains('\t') {
                0.6
            } else {
                0.3
            }
        } else {
            1.0 // Not expected — full marks.
        };

        length_score * 0.6 + code_score * 0.4
    }

    /// Conciseness: penalize excessive length and repetition.
    #[allow(clippy::unused_self)]
    fn score_conciseness(&self, response: &str, context: &ScoringContext) -> f64 {
        let len = response.len();
        if len == 0 {
            return 0.0;
        }

        let (_, max_len) = context.ideal_length.unwrap_or((50, 2000));

        // Length penalty.
        let length_score = if len <= max_len {
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                (max_len as f64 / len as f64).max(0.2)
            }
        };

        // Repetition penalty: check for repeated lines.
        let lines: Vec<&str> = response.lines().filter(|l| !l.trim().is_empty()).collect();
        let unique_lines = {
            let mut seen = std::collections::HashSet::new();
            lines.iter().filter(|l| seen.insert(l.trim())).count()
        };

        #[allow(clippy::cast_precision_loss)]
        let repetition_score = if lines.is_empty() {
            1.0
        } else {
            unique_lines as f64 / lines.len() as f64
        };

        length_score * 0.6 + repetition_score * 0.4
    }
}

/// Minimal stop-word list for relevance scoring.
fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "and"
            | "for"
            | "are"
            | "but"
            | "not"
            | "you"
            | "all"
            | "can"
            | "had"
            | "her"
            | "was"
            | "one"
            | "our"
            | "out"
            | "has"
            | "have"
            | "been"
            | "from"
            | "this"
            | "that"
            | "with"
            | "they"
            | "will"
            | "each"
            | "make"
            | "how"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_response_scores_zero() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("Tell me about Rust");
        let score = scorer.score("", &ctx);
        assert_eq!(score.relevance, 0.0);
        assert_eq!(score.completeness, 0.0);
        assert_eq!(score.overall, 0.0);
    }

    #[test]
    fn empty_prompt_relevance_zero() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("");
        let score = scorer.score("Some response text here", &ctx);
        assert_eq!(score.relevance, 0.0);
    }

    #[test]
    fn high_relevance_response() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("Explain Rust ownership and borrowing");
        let response = "Rust ownership means each value has exactly one owner. \
                         Borrowing allows references without taking ownership.";
        let score = scorer.score(response, &ctx);
        assert!(score.relevance > 0.5, "relevance={}", score.relevance);
    }

    #[test]
    fn low_relevance_response() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("Explain Rust ownership and borrowing");
        let response = "The weather today is sunny and warm.";
        let score = scorer.score(response, &ctx);
        assert!(score.relevance < 0.3, "relevance={}", score.relevance);
    }

    #[test]
    fn code_block_boosts_completeness() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("Write a function to add two numbers");
        let with_code =
            "Here is the function:\n```rust\nfn add(a: i32, b: i32) -> i32 { a + b }\n```";
        let without_code = "You can add two numbers by using the + operator.";

        let score_with = scorer.score(with_code, &ctx);
        let score_without = scorer.score(without_code, &ctx);
        assert!(
            score_with.completeness > score_without.completeness,
            "with={} without={}",
            score_with.completeness,
            score_without.completeness
        );
    }

    #[test]
    fn conciseness_penalizes_excessive_length() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("Hello").with_ideal_length(10, 100);
        let short = "Hi there!";
        let long = "word ".repeat(500);

        let score_short = scorer.score(short, &ctx);
        let score_long = scorer.score(&long, &ctx);
        assert!(
            score_short.conciseness > score_long.conciseness,
            "short={} long={}",
            score_short.conciseness,
            score_long.conciseness
        );
    }

    #[test]
    fn repetition_penalty() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("test").with_ideal_length(10, 5000);
        let unique = "Line one\nLine two\nLine three\nLine four\nLine five";
        let repeated = "Same line\nSame line\nSame line\nSame line\nSame line";

        let score_unique = scorer.score(unique, &ctx);
        let score_repeated = scorer.score(repeated, &ctx);
        assert!(
            score_unique.conciseness > score_repeated.conciseness,
            "unique={} repeated={}",
            score_unique.conciseness,
            score_repeated.conciseness
        );
    }

    #[test]
    fn overall_is_weighted_sum() {
        let scorer = ResponseScorer::with_weights(ResponseScorer::new(), 1.0, 0.0, 0.0);
        let ctx = ScoringContext::new("Rust programming language features");
        let response = "Rust is a systems programming language focused on safety.";
        let score = scorer.score(response, &ctx);
        // With only relevance weight, overall should equal relevance.
        assert!(
            (score.overall - score.relevance).abs() < 0.01,
            "overall={} relevance={}",
            score.overall,
            score.relevance
        );
    }

    #[test]
    fn custom_weights_normalized() {
        let scorer = ResponseScorer::new().with_weights(2.0, 2.0, 1.0);
        let ctx = ScoringContext::new("test query");
        let score = scorer.score("Some response with test content", &ctx);
        // overall should be <= 1.0
        assert!(score.overall <= 1.0, "overall={}", score.overall);
    }

    #[test]
    fn score_display() {
        let score = ResponseScore {
            relevance: 0.8,
            completeness: 0.7,
            conciseness: 0.9,
            overall: 0.79,
        };
        let s = score.to_string();
        assert!(s.contains("0.80"));
        assert!(s.contains("0.70"));
        assert!(s.contains("0.90"));
    }

    #[test]
    fn scoring_context_auto_detects_code() {
        let ctx = ScoringContext::new("Implement a binary search function");
        assert!(ctx.expects_code);

        let ctx2 = ScoringContext::new("What is the capital of France");
        assert!(!ctx2.expects_code);
    }

    #[test]
    fn scoring_context_override_expects_code() {
        let ctx = ScoringContext::new("Tell me about Rust").with_expects_code(true);
        assert!(ctx.expects_code);
    }

    #[test]
    fn ideal_length_range() {
        let scorer = ResponseScorer::new();
        let ctx = ScoringContext::new("test").with_ideal_length(100, 500);

        // Too short
        let short_score = scorer.score("hi", &ctx);
        // Just right
        let mid = "x".repeat(300);
        let mid_score = scorer.score(&mid, &ctx);

        assert!(
            mid_score.completeness > short_score.completeness,
            "mid={} short={}",
            mid_score.completeness,
            short_score.completeness
        );
    }

    #[test]
    fn no_keywords_gives_neutral_relevance() {
        let scorer = ResponseScorer::new();
        // All short/stop words
        let ctx = ScoringContext::new("I am");
        let score = scorer.score("Hello world", &ctx);
        assert!(
            (score.relevance - 0.5).abs() < 0.01,
            "relevance={}",
            score.relevance
        );
    }
}
