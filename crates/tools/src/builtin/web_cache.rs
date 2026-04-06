//! URL content cache — LRU cache with configurable TTL for web fetches.
//!
//! `WebCache` stores fetched URL content keyed by normalized URLs, with
//! automatic expiration and eviction tracking via `CacheStats`.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

// ── Types ────────────────────────────────────────────────────────────

/// Default TTL for cache entries (15 minutes).
const DEFAULT_TTL_SECS: u64 = 900;

/// Default maximum number of entries in the cache.
const DEFAULT_MAX_ENTRIES: usize = 256;

/// A single cached entry.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The cached content.
    pub content: String,
    /// When this entry was fetched.
    pub fetched_at: Instant,
    /// Time-to-live for this entry.
    pub ttl: Duration,
    /// MIME content type.
    pub content_type: String,
    /// Size of the content in bytes.
    pub size_bytes: usize,
}

impl CacheEntry {
    /// Whether this entry has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > self.ttl
    }

    /// Remaining TTL, or zero if expired.
    #[must_use]
    pub fn remaining_ttl(&self) -> Duration {
        self.ttl.saturating_sub(self.fetched_at.elapsed())
    }
}

/// Cache statistics.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of entries evicted (LRU or expired).
    pub evictions: u64,
    /// Total size of all cached content in bytes.
    pub total_size: usize,
}

impl fmt::Display for CacheStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[allow(clippy::cast_precision_loss)]
        let hit_rate = if self.hits + self.misses > 0 {
            (self.hits as f64 / (self.hits + self.misses) as f64) * 100.0
        } else {
            0.0
        };
        write!(
            f,
            "hits: {}, misses: {}, evictions: {}, total_size: {} bytes, hit_rate: {hit_rate:.1}%",
            self.hits, self.misses, self.evictions, self.total_size
        )
    }
}

/// LRU web content cache with TTL-based expiration.
pub struct WebCache {
    entries: HashMap<String, CacheEntry>,
    /// Tracks access order for LRU eviction (most recent at end).
    access_order: Vec<String>,
    max_entries: usize,
    default_ttl: Duration,
    stats: CacheStats,
}

impl Default for WebCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_TTL_SECS)
    }
}

impl WebCache {
    /// Create a new cache with the given capacity and default TTL (in seconds).
    #[must_use]
    pub fn new(max_entries: usize, default_ttl_secs: u64) -> Self {
        Self {
            entries: HashMap::new(),
            access_order: Vec::new(),
            max_entries: max_entries.max(1),
            default_ttl: Duration::from_secs(default_ttl_secs),
            stats: CacheStats::default(),
        }
    }

    /// Look up a URL in the cache. Returns `None` on miss or expiry.
    pub fn get(&mut self, url: &str) -> Option<&CacheEntry> {
        let key = cache_key(url);
        // Check expiry first
        if let Some(entry) = self.entries.get(&key) {
            if entry.is_expired() {
                let size = entry.size_bytes;
                self.entries.remove(&key);
                self.access_order.retain(|k| k != &key);
                self.stats.evictions += 1;
                self.stats.total_size = self.stats.total_size.saturating_sub(size);
                self.stats.misses += 1;
                return None;
            }
        } else {
            self.stats.misses += 1;
            return None;
        }

        // Move to end of access_order (most recently used)
        self.access_order.retain(|k| k != &key);
        self.access_order.push(key.clone());
        self.stats.hits += 1;
        self.entries.get(&key)
    }

    /// Insert or update a cache entry.
    pub fn put(&mut self, url: &str, content: String, content_type: String) {
        self.put_with_ttl(url, content, content_type, self.default_ttl);
    }

    /// Insert with a custom TTL.
    pub fn put_with_ttl(
        &mut self,
        url: &str,
        content: String,
        content_type: String,
        ttl: Duration,
    ) {
        let key = cache_key(url);
        let size_bytes = content.len();

        // Remove old entry if exists
        if let Some(old) = self.entries.remove(&key) {
            self.stats.total_size = self.stats.total_size.saturating_sub(old.size_bytes);
            self.access_order.retain(|k| k != &key);
        }

        // Evict LRU entries if at capacity
        while self.entries.len() >= self.max_entries {
            self.evict_lru();
        }

        self.entries.insert(
            key.clone(),
            CacheEntry {
                content,
                fetched_at: Instant::now(),
                ttl,
                content_type,
                size_bytes,
            },
        );
        self.access_order.push(key);
        self.stats.total_size += size_bytes;
    }

