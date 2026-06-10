//! Web search tool — searches the web and returns results.
//!
//! Provides structured `SearchResult` types, domain filtering, date range
//! filtering, search result caching (15-minute TTL), and search history
//! for deduplication. Supports Brave Search, Google Custom Search, and
//! Serper.dev via environment variables (`CRAB_SEARCH_API` +
//! `CRAB_SEARCH_API_KEY`). Falls back to `DuckDuckGo` Instant Answer API
//! when no paid API is configured.

use crab_core::Result;
use crab_core::tool::{CollapsedGroupLabel, Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

/// Maximum number of results to return.
const DEFAULT_MAX_RESULTS: u64 = 10;

/// Default TTL for search result cache (15 minutes).
const SEARCH_CACHE_TTL_SECS: u64 = 900;

// ── Structured search result types ───────────────────────────────────

/// A single search result with structured fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// Title of the result page.
    pub title: String,
    /// URL of the result.
    pub url: String,
    /// Snippet / summary text.
    pub snippet: String,
    /// Optional published date (ISO 8601 or human-readable).
    pub published_date: Option<String>,
}

impl SearchResult {
    /// Convert to a JSON `Value`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut obj = serde_json::json!({
            "title": self.title,
            "url": self.url,
            "snippet": self.snippet,
        });
        if let Some(date) = &self.published_date {
            obj["published_date"] = Value::String(date.clone());
        }
        obj
    }
}

/// Domain filter for search results.
#[derive(Debug, Clone, Default)]
pub struct DomainFilter {
    /// Only include results from these domains (if non-empty).
    pub allowed: Vec<String>,
    /// Exclude results from these domains.
    pub blocked: Vec<String>,
}

impl DomainFilter {
    /// Check whether a URL passes this filter.
    #[must_use]
    pub fn allows(&self, url: &str) -> bool {
        let host = extract_host(url);
        if !self.blocked.is_empty() && self.blocked.iter().any(|d| host_matches(&host, d)) {
            return false;
        }
        if !self.allowed.is_empty() {
            return self.allowed.iter().any(|d| host_matches(&host, d));
        }
        true
    }
}

/// Extract the host portion from a URL.
fn extract_host(url: &str) -> String {
    let after_scheme = url.find("://").map_or(url, |pos| &url[pos + 3..]);
    let host = after_scheme.split('/').next().unwrap_or("");
    host.split(':').next().unwrap_or("").to_lowercase()
}

/// Check if a host matches a domain filter (supports subdomain matching).
fn host_matches(host: &str, domain: &str) -> bool {
    let d = domain.to_lowercase();
    host == d || host.ends_with(&format!(".{d}"))
}

/// Date range filter for search results.
#[derive(Debug, Clone)]
pub struct DateRange {
    /// Start date (inclusive), format "YYYY-MM-DD".
    pub from: Option<String>,
    /// End date (inclusive), format "YYYY-MM-DD".
    pub to: Option<String>,
}

// ── Search result cache ──────────────────────────────────────────────

/// Cached search results for a query.
#[derive(Debug, Clone)]
struct CachedSearch {
    results: Vec<SearchResult>,
    cached_at: Instant,
}

/// In-memory cache for search results, keyed by normalized query string.
pub struct SearchCache {
    entries: HashMap<String, CachedSearch>,
    ttl: Duration,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::new(SEARCH_CACHE_TTL_SECS)
    }
}

