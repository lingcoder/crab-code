//! Fuzzy matching wrapper around `nucleo-matcher`.
//!
//! Central place for all fuzzy matching in the TUI. Owns a reusable
//! `nucleo_matcher::Matcher` and a scratch `Vec<char>` buffer so callers
//! don't pay the construction cost per keystroke.
//!
//! The underlying crate is the same fuzzy matcher used by the Helix editor
//! and the Nucleo picker, so match quality tracks Helix's.
//!
//! # Usage
//!
//! ```ignore
//! let mut fuzzy = FuzzyMatcher::new();
//! let items = vec!["open_file", "new_file", "undo"];
//! let ranked = fuzzy.match_and_rank(&items, "of", |s| s);
//! // ranked is sorted best-score first, zero-score items filtered out
//! ```

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A reusable fuzzy matcher backed by `nucleo-matcher`.
///
/// Construct once, keep around, and reuse across queries. The internal
/// `Matcher` and scratch char buffer are cheap to reuse but non-trivial
/// to reconstruct on every keystroke.
pub struct FuzzyMatcher {
    inner: Matcher,
    /// Scratch buffer used to segment UTF-8 strings into codepoints for
    /// `Utf32Str::new`. Held on the struct so we don't reallocate on
    /// every match call.
    scratch: Vec<char>,
}

