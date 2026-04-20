//! LRU cache keyed by `(content, theme, width)`.
//!
//! The key hashes the input content along with the theme and target
//! width — width matters because line breaks and wrapping in the
//! rendered output depend on it.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use lru::LruCache;
use ratatui::text::Line;

use crate::theme::{Theme, ThemeName};

/// Cache lookup key. Equality and hash treat the three components
/// together; cache misses on any change.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct MarkdownCacheKey {
    content_hash: u64,
    theme: ThemeName,
    width: u16,
}

impl MarkdownCacheKey {
    /// Compute the cache key from the three components.
    #[must_use]
    pub fn compute(content: &str, theme: &Theme, width: u16) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        Self {
            content_hash: h.finish(),
            theme: theme.name,
            width,
        }
    }
}

/// Hit / miss counters for diagnostics.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub capacity: usize,
    pub len: usize,
}

/// LRU cache of rendered Markdown lines.
pub struct MarkdownCache {
    inner: LruCache<MarkdownCacheKey, Arc<Vec<Line<'static>>>>,
    stats: CacheStats,
}

impl MarkdownCache {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = std::num::NonZeroUsize::new(capacity.max(1))
            .expect("capacity is non-zero after max(1)");
        Self {
            inner: LruCache::new(cap),
            stats: CacheStats {
                capacity,
                ..CacheStats::default()
            },
        }
    }

    pub fn get(&mut self, key: &MarkdownCacheKey) -> Option<Arc<Vec<Line<'static>>>> {
        if let Some(v) = self.inner.get(key) {
            self.stats.hits += 1;
            Some(Arc::clone(v))
        } else {
            self.stats.misses += 1;
            None
        }
    }

    pub fn put(&mut self, key: MarkdownCacheKey, value: Arc<Vec<Line<'static>>>) {
        let before_len = self.inner.len();
        self.inner.put(key, value);
        let after_len = self.inner.len();
        if after_len < before_len + 1 && before_len == self.stats.capacity {
            self.stats.evictions += 1;
        }
        self.stats.len = self.inner.len();
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.stats.len = 0;
    }

    pub fn stats(&self) -> CacheStats {
        let mut s = self.stats;
        s.len = self.inner.len();
        s
    }

    pub fn capacity(&self) -> usize {
        self.inner.cap().get()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Arc<Vec<Line<'static>>> {
        Arc::new(vec![Line::raw(s.to_string())])
    }

    #[test]
    fn put_and_get_hit() {
        let theme = Theme::dark();
        let mut cache = MarkdownCache::with_capacity(4);
        let key = MarkdownCacheKey::compute("hello", &theme, 80);
        cache.put(key, line("x"));
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&key).is_some());
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn miss_increments_counter() {
        let theme = Theme::dark();
        let mut cache = MarkdownCache::with_capacity(4);
        let key = MarkdownCacheKey::compute("hi", &theme, 80);
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn different_width_misses_separately() {
        let theme = Theme::dark();
        let mut cache = MarkdownCache::with_capacity(4);
        let k80 = MarkdownCacheKey::compute("same", &theme, 80);
        let k120 = MarkdownCacheKey::compute("same", &theme, 120);
        assert_ne!(k80, k120);
        cache.put(k80, line("a"));
        assert!(cache.get(&k120).is_none());
    }

    #[test]
    fn different_theme_misses_separately() {
        let dark = Theme::dark();
        let light = Theme::light();
        let mut cache = MarkdownCache::with_capacity(4);
        let k_dark = MarkdownCacheKey::compute("same", &dark, 80);
        let k_light = MarkdownCacheKey::compute("same", &light, 80);
        assert_ne!(k_dark, k_light);
        cache.put(k_dark, line("a"));
        assert!(cache.get(&k_light).is_none());
    }

    #[test]
    fn eviction_when_full() {
        let theme = Theme::dark();
        let mut cache = MarkdownCache::with_capacity(2);
        let k1 = MarkdownCacheKey::compute("one", &theme, 80);
        let k2 = MarkdownCacheKey::compute("two", &theme, 80);
        let k3 = MarkdownCacheKey::compute("three", &theme, 80);
        cache.put(k1, line("1"));
        cache.put(k2, line("2"));
        cache.put(k3, line("3"));
        assert!(cache.get(&k1).is_none()); // evicted
        assert!(cache.get(&k2).is_some());
        assert!(cache.get(&k3).is_some());
    }

    #[test]
    fn clear_empties_cache() {
        let theme = Theme::dark();
        let mut cache = MarkdownCache::with_capacity(4);
        cache.put(MarkdownCacheKey::compute("x", &theme, 80), line("x"));
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn capacity_is_honored() {
        let cache = MarkdownCache::with_capacity(10);
        assert_eq!(cache.capacity(), 10);
    }
}