impl SearchCache {
    /// Create with a given TTL in seconds.
    #[must_use]
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            entries: HashMap::new(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Look up cached results for a query. Returns `None` on miss/expiry.
    pub fn get(&mut self, query: &str) -> Option<&[SearchResult]> {
        let key = normalize_query(query);
        // Check for expiry and remove if needed
        let is_expired = self
            .entries
            .get(&key)
            .is_none_or(|e| e.cached_at.elapsed() > self.ttl);
        if is_expired {
            self.entries.remove(&key);
            return None;
        }
        self.entries.get(&key).map(|e| e.results.as_slice())
    }

    /// Store results for a query.
    pub fn put(&mut self, query: &str, results: Vec<SearchResult>) {
        let key = normalize_query(query);
        self.entries.insert(
            key,
            CachedSearch {
                results,
                cached_at: Instant::now(),
            },
        );
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all expired entries.
    pub fn cleanup(&mut self) -> usize {
        let before = self.entries.len();
        let ttl = self.ttl;
        self.entries.retain(|_, v| v.cached_at.elapsed() <= ttl);
        before - self.entries.len()
    }
}

/// Normalize a search query for cache keying (trim + lowercase).
fn normalize_query(query: &str) -> String {
    query.trim().to_lowercase()
}

// ── Search history ───────────────────────────────────────────────────

/// A record of a past search.
#[derive(Debug, Clone)]
pub struct SearchHistoryEntry {
    /// The original query.
    pub query: String,
    /// When the search was performed.
    pub searched_at: Instant,
    /// Number of results returned.
    pub result_count: usize,
}

/// Tracks search history for deduplication and analytics.
pub struct SearchHistory {
    entries: Vec<SearchHistoryEntry>,
    max_entries: usize,
}

impl Default for SearchHistory {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl SearchHistory {
    /// Create with a maximum entry count.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    /// Record a search.
    pub fn record(&mut self, query: &str, result_count: usize) {
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push(SearchHistoryEntry {
            query: query.to_owned(),
            searched_at: Instant::now(),
            result_count,
        });
    }

    /// Check if a query was searched recently (within the given duration).
    #[must_use]
    pub fn was_recently_searched(&self, query: &str, within: Duration) -> bool {
        let normalized = normalize_query(query);
        self.entries
            .iter()
            .rev()
            .any(|e| normalize_query(&e.query) == normalized && e.searched_at.elapsed() <= within)
    }

    /// Number of recorded searches.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether history is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all entries.
    #[must_use]
    pub fn entries(&self) -> &[SearchHistoryEntry] {
        &self.entries
    }
}

/// Web search tool.
pub const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        WEB_SEARCH_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Search the web for up-to-date information. Returns search results with \
         titles, URLs, and snippets. Use this for questions about recent events, \
         documentation lookups, or anything beyond your training data."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to use"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 20)"
                },
                "allowed_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Only include results from these domains"
                },
                "blocked_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Exclude results from these domains"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let query = input["query"].as_str().unwrap_or("").to_owned();
        #[allow(clippy::cast_possible_truncation)]
        let max_results = input["max_results"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(20) as usize;
        let allowed_domains = parse_string_array(&input["allowed_domains"]);
        let blocked_domains = parse_string_array(&input["blocked_domains"]);

        Box::pin(async move {
            if query.is_empty() {
                return Ok(ToolOutput::error("query is required and must be non-empty"));
            }

            // Try real search via configured API, fall back to informative message
            match search_via_api(&query, max_results, &allowed_domains, &blocked_domains).await {
                Ok(results) => {
                    let json =
                        serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());
                    Ok(ToolOutput::success(format!(
                        "Search results for \"{query}\":\n\n{json}"
                    )))
                }
                Err(reason) => {
                    tracing::warn!(query = %query, reason = %reason, "search failed");
                    Ok(ToolOutput::error(format!(
                        "Web search failed for \"{query}\": {reason}\n\n\
                         To enable web search, set these environment variables:\n\
                           CRAB_SEARCH_API=brave|google|serper\n\
                           CRAB_SEARCH_API_KEY=<your-api-key>\n\n\
                         Without a paid API, DuckDuckGo is used automatically but may \
                         return limited results."
                    )))
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let query = input["query"].as_str()?;
        Some(format!("WebSearch (\"{query}\")"))
    }

    fn collapsed_group_label(&self) -> Option<CollapsedGroupLabel> {
        Some(CollapsedGroupLabel {
            active_verb: "Searching",
            past_verb: "Searched",
            noun_singular: "query",
            noun_plural: "queries",
        })
    }
}

/// Parse a JSON array of strings into a `Vec<String>`.
fn parse_string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Attempt to search via a configured search API or `DuckDuckGo` fallback.
///
/// Checks `CRAB_SEARCH_API` and `CRAB_SEARCH_API_KEY` env vars for a paid
/// provider (brave / google / serper). If neither var is set, falls back to
/// the `DuckDuckGo` Instant Answer API which requires no key. Returns `Err`
/// with a reason string when all backends fail.
async fn search_via_api(
    query: &str,
    max_results: usize,
    allowed_domains: &[String],
    blocked_domains: &[String],
) -> std::result::Result<Value, String> {
    let filter = DomainFilter {
        allowed: allowed_domains.to_vec(),
        blocked: blocked_domains.to_vec(),
    };

    // Try paid API first
    if let Ok(api) = std::env::var("CRAB_SEARCH_API") {
        let key = std::env::var("CRAB_SEARCH_API_KEY")
            .map_err(|_| "CRAB_SEARCH_API_KEY env var is not set".to_string())?;
        tracing::info!(provider = %api, "using configured search API");
        return match api.to_lowercase().as_str() {
            "brave" => search_brave(query, max_results, &key, &filter).await,
            "google" => search_google(query, max_results, &key, &filter).await,
            "serper" => search_serper(query, max_results, &key, &filter).await,
            other => Err(format!(
                "unknown CRAB_SEARCH_API value '{other}' — expected brave, google, or serper"
            )),
        };
    }

    // Fall back to DuckDuckGo Instant Answer API (no key required)
    tracing::debug!("no paid search API configured, trying DuckDuckGo");
    search_duckduckgo(query, max_results, &filter)
        .await
        .map_err(|e| format!("DuckDuckGo fallback failed: {e}"))
}

// ── Brave Search API ─────────────────────────────────────────────────

async fn search_brave(
    query: &str,
    max_results: usize,
    api_key: &str,
    filter: &DomainFilter,
) -> std::result::Result<Value, String> {
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
        urlencoding::encode(query),
        max_results
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "gzip")
        .header("X-Subscription-Token", api_key)
        .send()
        .await
        .map_err(|e| format!("Brave Search request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Brave Search returned HTTP {status}: {body}"));
    }

    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse Brave Search response: {e}"))?;

    let results = json
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .ok_or_else(|| "unexpected Brave Search response structure".to_string())?;

    let mut search_results: Vec<Value> = results
        .iter()
        .filter_map(|r| {
            let url = r.get("url")?.as_str()?;
            if !filter.allows(url) {
                return None;
            }
            Some(
                SearchResult {
                    title: r.get("title")?.as_str().unwrap_or("").to_owned(),
                    url: url.to_owned(),
                    snippet: r.get("description")?.as_str().unwrap_or("").to_owned(),
                    published_date: r.get("age").and_then(|v| v.as_str()).map(String::from),
                }
                .to_json(),
            )
        })
        .collect();

    search_results.truncate(max_results);
    Ok(Value::Array(search_results))
}

