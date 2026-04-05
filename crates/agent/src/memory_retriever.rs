//! Memory retrieval and ranking: searches loaded memories by keyword relevance
//! and applies time-decay to prioritize recent, contextually relevant memories.

use std::fmt::Write;

use crab_session::MemoryFile;

// ── Retrieval ────────────────────────────────────────���────────────────

/// A memory with its computed relevance score.
#[derive(Debug, Clone)]
pub struct RankedMemory {
    pub memory: MemoryFile,
    /// Combined relevance score (higher = more relevant).
    pub score: f64,
    /// Breakdown of scoring factors.
    pub factors: Vec<(String, f64)>,
}

/// Configuration for memory retrieval.
#[derive(Debug, Clone)]
pub struct RetrieverConfig {
    /// Maximum number of memories to return.
    pub max_results: usize,
    /// Minimum score threshold (0.0 to 1.0). Memories below this are excluded.
    pub min_score: f64,
    /// Weight for keyword match scoring.
    pub keyword_weight: f64,
    /// Weight for memory type match scoring.
    pub type_weight: f64,
    /// Weight for description match scoring.
    pub description_weight: f64,
}

impl Default for RetrieverConfig {
    fn default() -> Self {
        Self {
            max_results: 10,
            min_score: 0.1,
            keyword_weight: 1.0,
            type_weight: 0.5,
            description_weight: 0.3,
        }
    }
}

/// Retrieve and rank memories relevant to a query context.
///
/// Scores each memory against the query keywords, then returns the top N
/// ranked by relevance.
#[must_use]
pub fn retrieve_memories(
    memories: &[MemoryFile],
    query: &str,
    config: &RetrieverConfig,
) -> Vec<RankedMemory> {
    if memories.is_empty() || query.trim().is_empty() {
        return Vec::new();
    }

    let query_terms = extract_terms(query);
    if query_terms.is_empty() {
        return Vec::new();
    }

    let mut ranked: Vec<RankedMemory> = memories
        .iter()
        .map(|mem| score_memory(mem, &query_terms, config))
        .filter(|rm| rm.score >= config.min_score)
        .collect();

    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.truncate(config.max_results);
    ranked
}

/// Retrieve memories relevant to a detected context type.
///
/// Maps context type names to memory type preferences.
#[must_use]
pub fn retrieve_for_context(
    memories: &[MemoryFile],
    context_type: &str,
    query: &str,
    config: &RetrieverConfig,
) -> Vec<RankedMemory> {
    if memories.is_empty() {
        return Vec::new();
    }

    // First get keyword-based results
    let mut ranked = retrieve_memories(memories, query, config);

    // Boost memories whose type matches the context
    let preferred_types = context_preferred_types(context_type);
    for rm in &mut ranked {
        if preferred_types.contains(&rm.memory.memory_type.as_str()) {
            rm.score *= 1.3;
            rm.factors
                .push(("context_type_boost".into(), 0.3));
        }
    }

    // Re-sort after boosting
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.truncate(config.max_results);
    ranked
}

/// Map context type to preferred memory types.
fn context_preferred_types(context_type: &str) -> Vec<&'static str> {
    match context_type {
        "debugging" => vec!["feedback", "project"],
        "code_generation" => vec!["feedback", "user", "reference"],
        "code_review" => vec!["feedback", "user"],
        "refactoring" => vec!["feedback", "project"],
        "navigation" => vec!["reference", "project"],
        "testing" => vec!["feedback", "project"],
        "documentation" => vec!["feedback", "reference"],
        _ => vec!["user", "feedback", "project", "reference"],
    }
}

// ── Scoring ────────────────────────────────────���──────────────────────

