//! MCP notification handlers.
//!
//! Parses the two IDE-specific notifications plugins send:
//!
//! - `selection_changed` — ambient state; written to `handles.selection`.
//! - `at_mentioned` — one-shot; fanned out via a broadcast channel.
//!
//! The dispatch loop lives in [`run_dispatch_loop`] and is owned by the
//! [`crate::client::IdeClient`] reconnect task.

use crab_core::ide::{IdeAtMention, IdeSelection};
use crab_mcp::protocol::JsonRpcNotification;
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};

use crate::state::IdeHandles;

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

/// Drain notifications from `rx` until the channel closes, routing each
/// to the appropriate sink.
///
/// Returns when the WebSocket reader task exits (transport close, read
/// error, or server-initiated close). Callers should then attempt
/// reconnection.
pub(crate) async fn run_dispatch_loop(
    mut rx: mpsc::UnboundedReceiver<JsonRpcNotification>,
    handles: IdeHandles,
    mention_tx: broadcast::Sender<IdeAtMention>,
) {
    while let Some(notif) = rx.recv().await {
        dispatch_one(notif, &handles, &mention_tx).await;
    }
    tracing::debug!("IDE notification dispatch loop exiting");
}

/// Route a single notification. Unknown methods are logged and dropped.
async fn dispatch_one(
    notif: JsonRpcNotification,
    handles: &IdeHandles,
    mention_tx: &broadcast::Sender<IdeAtMention>,
) {
    let Some(params) = notif.params else {
        tracing::trace!(method = %notif.method, "ignoring parameterless IDE notification");
        return;
    };
    match notif.method.as_str() {
        "selection_changed" => match serde_json::from_value::<SelectionChangedParams>(params) {
            Ok(p) => {
                let selection: IdeSelection = p.into();
                *handles.selection.write().await = Some(selection);
            }
            Err(e) => tracing::warn!(error = %e, "malformed selection_changed params"),
        },
        "at_mentioned" => match serde_json::from_value::<AtMentionedParams>(params) {
            Ok(p) => {
                let mention: IdeAtMention = p.into();
                // Receivers may have lagged or been dropped — broadcast.send
                // returning Err means "no active subscribers"; that's fine.
                let _ = mention_tx.send(mention);
            }
            Err(e) => tracing::warn!(error = %e, "malformed at_mentioned params"),
        },
        other => tracing::trace!(method = other, "ignoring unhandled IDE notification"),
    }
}

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

    #[tokio::test]
    async fn dispatch_updates_selection_handle() {
        let handles = IdeHandles::default();
        let (mention_tx, _) = broadcast::channel(4);
        let notif = JsonRpcNotification::new(
            "selection_changed".to_string(),
            Some(serde_json::json!({"lineCount":2,"lineStart":1,"text":"hi","filePath":"/a"})),
        );
        dispatch_one(notif, &handles, &mention_tx).await;
        let sel = handles.selection.read().await.clone().unwrap();
        assert_eq!(sel.line_count, 2);
        assert_eq!(sel.text.as_deref(), Some("hi"));
    }

    #[tokio::test]
    async fn dispatch_broadcasts_at_mention() {
        let handles = IdeHandles::default();
        let (mention_tx, mut rx) = broadcast::channel(4);
        let notif = JsonRpcNotification::new(
            "at_mentioned".to_string(),
            Some(serde_json::json!({"filePath":"/a","lineStart":1,"lineEnd":3})),
        );
        dispatch_one(notif, &handles, &mention_tx).await;
        let mention = rx.recv().await.unwrap();
        assert_eq!(mention.line_end, Some(3));
    }

    #[tokio::test]
    async fn dispatch_ignores_unknown_method() {
        let handles = IdeHandles::default();
        let (mention_tx, _rx) = broadcast::channel(4);
        let notif =
            JsonRpcNotification::new("something_else".to_string(), Some(serde_json::json!({})));
        dispatch_one(notif, &handles, &mention_tx).await;
        assert!(handles.selection.read().await.is_none());
    }

    #[tokio::test]
    async fn dispatch_tolerates_malformed_payload() {
        let handles = IdeHandles::default();
        let (mention_tx, _rx) = broadcast::channel(4);
        let notif = JsonRpcNotification::new(
            "selection_changed".to_string(),
            Some(serde_json::json!({"lineCount":"not-a-number"})),
        );
        // Must not panic.
        dispatch_one(notif, &handles, &mention_tx).await;
        assert!(handles.selection.read().await.is_none());
    }
}
