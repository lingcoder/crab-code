//! Score and select memory files for injection into the system prompt.
//!
//! Scores each [`MemoryFile`] against the current conversation context
//! (keyword overlap, memory-type priority, name match), then picks the
//! highest-scoring entries that fit within configurable count and byte
//! budgets. Keeps the system prompt focused and avoids wasting context
//! window on irrelevant memories.

use crate::store::MemoryFile;
use crate::types::MemoryType;

// ─── MemorySelector ─────────────────────────────────────────────

/// Configuration for selecting relevant memories.
///
/// Bounds how many memories are injected and the total byte budget
/// across all selected entries (description + body).
#[derive(Debug, Clone, Copy)]
pub struct MemorySelector {
    /// Maximum number of memory entries to inject.
    pub max_memories: usize,
    /// Maximum total bytes (description + body) across all selected entries.
    pub max_total_bytes: usize,
}

impl Default for MemorySelector {
    fn default() -> Self {
        Self {
            max_memories: 20,
            max_total_bytes: 8_000,
        }
    }
}

// ─── ScoredMemory ───────────────────────────────────────────────

/// A [`MemoryFile`] paired with its computed relevance score in `[0.0, 1.0]`.
#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub file: MemoryFile,
    pub score: f64,
}

// ─── type_priority ──────────────────────────────────────────────

/// Base priority for each memory type. Feedback and user memories are
/// generally more actionable than references.
#[must_use]
pub fn type_priority(memory_type: MemoryType) -> f64 {
    match memory_type {
        MemoryType::Feedback => 0.8,
        MemoryType::User => 0.7,
        MemoryType::Project => 0.6,
        MemoryType::Reference => 0.4,
    }
}

// ─── Scoring + selection ────────────────────────────────────────

