//! `WebBrowserTool` — Playwright/CDP browser automation.
//!
//! Provides programmatic control of a headless browser for web scraping,
//! testing, and interactive automation. Supports navigation, clicking,
//! typing, screenshots, and closing the browser session.
//!
//! The tool communicates with a browser instance via the Chrome `DevTools`
//! Protocol (CDP) or Playwright server.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `WebBrowserTool`.
pub const WEB_BROWSER_TOOL_NAME: &str = "WebBrowser";

// ── Input types ───────────────────────────────────────────────────────

/// The action to perform in the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    /// Navigate to a URL.
    Navigate,
    /// Click an element matching a selector.
    Click,
    /// Type text into an element matching a selector.
    Type,
    /// Take a screenshot of the current page.
    Screenshot,
    /// Close the browser session.
    Close,
}

/// Parsed input for the `WebBrowser` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebBrowserInput {
    /// The browser action to perform.
    pub action: BrowserAction,
    /// URL to navigate to (required for `navigate`).
    #[serde(default)]
    pub url: Option<String>,
    /// CSS selector for `click` and `type` actions.
    #[serde(default)]
    pub selector: Option<String>,
    /// Text to type (required for `type` action).
    #[serde(default)]
    pub text: Option<String>,
}

// ── Tool implementation ───────────────────────────────────────────────

/// Browser automation tool using Playwright/CDP.
///
/// Input schema:
/// ```json
/// {
///   "action": "navigate" | "click" | "type" | "screenshot" | "close",
///   "url": "<optional URL>",
///   "selector": "<optional CSS selector>",
///   "text": "<optional text to type>"
/// }
/// ```
pub struct WebBrowserTool;

impl Tool for WebBrowserTool {
    fn name(&self) -> &str {
        WEB_BROWSER_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Control a headless browser for web automation. Supports navigating to URLs, \
         clicking elements, typing text, taking screenshots, and closing the session."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "type", "screenshot", "close"],
                    "description": "The browser action to perform"
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (required for navigate action)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for click/type actions"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type (required for type action)"
                }
            },
            "required": ["action"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let parsed: WebBrowserInput = serde_json::from_value(input)
                .map_err(|e| crab_core::Error::Tool(format!("Invalid input: {e}")))?;

            // All browser actions require Playwright MCP integration which
            // is not yet wired up. Return a descriptive error instead of
            // panicking so the agent loop can recover gracefully.
            let action_label = match parsed.action {
                BrowserAction::Navigate => "navigate",
                BrowserAction::Click => "click",
                BrowserAction::Type => "type",
                BrowserAction::Screenshot => "screenshot",
                BrowserAction::Close => "close",
            };
            Ok(ToolOutput::error(format!(
                "Web browser action '{action_label}' requires Playwright MCP \
                 integration which is not yet available. Consider adding a \
                 Playwright MCP server to your .crab/settings.json instead."
            )))
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let action = input["action"].as_str().unwrap_or("?");
        Some(format!("WebBrowser ({action})"))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_metadata() {
        let tool = WebBrowserTool;
        assert_eq!(tool.name(), "WebBrowser");
        assert!(!tool.description().is_empty());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn schema_has_action_enum() {
        let schema = WebBrowserTool.input_schema();
        assert_eq!(schema["required"], json!(["action"]));
        let action_enum = &schema["properties"]["action"]["enum"];
        assert!(action_enum.is_array());
        assert_eq!(action_enum.as_array().unwrap().len(), 5);
    }

    #[test]
    fn browser_action_serde() {
        let action = BrowserAction::Navigate;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"navigate\"");
        let parsed: BrowserAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, BrowserAction::Navigate));
    }

    #[test]
    fn input_parse() {
        let input: WebBrowserInput = serde_json::from_value(json!({
            "action": "navigate",
            "url": "https://example.com"
        }))
        .unwrap();
        assert!(matches!(input.action, BrowserAction::Navigate));
        assert_eq!(input.url.as_deref(), Some("https://example.com"));
        assert!(input.selector.is_none());
    }
}
