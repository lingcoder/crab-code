//! `WebBrowserTool` — Playwright/CDP browser automation.
//!
//! Provides programmatic control of a headless browser for web scraping,
//! testing, and interactive automation. Supports navigation, clicking,
//! typing, screenshots, and closing the browser session.
//!
//! The tool communicates with a browser instance via the Chrome DevTools
//! Protocol (CDP) or Playwright server.

use crab_common::Result;
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

/// Parsed input for the WebBrowser tool.
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

    fn description(&self) -> &str {
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
                .map_err(|e| crab_common::Error::Tool(format!("Invalid input: {e}")))?;

            match parsed.action {
                BrowserAction::Navigate => {
                    let _url = parsed.url.as_deref().unwrap_or_default();
                    todo!("WebBrowser::navigate: launch/reuse browser, navigate to URL")
                }
                BrowserAction::Click => {
                    let _selector = parsed.selector.as_deref().unwrap_or_default();
                    todo!("WebBrowser::click: find element by selector, click it")
                }
                BrowserAction::Type => {
                    let _selector = parsed.selector.as_deref().unwrap_or_default();
                    let _text = parsed.text.as_deref().unwrap_or_default();
                    todo!("WebBrowser::type: find element, type text into it")
                }
                BrowserAction::Screenshot => {
                    todo!("WebBrowser::screenshot: capture viewport, return as base64 image")
                }
                BrowserAction::Close => {
                    todo!("WebBrowser::close: close browser session, release resources")
                }
            }
        })
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
