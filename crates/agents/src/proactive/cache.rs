//! Recent-suggestion memoization — prevents re-proposing what the user
//! already saw or dismissed in the current session.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::suggestion::Suggestion;

/// Default TTL for cached suggestions (5 minutes).
const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// A cache entry: suggestions plus the time they were stored.
#[derive(Debug, Clone)]
struct CacheEntry {
    suggestions: Vec<Suggestion>,
    inserted_at: Instant,
}

/// TTL-based suggestion cache keyed by a context hash.
///
/// The context hash is a caller-defined string (e.g. a hash of the recent
/// conversation) that identifies when the suggestion set is still valid.
#[derive(Debug)]
pub struct SuggestionCache {
    entries: HashMap<String, CacheEntry>,
    ttl: Duration,
}

impl SuggestionCache {
    /// Create a new cache with the default TTL (5 minutes).
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ttl: DEFAULT_TTL,
        }
    }

    /// Create a new cache with a custom TTL.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            ttl,
        }
    }

    /// Look up cached suggestions for the given context hash.
    ///
    /// Returns `None` if no entry exists or the entry has expired.
    pub fn get(&self, context_hash: &str) -> Option<&[Suggestion]> {
        self.entries.get(context_hash).and_then(|entry| {
            if entry.inserted_at.elapsed() < self.ttl {
                Some(entry.suggestions.as_slice())
            } else {
                None
            }
        })
    }

    /// Store suggestions for a given context hash.
    pub fn insert(&mut self, context_hash: String, suggestions: Vec<Suggestion>) {
        self.entries.insert(
            context_hash,
            CacheEntry {
                suggestions,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Evict all expired entries.
    pub fn evict_expired(&mut self) {
        self.entries
            .retain(|_, entry| entry.inserted_at.elapsed() < self.ttl);
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of entries (including potentially expired ones).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SuggestionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::suggestion::ActionType;
    use super::*;

    fn sample_suggestion(title: &str) -> Suggestion {
        Suggestion::new(title, "desc", 0.8, ActionType::Advice)
    }

    #[test]
    fn cache_miss_on_empty() {
        let cache = SuggestionCache::new();
        assert!(cache.get("any_key").is_none());
    }

    #[test]
    fn cache_hit_after_insert() {
        let mut cache = SuggestionCache::new();
        cache.insert("ctx1".into(), vec![sample_suggestion("s1")]);
        let results = cache.get("ctx1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "s1");
    }

    #[test]
    fn cache_miss_on_different_key() {
        let mut cache = SuggestionCache::new();
        cache.insert("ctx1".into(), vec![sample_suggestion("s1")]);
        assert!(cache.get("ctx2").is_none());
    }

    #[test]
    fn cache_expiry() {
        let mut cache = SuggestionCache::with_ttl(Duration::from_millis(50));
        cache.insert("ctx1".into(), vec![sample_suggestion("s1")]);
        assert!(cache.get("ctx1").is_some());

        std::thread::sleep(Duration::from_millis(100));
        assert!(cache.get("ctx1").is_none());
    }

    #[test]
    fn evict_expired_removes_stale() {
        let mut cache = SuggestionCache::with_ttl(Duration::from_millis(50));
        cache.insert("old".into(), vec![sample_suggestion("old")]);
        cache.insert("new".into(), vec![sample_suggestion("new")]);

        std::thread::sleep(Duration::from_millis(100));
        cache.evict_expired();
        assert!(cache.is_empty());
    }

    #[test]
    fn clear_empties_cache() {
        let mut cache = SuggestionCache::new();
        cache.insert("ctx1".into(), vec![sample_suggestion("s1")]);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn len_counts_entries() {
        let mut cache = SuggestionCache::new();
        assert_eq!(cache.len(), 0);
        cache.insert("a".into(), vec![]);
        cache.insert("b".into(), vec![]);
        assert_eq!(cache.len(), 2);
    }
}
