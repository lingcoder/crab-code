//! IDE integration shared types.
//!
//! These types are the public data contract between:
//!
//! - `crab-ide` — owns the MCP client that receives notifications and
//!   writes into the shared state backed by these types.
//! - `crab-tui` — reads `IdeSelection` / `IdeConnection` to render the
//!   "⧉ N lines foo.py" status indicator.
//! - `crab-agent` — reads `IdeSelection` at prompt submit time to
//!   build `<system-reminder>` injection.
//!
//! Keeping them in `crab-core` avoids same-layer dependencies between
//! `crab-tui` and `crab-ide` (both Layer 2).
//!
//! ## Wire schema alignment
//!
//! Field names mirror the upstream `selection_changed` MCP notification
//! payload so the deserializer is trivial, and preserve the "opened file
//! but nothing selected" semantics: `line_count == 0 && file_path.is_some()`.
//!
//! The injection template in `crab-ide` also mirrors the upstream prose
//! to keep LLM prompt patterns familiar.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// IDE selection state. Mirrors MCP `selection_changed` notification.
///
/// All fields except `line_count` are optional because the plugin sends
/// a "cleared" notification when the user deselects — leaving only a
/// cursor position, no text, no range.
///
/// When `line_count == 0` but `file_path` is `Some`, the user has the
/// file open but nothing selected. Consumers should fall back to
/// "opened file" semantics in that case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeSelection {
    /// Number of selected lines. `0` means cursor-only (no range).
    pub line_count: u32,
    /// 1-based start line of the selection, if any.
    pub line_start: Option<u32>,
    /// Selected text verbatim. May be absent even with `line_count > 0`
    /// if the plugin elected not to transmit large selections.
    pub text: Option<String>,
    /// Absolute path of the file the selection is in.
    pub file_path: Option<PathBuf>,
}

impl IdeSelection {
    /// True if there is a non-empty text selection.
    pub fn has_text(&self) -> bool {
        self.line_count > 0 && self.text.is_some()
    }

    /// True if a file is associated (even without text selected).
    pub fn has_file(&self) -> bool {
        self.file_path.is_some()
    }

    /// Inclusive end line, computed from `line_start + line_count - 1`.
    /// Returns `None` if there is no range (`line_count` == 0 or
    /// `line_start` absent).
    pub fn line_end(&self) -> Option<u32> {
        if self.line_count == 0 {
            return None;
        }
        self.line_start.map(|s| s + self.line_count - 1)
    }
}

/// One-shot IDE → CLI push triggered by the user (e.g. right-click
/// "Send to Crab" in `JetBrains`). Unlike `IdeSelection` which is
/// ambient state, a mention is consumed once and dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeAtMention {
    pub file_path: PathBuf,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
}

/// IDE connection metadata — established at handshake, mostly used by
/// the TUI to label the status indicator ("⧉ … · `IntelliJ`").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeConnection {
    /// IDE product name, e.g. "`IntelliJ` IDEA", "`VSCode`", "`PyCharm`".
    pub ide_name: String,
    /// Workspace roots the IDE reports as currently open.
    pub workspace_folders: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selection_is_empty() {
        let sel = IdeSelection::default();
        assert_eq!(sel.line_count, 0);
        assert!(!sel.has_text());
        assert!(!sel.has_file());
        assert_eq!(sel.line_end(), None);
    }

    #[test]
    fn selection_with_file_no_text_signals_opened_file() {
        let sel = IdeSelection {
            line_count: 0,
            file_path: Some(PathBuf::from("/work/foo.rs")),
            ..Default::default()
        };
        assert!(!sel.has_text());
        assert!(sel.has_file());
    }

    #[test]
    fn line_end_is_inclusive() {
        let sel = IdeSelection {
            line_count: 11,
            line_start: Some(10),
            ..Default::default()
        };
        // lines 10..=20 → 11 lines → end is 20
        assert_eq!(sel.line_end(), Some(20));
    }

    #[test]
    fn line_end_single_line() {
        let sel = IdeSelection {
            line_count: 1,
            line_start: Some(42),
            ..Default::default()
        };
        assert_eq!(sel.line_end(), Some(42));
    }

    #[test]
    fn selection_roundtrips_through_json() {
        let sel = IdeSelection {
            line_count: 3,
            line_start: Some(10),
            text: Some("fn main() {}".to_string()),
            file_path: Some(PathBuf::from("/work/foo.rs")),
        };
        let json = serde_json::to_string(&sel).unwrap();
        let back: IdeSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(sel, back);
    }

    #[test]
    fn selection_parses_mcp_wire_format() {
        // Shape matches the upstream `selection_changed` MCP notification params.
        let json = r#"{
            "line_count": 11,
            "line_start": 10,
            "text": "fn bar() {}",
            "file_path": "/work/foo.rs"
        }"#;
        let sel: IdeSelection = serde_json::from_str(json).unwrap();
        assert!(sel.has_text());
        assert_eq!(sel.line_end(), Some(20));
    }

    #[test]
    fn at_mention_roundtrips() {
        let m = IdeAtMention {
            file_path: PathBuf::from("/work/x.rs"),
            line_start: Some(1),
            line_end: Some(5),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: IdeAtMention = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn connection_roundtrips() {
        let c = IdeConnection {
            ide_name: "IntelliJ IDEA".to_string(),
            workspace_folders: vec![PathBuf::from("/work")],
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: IdeConnection = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
