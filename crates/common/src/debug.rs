use std::path::PathBuf;

/// Known debug categories that map to crate-level tracing targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCategory {
    /// API request/response logging (`crab_api=debug`)
    Api,
    /// Hook execution logging (`crab_agent::hooks=debug`)
    Hooks,
    /// Tool input/output logging (`crab_tools=debug`)
    Tools,
    /// MCP protocol logging (`crab_mcp=debug`)
    Mcp,
}

impl DebugCategory {
    /// Parse a comma-separated category string (e.g. "api,hooks,tools").
    /// Unknown categories are silently ignored.
    pub fn parse_list(s: &str) -> Vec<Self> {
        s.split(',')
            .filter_map(|part| match part.trim() {
                "api" => Some(Self::Api),
                "hooks" => Some(Self::Hooks),
                "tools" => Some(Self::Tools),
                "mcp" => Some(Self::Mcp),
                _ => None,
            })
            .collect()
    }

    /// Convert to the tracing `EnvFilter` directive string.
    pub fn to_filter_directive(self) -> &'static str {
        match self {
            Self::Api => "crab_api=debug",
            Self::Hooks => "crab_agent=debug",
            Self::Tools => "crab_tools=debug",
            Self::Mcp => "crab_mcp=debug",
        }
    }
}

/// Build a tracing `EnvFilter`-compatible string from a list of categories.
///
/// If the list is empty, returns the default "crab=debug" (all modules).
pub fn categories_to_filter(categories: &[DebugCategory]) -> String {
    if categories.is_empty() {
        return "crab=debug".to_string();
    }
    categories
        .iter()
        .map(|c| c.to_filter_directive())
        .collect::<Vec<_>>()
        .join(",")
}

/// Resolve the debug filter string from the raw `-d` flag value.
///
/// - `None` → debug is disabled
/// - `Some("")` → all categories (returns "crab=debug")
/// - `Some("api,hooks")` → category-based filter directives
/// - `Some("crab_api=trace")` → pass through as-is (advanced usage)
pub fn resolve_debug_filter(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    if raw.is_empty() {
        return Some("crab=debug".to_string());
    }
    // If it contains '=' it's already a tracing directive — pass through
    if raw.contains('=') {
        return Some(raw.to_string());
    }
    // Otherwise, parse as category list
    let categories = DebugCategory::parse_list(raw);
    if categories.is_empty() {
        // Unknown categories — fall back to default
        Some("crab=debug".to_string())
    } else {
        Some(categories_to_filter(&categories))
    }
}

/// Configuration for the debug/tracing subsystem.
#[derive(Debug, Clone, Default)]
pub struct DebugConfig {
    /// Whether debug output is enabled at all.
    pub enabled: bool,
    /// Optional filter string (e.g. "api", "hooks,tools") passed to `EnvFilter`.
    /// When `None` and `enabled` is true, defaults to "crab=debug".
    pub filter: Option<String>,
    /// Optional file path to write debug logs to (in addition to stderr).
    pub file: Option<PathBuf>,
}

/// Initialise the global tracing subscriber based on `config`.
///
/// Call this once, early in `main`. If `config.enabled` is false, this is a
/// no-op and the default (no-op) subscriber remains active.
pub fn init_debug(config: &DebugConfig) {
    if !config.enabled {
        return;
    }

    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

    let filter_str = config
        .filter
        .as_deref()
        .unwrap_or("crab=debug");

    let env_filter = EnvFilter::try_new(filter_str)
        .unwrap_or_else(|_| EnvFilter::new("crab=debug"));

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true);

    if let Some(ref file_path) = config.file {
        let dir = file_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let filename = file_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("crab-debug.log"));
        let file_appender = tracing_appender::rolling::never(dir, filename);
        let file_layer = fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false)
            .with_target(true)
            .with_file(true)
            .with_line_number(true);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_config_default_is_disabled() {
        let cfg = DebugConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.filter.is_none());
        assert!(cfg.file.is_none());
    }

    #[test]
    fn init_debug_noop_when_disabled() {
        // Should not panic
        init_debug(&DebugConfig::default());
    }

    // ── DebugCategory tests ──

    #[test]
    fn parse_category_list_single() {
        let cats = DebugCategory::parse_list("api");
        assert_eq!(cats, vec![DebugCategory::Api]);
    }

    #[test]
    fn parse_category_list_multiple() {
        let cats = DebugCategory::parse_list("api,hooks,tools,mcp");
        assert_eq!(
            cats,
            vec![
                DebugCategory::Api,
                DebugCategory::Hooks,
                DebugCategory::Tools,
                DebugCategory::Mcp,
            ]
        );
    }

    #[test]
    fn parse_category_list_unknown_ignored() {
        let cats = DebugCategory::parse_list("api,unknown,tools");
        assert_eq!(cats, vec![DebugCategory::Api, DebugCategory::Tools]);
    }

    #[test]
    fn parse_category_list_empty() {
        let cats = DebugCategory::parse_list("");
        assert!(cats.is_empty());
    }

    #[test]
    fn category_to_filter_directive() {
        assert_eq!(DebugCategory::Api.to_filter_directive(), "crab_api=debug");
        assert_eq!(DebugCategory::Hooks.to_filter_directive(), "crab_agent=debug");
        assert_eq!(DebugCategory::Tools.to_filter_directive(), "crab_tools=debug");
        assert_eq!(DebugCategory::Mcp.to_filter_directive(), "crab_mcp=debug");
    }

    #[test]
    fn categories_to_filter_empty_is_default() {
        assert_eq!(categories_to_filter(&[]), "crab=debug");
    }

    #[test]
    fn categories_to_filter_single() {
        assert_eq!(
            categories_to_filter(&[DebugCategory::Api]),
            "crab_api=debug"
        );
    }

    #[test]
    fn categories_to_filter_multiple() {
        assert_eq!(
            categories_to_filter(&[DebugCategory::Api, DebugCategory::Mcp]),
            "crab_api=debug,crab_mcp=debug"
        );
    }

    // ── resolve_debug_filter tests ──

    #[test]
    fn resolve_debug_filter_none_is_disabled() {
        assert!(resolve_debug_filter(None).is_none());
    }

    #[test]
    fn resolve_debug_filter_empty_is_default() {
        assert_eq!(resolve_debug_filter(Some("")), Some("crab=debug".into()));
    }

    #[test]
    fn resolve_debug_filter_categories() {
        assert_eq!(
            resolve_debug_filter(Some("api,tools")),
            Some("crab_api=debug,crab_tools=debug".into())
        );
    }

    #[test]
    fn resolve_debug_filter_passthrough_tracing_directive() {
        assert_eq!(
            resolve_debug_filter(Some("crab_api=trace")),
            Some("crab_api=trace".into())
        );
    }

    #[test]
    fn resolve_debug_filter_unknown_categories_fallback() {
        assert_eq!(
            resolve_debug_filter(Some("foobar")),
            Some("crab=debug".into())
        );
    }
}