/// Score a single memory against query terms.
fn score_memory(
    mem: &MemoryFile,
    query_terms: &[String],
    config: &RetrieverConfig,
) -> RankedMemory {
    let mut factors = Vec::new();

    // Keyword matching in body
    let body_score = keyword_score(&mem.body, query_terms);
    if body_score > 0.0 {
        factors.push(("body_match".into(), body_score * config.keyword_weight));
    }

    // Keyword matching in name
    let name_score = keyword_score(&mem.name, query_terms);
    if name_score > 0.0 {
        factors.push(("name_match".into(), name_score * config.keyword_weight * 1.5));
    }

    // Keyword matching in description
    let desc_score = keyword_score(&mem.description, query_terms);
    if desc_score > 0.0 {
        factors.push(("desc_match".into(), desc_score * config.description_weight));
    }

    // Memory type relevance bonus
    let type_bonus = type_base_score(&mem.memory_type);
    if type_bonus > 0.0 {
        factors.push(("type_base".into(), type_bonus * config.type_weight));
    }

    let total: f64 = factors.iter().map(|(_, s)| s).sum();

    RankedMemory {
        memory: mem.clone(),
        score: total,
        factors,
    }
}

/// Compute keyword match score: fraction of query terms found in text.
fn keyword_score(text: &str, terms: &[String]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    let text_lower = text.to_lowercase();
    #[allow(clippy::cast_precision_loss)]
    let matched = terms.iter().filter(|t| text_lower.contains(t.as_str())).count() as f64;
    #[allow(clippy::cast_precision_loss)]
    let total = terms.len() as f64;
    matched / total
}

/// Base score by memory type (feedback and project are generally more actionable).
fn type_base_score(memory_type: &str) -> f64 {
    match memory_type {
        "feedback" => 0.3,
        "project" => 0.2,
        "user" => 0.15,
        "reference" => 0.1,
        _ => 0.05,
    }
}

/// Extract lowercase search terms from a query, filtering stop words.
fn extract_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_lowercase()
        })
        .filter(|w| w.len() >= 3 && !is_stop_word(w))
        .collect()
}

fn is_stop_word(word: &str) -> bool {
    const STOP_WORDS: &[&str] = &[
        "the", "and", "for", "are", "but", "not", "you", "all", "can", "had", "her", "was",
        "one", "our", "out", "has", "its", "how", "did", "any", "she", "him", "his", "let",
        "may", "who", "use", "been", "from", "have", "each", "make", "like", "more", "into",
        "over", "such", "than", "them", "then", "they", "this", "that", "what", "when",
        "with", "will", "your", "which", "their", "there", "these", "those", "about",
        "would", "could", "should",
    ];
    STOP_WORDS.contains(&word)
}

// ── Memory ranker ─────────────────────────────────────────────────────

/// Re-ranks a set of already-retrieved memories using additional signals.
pub struct MemoryRanker {
    /// Recency boost: how much to favor recently accessed memories.
    pub recency_factor: f64,
    /// Diversity penalty: reduce score for memories of the same type.
    pub diversity_factor: f64,
}

impl Default for MemoryRanker {
    fn default() -> Self {
        Self {
            recency_factor: 0.2,
            diversity_factor: 0.1,
        }
    }
}

impl MemoryRanker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-rank memories, applying diversity penalty and recency boost.
    ///
    /// `access_order` maps memory filenames to their last access order
    /// (higher = more recent).
    #[must_use]
    pub fn rerank(
        &self,
        mut memories: Vec<RankedMemory>,
        access_order: &std::collections::HashMap<String, usize>,
    ) -> Vec<RankedMemory> {
        if memories.is_empty() {
            return memories;
        }

        // Find max access order for normalization
        #[allow(clippy::cast_precision_loss)]
        let max_order = access_order.values().copied().max().unwrap_or(1) as f64;

        // Track how many of each type we've seen for diversity penalty
        let mut type_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        // Sort by current score first
        memories.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for rm in &mut memories {
            // Recency boost
            if let Some(&order) = access_order.get(&rm.memory.filename) {
                #[allow(clippy::cast_precision_loss)]
                let recency = (order as f64) / max_order;
                rm.score += recency * self.recency_factor;
                rm.factors
                    .push(("recency".into(), recency * self.recency_factor));
            }

            // Diversity penalty: reduce score for repeated types
            let count = type_counts
                .entry(rm.memory.memory_type.clone())
                .or_insert(0);
            if *count > 0 {
                #[allow(clippy::cast_precision_loss)]
                let penalty = (*count as f64) * self.diversity_factor;
                rm.score -= penalty;
                rm.factors.push(("diversity_penalty".into(), -penalty));
            }
            *count += 1;
        }

        // Final sort
        memories.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        memories
    }
}