impl MemorySelector {
    /// Score every `memory` against `context`, sort by descending score, and
    /// return the top entries fitting within `max_memories` and
    /// `max_total_bytes`.
    #[must_use]
    pub fn select_by_keywords(&self, memories: &[MemoryFile], context: &str) -> Vec<ScoredMemory> {
        let mut scored: Vec<ScoredMemory> = memories
            .iter()
            .map(|file| ScoredMemory {
                file: file.clone(),
                score: Self::score(file, context),
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut selected: Vec<ScoredMemory> = Vec::new();
        let mut total_bytes = 0usize;

        for entry in scored {
            if selected.len() >= self.max_memories {
                break;
            }
            let entry_bytes = entry.file.body.len() + entry.file.metadata.description.len();
            if total_bytes + entry_bytes > self.max_total_bytes && !selected.is_empty() {
                break;
            }
            total_bytes += entry_bytes;
            selected.push(entry);
        }

        selected
    }

    /// Score a single memory file against `context`.
    ///
    /// With an empty `context`, returns [`type_priority`] as the baseline.
    /// Otherwise combines keyword overlap, type priority, and name-match
    /// bonus into a score in `[0.0, 1.0]`.
    #[must_use]
    pub fn score(entry: &MemoryFile, context: &str) -> f64 {
        if context.is_empty() {
            return type_priority(entry.metadata.memory_type);
        }

        let ctx_lower = context.to_lowercase();
        let ctx_words: Vec<&str> = ctx_lower.split_whitespace().collect();

        // 1. Keyword overlap on (description + body), lowercased.
        let desc = entry.metadata.description.to_lowercase();
        let body = entry.body.to_lowercase();
        let combined = format!("{desc} {body}");

        let matching_words = ctx_words
            .iter()
            .filter(|w| w.len() >= 3 && combined.contains(**w))
            .count();

        let keyword_score = if ctx_words.is_empty() {
            0.0
        } else {
            (matching_words as f64 / ctx_words.len() as f64).min(1.0)
        };

        // 2. Type priority.
        let type_bonus = type_priority(entry.metadata.memory_type);

        // 3. Name-match bonus.
        let name_lower = entry.metadata.name.to_lowercase();
        let name_bonus = if ctx_lower.contains(&name_lower) {
            0.2
        } else {
            0.0
        };

        (keyword_score * 0.6 + type_bonus * 0.3 + name_bonus * 0.1).min(1.0)
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::MemoryMetadata;

    fn make_file(name: &str, description: &str, body: &str, memory_type: MemoryType) -> MemoryFile {
        MemoryFile {
            filename: format!("{name}.md"),
            path: PathBuf::from(format!("/tmp/{name}.md")),
            metadata: MemoryMetadata {
                name: name.to_string(),
                description: description.to_string(),
                memory_type,
                created_at: None,
                updated_at: None,
            },
            body: body.to_string(),
            mtime: None,
        }
    }

    #[test]
    fn default_selector_values() {
        let sel = MemorySelector::default();
        assert_eq!(sel.max_memories, 20);
        assert_eq!(sel.max_total_bytes, 8_000);
    }

    #[test]
    fn score_no_context_returns_type_priority() {
        let entry = make_file("a", "desc", "body", MemoryType::Feedback);
        let score = MemorySelector::score(&entry, "");
        assert!((score - 0.8).abs() < 1e-9);
    }

    #[test]
    fn score_with_keyword_match() {
        let entry = make_file(
            "rust-prefs",
            "User prefers Rust async patterns",
            "Use tokio for async runtime",
            MemoryType::User,
        );
        let score = MemorySelector::score(&entry, "async runtime tokio patterns");
        assert!(score > 0.5, "expected high score, got {score}");
    }

    #[test]
    fn score_no_keyword_match() {
        let entry = make_file(
            "database",
            "Database connection settings",
            "PostgreSQL on port 5432",
            MemoryType::Reference,
        );
        let score = MemorySelector::score(&entry, "frontend react components");
        assert!(score < 0.5, "expected low score, got {score}");
    }

    #[test]
    fn type_priority_ordering() {
        assert!(type_priority(MemoryType::Feedback) > type_priority(MemoryType::Reference));
        assert!(type_priority(MemoryType::User) > type_priority(MemoryType::Project));
        assert!(type_priority(MemoryType::Project) > type_priority(MemoryType::Reference));
    }

    #[test]
    fn select_respects_max_memories() {
        let mem: Vec<MemoryFile> = (0..10)
            .map(|i| make_file(&format!("m{i}"), "desc", "body", MemoryType::User))
            .collect();
        let sel = MemorySelector {
            max_memories: 3,
            max_total_bytes: 100_000,
        };
        let picked = sel.select_by_keywords(&mem, "");
        assert_eq!(picked.len(), 3);
    }

    #[test]
    fn select_respects_byte_budget() {
        // Each entry is ~20 bytes (body=10, desc=10). Budget 35 should fit
        // exactly one (the first), because adding a second (20 + 20 = 40)
        // would exceed 35.
        let mem: Vec<MemoryFile> = (0..5)
            .map(|i| {
                make_file(
                    &format!("m{i}"),
                    "0123456789",
                    "abcdefghij",
                    MemoryType::User,
                )
            })
            .collect();
        let sel = MemorySelector {
            max_memories: 100,
            max_total_bytes: 35,
        };
        let picked = sel.select_by_keywords(&mem, "");
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn select_empty_input() {
        let sel = MemorySelector::default();
        let picked = sel.select_by_keywords(&[], "anything");
        assert!(picked.is_empty());
    }

    #[test]
    fn select_sorts_by_score_descending() {
        let feedback = make_file("fb", "d", "b", MemoryType::Feedback);
        let reference = make_file("rf", "d", "b", MemoryType::Reference);
        let user = make_file("us", "d", "b", MemoryType::User);
        let sel = MemorySelector::default();
        let picked = sel.select_by_keywords(&[reference, user, feedback], "");
        assert_eq!(picked[0].file.metadata.memory_type, MemoryType::Feedback);
        assert_eq!(picked[1].file.metadata.memory_type, MemoryType::User);
        assert_eq!(picked[2].file.metadata.memory_type, MemoryType::Reference);
    }
}