// ── Google Custom Search API ─────────────────────────────────────────

async fn search_google(
    query: &str,
    max_results: usize,
    api_key: &str,
    filter: &DomainFilter,
) -> std::result::Result<Value, String> {
    let cx = std::env::var("CRAB_SEARCH_GOOGLE_CX")
        .map_err(|_| "CRAB_SEARCH_GOOGLE_CX env var is required for Google search".to_string())?;

    let url = format!(
        "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}&num={}",
        api_key,
        cx,
        urlencoding::encode(query),
        max_results.min(10) // Google API max 10 per request
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Google Search request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Google Search returned HTTP {status}: {body}"));
    }

    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse Google Search response: {e}"))?;

    let items = json
        .get("items")
        .and_then(|i| i.as_array())
        .ok_or_else(|| "no results from Google Search".to_string())?;

    let mut search_results: Vec<Value> = items
        .iter()
        .filter_map(|r| {
            let url = r.get("link")?.as_str()?;
            if !filter.allows(url) {
                return None;
            }
            Some(
                SearchResult {
                    title: r.get("title")?.as_str().unwrap_or("").to_owned(),
                    url: url.to_owned(),
                    snippet: r
                        .get("snippet")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_owned(),
                    published_date: None,
                }
                .to_json(),
            )
        })
        .collect();

    search_results.truncate(max_results);
    Ok(Value::Array(search_results))
}