impl FuzzyMatcher {
    /// Create a new fuzzy matcher with the default nucleo config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Matcher::new(Config::DEFAULT),
            scratch: Vec::new(),
        }
    }

    /// Match and rank `items` against `query`, returning `(item, score)`
    /// pairs in descending score order.
    ///
    /// `get_label` extracts the string to match against from each item
    /// (e.g. `|cmd| cmd.label.as_str()` for a `Command`).
    ///
    /// Empty query is a pass-through: returns every item paired with
    /// score `0` in original order. This matches the "show all" behaviour
    /// most pickers want when the query is empty.
    ///
    /// Non-matching items are filtered out. The returned vector only
    /// contains items whose label fuzzy-matched the query with a non-zero
    /// score.
    pub fn match_and_rank<'a, T, F>(
        &mut self,
        items: &'a [T],
        query: &str,
        get_label: F,
    ) -> Vec<(&'a T, u32)>
    where
        F: Fn(&T) -> &str,
    {
        if query.is_empty() {
            return items.iter().map(|item| (item, 0u32)).collect();
        }

        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

        let mut results: Vec<(&'a T, u32)> = items
            .iter()
            .filter_map(|item| {
                let label = get_label(item);
                self.scratch.clear();
                let haystack = Utf32Str::new(label, &mut self.scratch);
                pattern
                    .score(haystack, &mut self.inner)
                    .map(|score| (item, score))
            })
            .collect();

        // Sort by score descending. Stable sort preserves input order for
        // equal-score items, which is what users expect from a picker.
        results.sort_by(|a, b| b.1.cmp(&a.1));
        results
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_prefix_match() {
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["hello", "world", "help"];
        let ranked = fuzzy.match_and_rank(&items, "hel", |s| s);

        // Both "hello" and "help" should match; "world" must not.
        assert_eq!(ranked.len(), 2);
        let matched: Vec<&&str> = ranked.iter().map(|(s, _)| *s).collect();
        assert!(matched.iter().any(|s| **s == "hello"));
        assert!(matched.iter().any(|s| **s == "help"));
        assert!(!matched.iter().any(|s| **s == "world"));
    }

    #[test]
    fn camel_case_skip_match() {
        // Query "tz" should match "toggle_transcript" (t from toggle,
        // then later ts — nucleo handles this via word-boundary bonus).
        // More canonical camelCase: query "tsb" matches "toggle_sidebar".
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["toggle_sidebar", "new_file", "undo"];
        let ranked = fuzzy.match_and_rank(&items, "tsb", |s| s);
        assert!(!ranked.is_empty(), "expected 'tsb' to match toggle_sidebar");
        assert_eq!(*ranked[0].0, "toggle_sidebar");
    }

    #[test]
    fn case_insensitive_smart_match() {
        // With CaseMatching::Smart, all-lowercase query matches both
        // cases. An uppercase query requires uppercase — but Smart mode
        // treats mixed/upper queries as case-sensitive. For all-lowercase
        // haystacks an upper query won't match, so use a mixed item.
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["search_history", "SearchHistory", "other"];
        let ranked = fuzzy.match_and_rank(&items, "search", |s| s);
        // Lowercase query → smart mode is case-insensitive → both hit.
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn empty_query_returns_all_in_order() {
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["a", "b", "c"];
        let ranked = fuzzy.match_and_rank(&items, "", |s| s);
        assert_eq!(ranked.len(), 3);
        assert_eq!(*ranked[0].0, "a");
        assert_eq!(*ranked[1].0, "b");
        assert_eq!(*ranked[2].0, "c");
        // All scores are 0 for empty query.
        assert!(ranked.iter().all(|(_, score)| *score == 0));
    }

    #[test]
    fn multi_byte_query_and_items() {
        // Emoji + CJK — ensure Utf32Str buffer handles non-ASCII without
        // panicking or byte-slicing.
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["打开文件", "新建文件", "help 帮助"];
        let ranked = fuzzy.match_and_rank(&items, "文件", |s| s);
        assert_eq!(ranked.len(), 2, "expected 2 hits for '文件'");

        // Emoji haystack shouldn't panic either.
        let items2 = vec!["🦀 crab", "🐙 octopus", "fox"];
        let ranked2 = fuzzy.match_and_rank(&items2, "crab", |s| s);
        assert_eq!(ranked2.len(), 1);
        assert_eq!(*ranked2[0].0, "🦀 crab");
    }

    #[test]
    fn score_ordering_closer_ranks_higher() {
        // "undo" should score higher for query "und" than "fund raiser"
        // would (prefix + contiguous + word-start bonus). A non-word-start
        // hit of the same length should rank below.
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["refund", "undo", "fundament"];
        let ranked = fuzzy.match_and_rank(&items, "und", |s| s);
        assert!(!ranked.is_empty());
        // "undo" is a word-start match with contiguous chars — should win.
        assert_eq!(*ranked[0].0, "undo", "undo should rank first: {ranked:?}");
    }

    #[test]
    fn non_matching_items_filtered() {
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["apple", "banana", "cherry"];
        let ranked = fuzzy.match_and_rank(&items, "zzz", |s| s);
        assert!(ranked.is_empty());
    }

    #[test]
    fn get_label_closure_applied() {
        // Exercise the closure: pair items with labels that aren't the
        // items themselves.
        struct Cmd {
            id: &'static str,
            label: &'static str,
        }
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec![
            Cmd {
                id: "a",
                label: "New File",
            },
            Cmd {
                id: "b",
                label: "Open File",
            },
            Cmd {
                id: "c",
                label: "Undo",
            },
        ];
        let ranked = fuzzy.match_and_rank(&items, "file", |c| c.label);
        assert_eq!(ranked.len(), 2);
        let ids: Vec<&str> = ranked.iter().map(|(c, _)| c.id).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
        assert!(!ids.contains(&"c"));
    }

    #[test]
    fn default_matches_new() {
        // Trivial: Default impl should be usable the same way.
        let mut fuzzy = FuzzyMatcher::default();
        let items = vec!["foo", "bar"];
        let ranked = fuzzy.match_and_rank(&items, "foo", |s| s);
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn reuse_across_queries() {
        // The same matcher must be safe to hammer across queries without
        // corruption of the scratch buffer.
        let mut fuzzy = FuzzyMatcher::new();
        let items = vec!["alpha", "beta", "gamma"];

        let r1 = fuzzy.match_and_rank(&items, "alp", |s| s);
        assert_eq!(r1.len(), 1);
        assert_eq!(*r1[0].0, "alpha");

        let r2 = fuzzy.match_and_rank(&items, "gam", |s| s);
        assert_eq!(r2.len(), 1);
        assert_eq!(*r2[0].0, "gamma");

        let r3 = fuzzy.match_and_rank(&items, "", |s| s);
        assert_eq!(r3.len(), 3);
    }
}
