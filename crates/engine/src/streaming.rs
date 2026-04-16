//! Streaming tool executor — starts tool execution as soon as
//! a `tool_use` block's JSON is fully parsed during SSE streaming.
//!
//! Extracted from `query_loop.rs` for separation of concerns.

use crab_core::tool::{ToolContext, ToolOutput};

/// Streaming tool executor — spawns tool tasks eagerly as inputs complete.
pub struct StreamingToolExecutor {
    pub pending: Vec<tokio::task::JoinHandle<(String, crab_common::Result<ToolOutput>)>>,
}

impl StreamingToolExecutor {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Spawn a tool execution as soon as its input JSON is complete.
    pub fn spawn(
        &mut self,
        _id: &str,
        name: String,
        input: serde_json::Value,
        ctx: ToolContext,
        tool_fn: impl FnOnce(
            String,
            serde_json::Value,
            ToolContext,
        )
            -> tokio::task::JoinHandle<(String, crab_common::Result<ToolOutput>)>,
    ) {
        let handle = tool_fn(name, input, ctx);
        self.pending.push(handle);
    }

    /// Collect all pending tool results after `message_stop`.
    pub async fn collect_all(&mut self) -> Vec<(String, crab_common::Result<ToolOutput>)> {
        let mut results = Vec::new();
        for handle in self.pending.drain(..) {
            results.push(handle.await.expect("tool task panicked"));
        }
        results
    }
}

impl Default for StreamingToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}