// ── Serper.dev API ───────────────────────────────────────────────────

async fn search_serper(
    query: &str,
    max_results: usize,
    api_key: &str,
    filter: &DomainFilter,
) -> std::result::Result<Value, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", api_key)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "q": query,
            "num": max_results,
        }))
        .send()
        .await
        .map_err(|e| format!("Serper request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Serper returned HTTP {status}: {body}"));
    }

    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse Serper response: {e}"))?;

    let organic = json
        .get("organic")
        .and_then(|o| o.as_array())
        .ok_or_else(|| "unexpected Serper response structure".to_string())?;

    let mut search_results: Vec<Value> = organic
        .iter()
        .filter_map(|r| {
            let url = r.get("link")?.as_str()?;
            if !filter.allows(url) {
                return None;
            }
            Some(
                SearchResult {
                    title: r.get("title")?.as_str().unwrap_or("").to_owned(),
                    url: url.to_owned(),
                    snippet: r
                        .get("snippet")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_owned(),
                    published_date: r.get("date").and_then(|d| d.as_str()).map(String::from),
                }
                .to_json(),
            )
        })
        .collect();

    search_results.truncate(max_results);
    Ok(Value::Array(search_results))
}

// ── DuckDuckGo Instant Answer API (keyless fallback) ────────────────