    /// Remove all expired entries.
    pub fn cleanup_expired(&mut self) -> usize {
        let expired_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, v)| v.is_expired())
            .map(|(k, _)| k.clone())
            .collect();
        let count = expired_keys.len();
        for key in &expired_keys {
            if let Some(entry) = self.entries.remove(key) {
                self.stats.total_size = self.stats.total_size.saturating_sub(entry.size_bytes);
                self.stats.evictions += 1;
            }
            self.access_order.retain(|k| k != key);
        }
        count
    }

    /// Number of entries currently in cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
        self.stats.total_size = 0;
    }

    /// Get a snapshot of cache statistics.
    #[must_use]
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Evict the least recently used entry.
    fn evict_lru(&mut self) {
        if let Some(key) = self.access_order.first().cloned() {
            if let Some(entry) = self.entries.remove(&key) {
                self.stats.total_size = self.stats.total_size.saturating_sub(entry.size_bytes);
                self.stats.evictions += 1;
            }
            self.access_order.remove(0);
        }
    }
}

/// Normalize a URL into a cache key.
///
/// Strips trailing slashes, lowercases the scheme and host, and removes
/// default ports (80 for HTTP, 443 for HTTPS) and fragment identifiers.
#[must_use]
pub fn cache_key(url: &str) -> String {
    let mut normalized = url.to_owned();

    // Remove fragment
    if let Some(hash_pos) = normalized.find('#') {
        normalized.truncate(hash_pos);
    }

    // Remove trailing slash (but not for bare domain)
    if normalized.ends_with('/')
        && normalized.len() > 8
        && normalized.chars().filter(|&c| c == '/').count() > 2
    {
        normalized.pop();
    }

    // Lowercase scheme and host portion
    if let Some(pos) = normalized.find("://") {
        let (scheme, rest) = normalized.split_at(pos);
        let scheme_lower = scheme.to_lowercase();
        let after_scheme = &rest[3..];
        let host_end = after_scheme
            .find('/')
            .unwrap_or(after_scheme.len());
        let (host, path) = after_scheme.split_at(host_end);
        let host_lower = host.to_lowercase();

        // Strip default ports
        let host_clean = if host_lower.ends_with(":443") && scheme_lower == "https" {
            &host_lower[..host_lower.len() - 4]
        } else if host_lower.ends_with(":80") && scheme_lower == "http" {
            &host_lower[..host_lower.len() - 3]
        } else {
            &host_lower
        };

        normalized = format!("{scheme_lower}://{host_clean}{path}");
    }

    normalized
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_strips_fragment() {
        assert_eq!(
            cache_key("https://example.com/page#section"),
            "https://example.com/page"
        );
    }

    #[test]
    fn cache_key_strips_trailing_slash() {
        assert_eq!(
            cache_key("https://example.com/path/"),
            "https://example.com/path"
        );
    }

    #[test]
    fn cache_key_lowercases_scheme_and_host() {
        assert_eq!(
            cache_key("HTTPS://Example.COM/Path"),
            "https://example.com/Path"
        );
    }

    #[test]
    fn cache_key_strips_default_https_port() {
        assert_eq!(
            cache_key("https://example.com:443/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn cache_key_strips_default_http_port() {
        assert_eq!(
            cache_key("http://example.com:80/path"),
            "http://example.com/path"
        );
    }

    #[test]
    fn cache_key_preserves_non_default_port() {
        assert_eq!(
            cache_key("https://example.com:8080/path"),
            "https://example.com:8080/path"
        );
    }

    #[test]
    fn cache_key_preserves_query_string() {
        assert_eq!(
            cache_key("https://example.com/search?q=rust"),
            "https://example.com/search?q=rust"
        );
    }

    #[test]
    fn new_cache_is_empty() {
        let cache = WebCache::new(10, 60);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn put_and_get() {
        let mut cache = WebCache::new(10, 300);
        cache.put(
            "https://example.com",
            "Hello".into(),
            "text/html".into(),
        );
        assert_eq!(cache.len(), 1);
        let entry = cache.get("https://example.com").unwrap();
        assert_eq!(entry.content, "Hello");
        assert_eq!(entry.content_type, "text/html");
        assert_eq!(entry.size_bytes, 5);
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn get_miss_increments_misses() {
        let mut cache = WebCache::new(10, 300);
        assert!(cache.get("https://notfound.com").is_none());
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn put_overwrites_existing() {
        let mut cache = WebCache::new(10, 300);
        cache.put("https://example.com", "Old".into(), "text/plain".into());
        cache.put("https://example.com", "New".into(), "text/plain".into());
        assert_eq!(cache.len(), 1);
        let entry = cache.get("https://example.com").unwrap();
        assert_eq!(entry.content, "New");
    }

    #[test]
    fn lru_eviction_when_full() {
        let mut cache = WebCache::new(2, 300);
        cache.put("https://a.com", "A".into(), "text/plain".into());
        cache.put("https://b.com", "B".into(), "text/plain".into());
        // Access a.com to make b.com the LRU
        cache.get("https://a.com");
        // Insert c.com, should evict b.com (LRU)
        cache.put("https://c.com", "C".into(), "text/plain".into());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("https://b.com").is_none());
        assert!(cache.get("https://a.com").is_some());
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn expired_entry_returns_none() {
        let mut cache = WebCache::new(10, 300);
        cache.put_with_ttl(
            "https://example.com",
            "Content".into(),
            "text/html".into(),
            Duration::from_secs(0),
        );
        // Entry was created with 0 TTL, so it's already expired
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get("https://example.com").is_none());
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn cleanup_expired_removes_old_entries() {
        let mut cache = WebCache::new(10, 300);
        cache.put_with_ttl(
            "https://expired.com",
            "Old".into(),
            "text/plain".into(),
            Duration::from_secs(0),
        );
        cache.put(
            "https://fresh.com",
            "Fresh".into(),
            "text/plain".into(),
        );
        std::thread::sleep(Duration::from_millis(5));
        let removed = cache.cleanup_expired();
        assert_eq!(removed, 1);
        assert_eq!(cache.len(), 1);
        assert!(cache.get("https://fresh.com").is_some());
    }

    #[test]
    fn clear_empties_cache() {
        let mut cache = WebCache::new(10, 300);
        cache.put("https://a.com", "A".into(), "text/plain".into());
        cache.put("https://b.com", "B".into(), "text/plain".into());
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.stats().total_size, 0);
    }

    #[test]
    fn total_size_tracks_content() {
        let mut cache = WebCache::new(10, 300);
        cache.put("https://a.com", "Hello".into(), "text/plain".into()); // 5 bytes
        cache.put("https://b.com", "World!".into(), "text/plain".into()); // 6 bytes
        assert_eq!(cache.stats().total_size, 11);
    }

    #[test]
    fn cache_entry_remaining_ttl() {
        let entry = CacheEntry {
            content: "test".into(),
            fetched_at: Instant::now(),
            ttl: Duration::from_secs(60),
            content_type: "text/plain".into(),
            size_bytes: 4,
        };
        assert!(!entry.is_expired());
        assert!(entry.remaining_ttl() > Duration::from_secs(50));
    }

    #[test]
    fn cache_stats_display() {
        let stats = CacheStats {
            hits: 10,
            misses: 5,
            evictions: 2,
            total_size: 1024,
        };
        let display = stats.to_string();
        assert!(display.contains("hits: 10"));
        assert!(display.contains("misses: 5"));
        assert!(display.contains("evictions: 2"));
        assert!(display.contains("1024 bytes"));
        assert!(display.contains("66.7%"));
    }

    #[test]
    fn cache_stats_display_zero_requests() {
        let stats = CacheStats::default();
        assert!(stats.to_string().contains("0.0%"));
    }

    #[test]
    fn default_cache_params() {
        let cache = WebCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.max_entries, DEFAULT_MAX_ENTRIES);
    }

    #[test]
    fn max_entries_at_least_one() {
        let cache = WebCache::new(0, 60);
        assert_eq!(cache.max_entries, 1);
    }

    #[test]
    fn normalized_keys_merge() {
        let mut cache = WebCache::new(10, 300);
        cache.put(
            "HTTPS://Example.COM/page",
            "Content".into(),
            "text/html".into(),
        );
        // Same URL, different casing
        let entry = cache.get("https://example.com/page");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().content, "Content");
    }
}