// ── Formatting ────────────────────────────────────────────────────────

/// Format ranked memories as a system prompt section.
#[must_use]
pub fn format_retrieved_memories(ranked: &[RankedMemory]) -> String {
    if ranked.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let _ = writeln!(out, "# Retrieved Memories\n");
    let _ = writeln!(
        out,
        "The following memories are relevant to the current context.\n"
    );

    for rm in ranked {
        let _ = writeln!(
            out,
            "## {} (type: {}, relevance: {:.0}%)\n",
            rm.memory.name,
            rm.memory.memory_type,
            rm.score * 100.0,
        );
        if !rm.memory.description.is_empty() {
            let _ = writeln!(out, "> {}\n", rm.memory.description);
        }
        let _ = writeln!(out, "{}\n", rm.memory.body);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_memory(name: &str, mem_type: &str, desc: &str, body: &str) -> MemoryFile {
        MemoryFile {
            name: name.into(),
            description: desc.into(),
            memory_type: mem_type.into(),
            body: body.into(),
            filename: format!("{}.md", name.to_lowercase().replace(' ', "_")),
        }
    }

    fn sample_memories() -> Vec<MemoryFile> {
        vec![
            make_memory(
                "User role",
                "user",
                "Senior Rust developer",
                "The user is a senior Rust developer focused on systems programming.",
            ),
            make_memory(
                "No mocks in tests",
                "feedback",
                "Use real DB in integration tests",
                "Always use real database connections. Why: mocked tests passed but prod migration failed.",
            ),
            make_memory(
                "Auth rewrite",
                "project",
                "Legal compliance requirement",
                "Auth middleware rewrite driven by legal compliance for session token storage.",
            ),
            make_memory(
                "Linear tracker",
                "reference",
                "Pipeline bugs in Linear INGEST",
                "Pipeline bugs are tracked in Linear project INGEST.",
            ),
        ]
    }

    // ── extract_terms ──────────────────────────────────────────────

    #[test]
    fn extract_terms_basic() {
        let terms = extract_terms("fix the authentication bug");
        assert!(terms.contains(&"fix".to_string()));
        assert!(terms.contains(&"authentication".to_string()));
        assert!(terms.contains(&"bug".to_string()));
        // "the" is a stop word
        assert!(!terms.contains(&"the".to_string()));
    }

    #[test]
    fn extract_terms_short_words_filtered() {
        let terms = extract_terms("a is it ok");
        // All too short or stop words
        assert!(terms.is_empty());
    }

    #[test]
    fn extract_terms_empty() {
        assert!(extract_terms("").is_empty());
    }

    #[test]
    fn extract_terms_preserves_identifiers() {
        let terms = extract_terms("query_loop function");
        assert!(terms.contains(&"query_loop".to_string()));
        assert!(terms.contains(&"function".to_string()));
    }

    // ── keyword_score ──────────────────────────────────────────────

    #[test]
    fn keyword_score_all_match() {
        let score = keyword_score("rust developer systems", &["rust".into(), "developer".into()]);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn keyword_score_partial_match() {
        let score = keyword_score("rust developer", &["rust".into(), "python".into()]);
        assert!((score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn keyword_score_no_match() {
        let score = keyword_score("hello world", &["rust".into(), "python".into()]);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn keyword_score_empty_terms() {
        assert!((keyword_score("anything", &[]) - 0.0).abs() < f64::EPSILON);
    }

    // ── type_base_score ────────────────────────────────────────────

    #[test]
    fn type_scores() {
        assert!(type_base_score("feedback") > type_base_score("user"));
        assert!(type_base_score("project") > type_base_score("reference"));
        assert!(type_base_score("unknown") > 0.0);
    }

    // ── is_stop_word ───────────────────────────────────────────────

    #[test]
    fn stop_words() {
        assert!(is_stop_word("the"));
        assert!(is_stop_word("and"));
        assert!(!is_stop_word("rust"));
        assert!(!is_stop_word("authentication"));
    }

    // ── retrieve_memories ──────────────────────────────────────────

    #[test]
    fn retrieve_empty_memories() {
        let result = retrieve_memories(&[], "test query", &RetrieverConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn retrieve_empty_query() {
        let mems = sample_memories();
        let result = retrieve_memories(&mems, "", &RetrieverConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn retrieve_whitespace_query() {
        let mems = sample_memories();
        let result = retrieve_memories(&mems, "   ", &RetrieverConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn retrieve_relevant_rust_memory() {
        let mems = sample_memories();
        let result = retrieve_memories(&mems, "rust developer programming", &RetrieverConfig::default());
        assert!(!result.is_empty());
        assert_eq!(result[0].memory.name, "User role");
    }

    #[test]
    fn retrieve_relevant_testing_memory() {
        let mems = sample_memories();
        let result = retrieve_memories(&mems, "database testing mocked integration", &RetrieverConfig::default());
        assert!(!result.is_empty());
        // The "No mocks in tests" memory should rank highest
        assert!(result.iter().any(|r| r.memory.name == "No mocks in tests"));
    }

    #[test]
    fn retrieve_relevant_auth_memory() {
        let mems = sample_memories();
        let result = retrieve_memories(&mems, "authentication session token compliance", &RetrieverConfig::default());
        assert!(!result.is_empty());
        assert!(result.iter().any(|r| r.memory.name == "Auth rewrite"));
    }

    #[test]
    fn retrieve_respects_max_results() {
        let mems = sample_memories();
        let config = RetrieverConfig {
            max_results: 1,
            min_score: 0.0,
            ..Default::default()
        };
        let result = retrieve_memories(&mems, "rust developer testing database", &config);
        assert!(result.len() <= 1);
    }

    #[test]
    fn retrieve_respects_min_score() {
        let mems = sample_memories();
        let config = RetrieverConfig {
            min_score: 100.0, // impossibly high
            ..Default::default()
        };
        let result = retrieve_memories(&mems, "rust developer", &config);
        assert!(result.is_empty());
    }

    // ── retrieve_for_context ───────────────────────────────────────

    #[test]
    fn retrieve_for_debugging_context() {
        let mems = sample_memories();
        let result = retrieve_for_context(&mems, "debugging", "database test failure", &RetrieverConfig::default());
        // Feedback memories should get a boost in debugging context
        if !result.is_empty() {
            let has_feedback = result.iter().any(|r| r.memory.memory_type == "feedback");
            assert!(has_feedback || result.is_empty());
        }
    }

    #[test]
    fn retrieve_for_context_empty() {
        let result = retrieve_for_context(&[], "debugging", "test", &RetrieverConfig::default());
        assert!(result.is_empty());
    }

    // ── context_preferred_types ────────────────────────────────────

    #[test]
    fn preferred_types_debugging() {
        let types = context_preferred_types("debugging");
        assert!(types.contains(&"feedback"));
        assert!(types.contains(&"project"));
    }

    #[test]
    fn preferred_types_navigation() {
        let types = context_preferred_types("navigation");
        assert!(types.contains(&"reference"));
    }

    #[test]
    fn preferred_types_general() {
        let types = context_preferred_types("general");
        assert_eq!(types.len(), 4); // all types
    }

    // ── MemoryRanker ───────────────────────────────────────────────

    #[test]
    fn ranker_default() {
        let ranker = MemoryRanker::new();
        assert!(ranker.recency_factor > 0.0);
        assert!(ranker.diversity_factor > 0.0);
    }

    #[test]
    fn ranker_empty_input() {
        let ranker = MemoryRanker::new();
        let result = ranker.rerank(vec![], &HashMap::new());
        assert!(result.is_empty());
    }

    #[test]
    fn ranker_recency_boost() {
        let ranker = MemoryRanker::new();
        let memories = vec![
            RankedMemory {
                memory: make_memory("Old", "user", "", "old memory"),
                score: 0.5,
                factors: vec![],
            },
            RankedMemory {
                memory: make_memory("Recent", "feedback", "", "recent memory"),
                score: 0.5,
                factors: vec![],
            },
        ];

        let mut access_order = HashMap::new();
        access_order.insert("old.md".to_string(), 1);
        access_order.insert("recent.md".to_string(), 10);

        let result = ranker.rerank(memories, &access_order);
        assert_eq!(result.len(), 2);
        // Recent memory should rank higher due to recency boost
        assert_eq!(result[0].memory.name, "Recent");
    }

    #[test]
    fn ranker_diversity_penalty() {
        let ranker = MemoryRanker {
            recency_factor: 0.0,
            diversity_factor: 0.5,
        };
        let memories = vec![
            RankedMemory {
                memory: make_memory("Feedback1", "feedback", "", "first feedback"),
                score: 1.0,
                factors: vec![],
            },
            RankedMemory {
                memory: make_memory("Feedback2", "feedback", "", "second feedback"),
                score: 0.9,
                factors: vec![],
            },
            RankedMemory {
                memory: make_memory("Project1", "project", "", "project info"),
                score: 0.8,
                factors: vec![],
            },
        ];

        let result = ranker.rerank(memories, &HashMap::new());
        // Second feedback should get a diversity penalty
        let fb2 = result.iter().find(|r| r.memory.name == "Feedback2").unwrap();
        assert!(fb2.score < 0.9); // penalized
    }

    // ── format_retrieved_memories ───────────────────────────────────

    #[test]
    fn format_empty() {
        assert!(format_retrieved_memories(&[]).is_empty());
    }

    #[test]
    fn format_single_memory() {
        let ranked = vec![RankedMemory {
            memory: make_memory("Test", "user", "A description", "Body content"),
            score: 0.85,
            factors: vec![],
        }];
        let text = format_retrieved_memories(&ranked);
        assert!(text.contains("Retrieved Memories"));
        assert!(text.contains("Test"));
        assert!(text.contains("type: user"));
        assert!(text.contains("85%"));
        assert!(text.contains("A description"));
        assert!(text.contains("Body content"));
    }

    #[test]
    fn format_multiple_memories() {
        let ranked = vec![
            RankedMemory {
                memory: make_memory("A", "user", "da", "ba"),
                score: 0.9,
                factors: vec![],
            },
            RankedMemory {
                memory: make_memory("B", "feedback", "db", "bb"),
                score: 0.7,
                factors: vec![],
            },
        ];
        let text = format_retrieved_memories(&ranked);
        assert!(text.contains("## A"));
        assert!(text.contains("## B"));
    }

    // ── RetrieverConfig defaults ────────────────────────────────────

    #[test]
    fn config_defaults() {
        let config = RetrieverConfig::default();
        assert_eq!(config.max_results, 10);
        assert!(config.min_score > 0.0);
        assert!(config.keyword_weight > 0.0);
    }

    // ── score_memory ───────────────────────────────────────────────

    #[test]
    fn score_irrelevant_memory() {
        let mem = make_memory("Unrelated", "user", "About cooking", "How to cook pasta.");
        let terms = vec!["rust".into(), "programming".into()];
        let config = RetrieverConfig::default();
        let ranked = score_memory(&mem, &terms, &config);
        // Should have low score (only type_base contributes)
        assert!(ranked.score < 0.5);
    }

    #[test]
    fn score_highly_relevant_memory() {
        let mem = make_memory(
            "Rust tips",
            "feedback",
            "Rust coding feedback",
            "Always use clippy for Rust code.",
        );
        let terms = vec!["rust".into(), "clippy".into(), "code".into()];
        let config = RetrieverConfig::default();
        let ranked = score_memory(&mem, &terms, &config);
        assert!(ranked.score > 0.5);
    }

    #[test]
    fn score_name_match_boosted() {
        let mem = make_memory("Rust style", "feedback", "", "Some generic content.");
        let terms = vec!["rust".into()];
        let config = RetrieverConfig::default();
        let ranked = score_memory(&mem, &terms, &config);
        // Name match gets 1.5x boost
        assert!(ranked.factors.iter().any(|(name, _)| name == "name_match"));
    }
}