async fn search_duckduckgo(
    query: &str,
    max_results: usize,
    filter: &DomainFilter,
) -> std::result::Result<Value, String> {
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding::encode(query)
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let resp = client
        .get(&url)
        .header("User-Agent", "crab-code/0.1.0")
        .send()
        .await
        .map_err(|e| format!("DuckDuckGo request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("DuckDuckGo returned HTTP {status}"));
    }

    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse DuckDuckGo response: {e}"))?;

    let mut results: Vec<Value> = Vec::new();

    // Extract the abstract (main answer) if present
    if let Some(abstract_text) = json.get("AbstractText").and_then(|v| v.as_str())
        && !abstract_text.is_empty()
        && let Some(abstract_url) = json.get("AbstractURL").and_then(|v| v.as_str())
        && filter.allows(abstract_url)
    {
        let heading = json
            .get("Heading")
            .and_then(|v| v.as_str())
            .unwrap_or(query);
        results.push(
            SearchResult {
                title: heading.to_owned(),
                url: abstract_url.to_owned(),
                snippet: abstract_text.to_owned(),
                published_date: None,
            }
            .to_json(),
        );
    }

    // Extract related topics
    if let Some(topics) = json.get("RelatedTopics").and_then(|v| v.as_array()) {
        for topic in topics {
            if results.len() >= max_results {
                break;
            }
            // DuckDuckGo related topics can be nested (groups with sub-topics)
            if let Some(sub_topics) = topic.get("Topics").and_then(|v| v.as_array()) {
                for sub in sub_topics {
                    if results.len() >= max_results {
                        break;
                    }
                    if let Some(result) = ddg_topic_to_result(sub, filter) {
                        results.push(result);
                    }
                }
            } else if let Some(result) = ddg_topic_to_result(topic, filter) {
                results.push(result);
            }
        }
    }

    if results.is_empty() {
        return Err("DuckDuckGo returned no results for this query".to_string());
    }

    results.truncate(max_results);
    Ok(Value::Array(results))
}

/// Convert a `DuckDuckGo` related topic JSON object into a `SearchResult` JSON value.
fn ddg_topic_to_result(topic: &Value, filter: &DomainFilter) -> Option<Value> {
    let text = topic.get("Text")?.as_str()?;
    let first_url = topic.get("FirstURL")?.as_str()?;
    if !filter.allows(first_url) {
        return None;
    }
    Some(
        SearchResult {
            title: text.chars().take(80).collect(),
            url: first_url.to_owned(),
            snippet: text.to_owned(),
            published_date: None,
        }
        .to_json(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::tool::ToolContext;
    use std::sync::Arc;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            task_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = WebSearchTool;
        assert_eq!(tool.name(), "WebSearch");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn input_schema_has_required_query() {
        let schema = WebSearchTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("query")));
    }

    #[test]
    fn input_schema_has_optional_fields() {
        let schema = WebSearchTool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("max_results"));
        assert!(props.contains_key("allowed_domains"));
        assert!(props.contains_key("blocked_domains"));
    }

    #[tokio::test]
    async fn execute_empty_query_returns_error() {
        let tool = WebSearchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(serde_json::json!({"query": ""}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn execute_valid_query_uses_search_backend() {
        let tool = WebSearchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(serde_json::json!({"query": "rust programming"}), &ctx)
            .await
            .unwrap();
        // Without API keys or network, this will either succeed (DDG) or
        // return a clear error — both are valid. Never fake results.
        let text = result.text();
        if result.is_error {
            assert!(text.contains("Web search failed"));
        } else {
            assert!(text.contains("rust programming"));
        }
    }

    #[test]
    fn parse_string_array_valid() {
        let val = serde_json::json!(["a.com", "b.com"]);
        let result = parse_string_array(&val);
        assert_eq!(result, vec!["a.com", "b.com"]);
    }

    #[test]
    fn parse_string_array_null() {
        let result = parse_string_array(&Value::Null);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_string_array_mixed() {
        let val = serde_json::json!(["valid.com", 42, "also.com"]);
        let result = parse_string_array(&val);
        assert_eq!(result, vec!["valid.com", "also.com"]);
    }

    #[test]
    fn ddg_topic_to_result_none_without_url() {
        let topic = serde_json::json!({"Text": "some text"});
        assert!(ddg_topic_to_result(&topic, &DomainFilter::default()).is_none());
    }

    #[test]
    fn ddg_topic_to_result_none_without_text() {
        let topic = serde_json::json!({"FirstURL": "https://example.com"});
        assert!(ddg_topic_to_result(&topic, &DomainFilter::default()).is_none());
    }

    #[test]
    fn ddg_topic_to_result_valid() {
        let topic = serde_json::json!({
            "Text": "Rust is a multi-paradigm programming language",
            "FirstURL": "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        });
        let result = ddg_topic_to_result(&topic, &DomainFilter::default()).unwrap();
        assert_eq!(
            result["url"],
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert!(result["snippet"].as_str().unwrap().contains("Rust"));
    }

    // ── SearchResult tests ──

    #[test]
    fn search_result_to_json() {
        let r = SearchResult {
            title: "Test".into(),
            url: "https://example.com".into(),
            snippet: "A snippet".into(),
            published_date: Some("2024-01-01".into()),
        };
        let json = r.to_json();
        assert_eq!(json["title"], "Test");
        assert_eq!(json["url"], "https://example.com");
        assert_eq!(json["published_date"], "2024-01-01");
    }

    #[test]
    fn search_result_to_json_no_date() {
        let r = SearchResult {
            title: "No date".into(),
            url: "https://example.com".into(),
            snippet: "Text".into(),
            published_date: None,
        };
        let json = r.to_json();
        assert!(json.get("published_date").is_none());
    }

    // ── DomainFilter tests ──

    #[test]
    fn domain_filter_allows_all_by_default() {
        let filter = DomainFilter::default();
        assert!(filter.allows("https://example.com/page"));
        assert!(filter.allows("https://other.com"));
    }

    #[test]
    fn domain_filter_allowed_list() {
        let filter = DomainFilter {
            allowed: vec!["example.com".into()],
            blocked: vec![],
        };
        assert!(filter.allows("https://example.com/page"));
        assert!(filter.allows("https://sub.example.com/page"));
        assert!(!filter.allows("https://other.com/page"));
    }

    #[test]
    fn domain_filter_blocked_list() {
        let filter = DomainFilter {
            allowed: vec![],
            blocked: vec!["spam.com".into()],
        };
        assert!(filter.allows("https://example.com/page"));
        assert!(!filter.allows("https://spam.com/page"));
        assert!(!filter.allows("https://sub.spam.com/page"));
    }

    #[test]
    fn domain_filter_blocked_overrides_allowed() {
        let filter = DomainFilter {
            allowed: vec!["example.com".into()],
            blocked: vec!["example.com".into()],
        };
        assert!(!filter.allows("https://example.com/page"));
    }

    // ── extract_host tests ──

    #[test]
    fn extract_host_basic() {
        assert_eq!(extract_host("https://Example.COM/path"), "example.com");
        assert_eq!(extract_host("http://localhost:8080/api"), "localhost");
    }

    // ── host_matches tests ──

    #[test]
    fn host_matches_exact() {
        assert!(host_matches("example.com", "example.com"));
        assert!(!host_matches("example.com", "other.com"));
    }

    #[test]
    fn host_matches_subdomain() {
        assert!(host_matches("sub.example.com", "example.com"));
        assert!(!host_matches("example.com", "sub.example.com"));
    }

    // ── SearchCache tests ──

    #[test]
    fn search_cache_put_and_get() {
        let mut cache = SearchCache::new(300);
        let results = vec![SearchResult {
            title: "Test".into(),
            url: "https://test.com".into(),
            snippet: "Snippet".into(),
            published_date: None,
        }];
        cache.put("rust programming", results);
        assert_eq!(cache.len(), 1);
        let cached = cache.get("rust programming").unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].title, "Test");
    }

    #[test]
    fn search_cache_normalized_key() {
        let mut cache = SearchCache::new(300);
        cache.put("Rust Programming", vec![]);
        // Same query different case should hit
        assert!(cache.get("rust programming").is_some());
        assert!(cache.get("  Rust Programming  ").is_some());
    }

    #[test]
    fn search_cache_miss() {
        let mut cache = SearchCache::new(300);
        assert!(cache.get("not cached").is_none());
    }

    #[test]
    fn search_cache_expired() {
        let mut cache = SearchCache::new(0); // 0 second TTL
        cache.put("test", vec![]);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(cache.get("test").is_none());
    }

    #[test]
    fn search_cache_cleanup() {
        let mut cache = SearchCache::new(0);
        cache.put("a", vec![]);
        cache.put("b", vec![]);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let removed = cache.cleanup();
        assert_eq!(removed, 2);
        assert!(cache.is_empty());
    }

    #[test]
    fn search_cache_default() {
        let cache = SearchCache::default();
        assert!(cache.is_empty());
    }

    // ── SearchHistory tests ──

    #[test]
    fn search_history_record_and_check() {
        let mut history = SearchHistory::new(100);
        assert!(history.is_empty());
        history.record("rust async", 10);
        assert_eq!(history.len(), 1);
        assert!(history.was_recently_searched("rust async", Duration::from_secs(60)));
        assert!(!history.was_recently_searched("python", Duration::from_secs(60)));
    }

    #[test]
    fn search_history_case_insensitive() {
        let mut history = SearchHistory::new(100);
        history.record("Rust Async", 5);
        assert!(history.was_recently_searched("rust async", Duration::from_secs(60)));
    }

    #[test]
    fn search_history_respects_max() {
        let mut history = SearchHistory::new(2);
        history.record("first", 1);
        history.record("second", 2);
        history.record("third", 3);
        assert_eq!(history.len(), 2);
        // First should have been evicted
        assert!(!history.was_recently_searched("first", Duration::from_secs(60)));
    }

    #[test]
    fn search_history_entries_accessor() {
        let mut history = SearchHistory::new(100);
        history.record("test query", 5);
        let entries = history.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].query, "test query");
        assert_eq!(entries[0].result_count, 5);
    }

    #[test]
    fn search_history_default() {
        let history = SearchHistory::default();
        assert!(history.is_empty());
    }

    #[test]
    fn normalize_query_trims_and_lowercases() {
        assert_eq!(normalize_query("  Hello World  "), "hello world");
    }
}
