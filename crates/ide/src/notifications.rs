//! MCP notification handlers.
//!
//! Registers handlers on the MCP client for the two IDE-specific
//! notifications IDE plugins send:
//!
//! - `selection_changed` — ambient state; updates `handles.selection`.
//! - `at_mentioned` — one-shot; fanned out via a broadcast channel.

#![allow(dead_code)] // R1 scaffolding; wired up in R2

use crab_core::ide::{IdeAtMention, IdeSelection};
use serde::Deserialize;

/// `selection_changed` notification params.
///
/// Kept private and `#[serde(rename_all = "camelCase")]` because
/// it mirrors the JS wire shape; the public `IdeSelection` in
/// `crab-core` is the Rust-idiomatic shape that consumers see.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionChangedParams {
    pub line_count: u32,
    #[serde(default)]
    pub line_start: Option<u32>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub file_path: Option<std::path::PathBuf>,
}

impl From<SelectionChangedParams> for IdeSelection {
    fn from(p: SelectionChangedParams) -> Self {
        Self {
            line_count: p.line_count,
            line_start: p.line_start,
            text: p.text,
            file_path: p.file_path,
        }
    }
}

/// `at_mentioned` notification params.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AtMentionedParams {
    pub file_path: std::path::PathBuf,
    #[serde(default)]
    pub line_start: Option<u32>,
    #[serde(default)]
    pub line_end: Option<u32>,
}

impl From<AtMentionedParams> for IdeAtMention {
    fn from(p: AtMentionedParams) -> Self {
        Self {
            file_path: p.file_path,
            line_start: p.line_start,
            line_end: p.line_end,
        }
    }
}

// R2: pub(crate) async fn register(
//         client: &crab_mcp::Client,
//         handles: IdeHandles,
//         tick_tx: broadcast::Sender<()>,
//         mention_tx: broadcast::Sender<IdeAtMention>,
//     ) -> Result<(), crab_mcp::Error>

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_changed_parses_camelcase_wire() {
        let json = r#"{"lineCount":3,"lineStart":10,"text":"ok","filePath":"/x"}"#;
        let p: SelectionChangedParams = serde_json::from_str(json).unwrap();
        let sel: IdeSelection = p.into();
        assert_eq!(sel.line_count, 3);
        assert_eq!(sel.line_start, Some(10));
        assert_eq!(sel.text.as_deref(), Some("ok"));
        assert!(sel.has_file());
    }

    #[test]
    fn selection_changed_parses_minimal_payload() {
        // Plugin may send cleared selection as just `{lineCount: 0}`.
        let json = r#"{"lineCount":0}"#;
        let p: SelectionChangedParams = serde_json::from_str(json).unwrap();
        let sel: IdeSelection = p.into();
        assert_eq!(sel.line_count, 0);
        assert!(!sel.has_text());
        assert!(!sel.has_file());
    }

    #[test]
    fn at_mentioned_parses() {
        let json = r#"{"filePath":"/x","lineStart":1,"lineEnd":5}"#;
        let p: AtMentionedParams = serde_json::from_str(json).unwrap();
        let m: IdeAtMention = p.into();
        assert_eq!(m.line_start, Some(1));
        assert_eq!(m.line_end, Some(5));
    }
}
