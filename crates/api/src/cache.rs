//! Prompt cache management (Anthropic path only).
//!
//! Anthropic's prompt caching allows reusing previously computed prefixes
//! to reduce latency and cost for repeated system prompts and tool definitions.

/// Cache control directive for message content blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheControl {
    /// Mark this block as a cache breakpoint (ephemeral lifetime).
    Ephemeral,
}

impl CacheControl {
    /// Anthropic API serialization value.
    pub const fn as_type_str(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// Tracks prompt cache hit/miss statistics.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

impl CacheStats {
    pub fn record_read(&mut self, tokens: u64) {
        self.cache_read_tokens += tokens;
    }

    pub fn record_creation(&mut self, tokens: u64) {
        self.cache_creation_tokens += tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_ephemeral_type_str() {
        assert_eq!(CacheControl::Ephemeral.as_type_str(), "ephemeral");
    }

    #[test]
    fn cache_stats_default() {
        let stats = CacheStats::default();
        assert_eq!(stats.cache_read_tokens, 0);
        assert_eq!(stats.cache_creation_tokens, 0);
    }

    #[test]
    fn cache_stats_record_read() {
        let mut stats = CacheStats::default();
        stats.record_read(100);
        stats.record_read(50);
        assert_eq!(stats.cache_read_tokens, 150);
    }

    #[test]
    fn cache_stats_record_creation() {
        let mut stats = CacheStats::default();
        stats.record_creation(200);
        assert_eq!(stats.cache_creation_tokens, 200);
    }
}
