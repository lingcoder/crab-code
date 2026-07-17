//! Streaming function call parser and parallel tool execution support.
//!
//! `StreamingToolParser` incrementally assembles `tool_use` content blocks
//! from a series of `StreamEvent`s without waiting for the complete JSON.
//! `ParallelToolCollector` detects multiple `tool_use` blocks in a single
//! assistant turn and exposes them for concurrent execution.

use serde_json::Value;

use crate::types::StreamEvent;

/// A `tool_use` block that has been fully assembled from the stream.
///
/// Returned by [`StreamingToolParser::process`] when a `ContentBlockStop`
/// finalizes a tool block, so callers can act on the completed call without
/// peeking back into the parser's internal buffers.
#[derive(Debug, Clone)]
pub struct CompletedToolBlock {
    /// Tool use ID (correlates with the eventual tool result).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Parsed JSON input on success, or the parse error message when the
    /// accumulated buffer was not valid JSON (truncated or malformed stream).
    /// Callers must surface the error as a tool result rather than executing
    /// the tool with degraded input.
    pub input: std::result::Result<Value, String>,
}

/// Tracks the state of a single `tool_use` block being assembled from stream deltas.
#[derive(Debug, Clone)]
pub struct ToolUseAccumulator {
    /// Block index in the stream (from `ContentBlockStart`).
    pub index: usize,
    /// Tool use ID (from `ContentBlockStart` metadata, or generated).
    pub id: String,
    /// Tool name (from `ContentBlockStart` metadata).
    pub name: String,
    /// Accumulated JSON input string (from `ContentDelta` events).
    json_buffer: String,
    /// Whether this block has been finalized (`ContentBlockStop` received).
    pub completed: bool,
}

impl ToolUseAccumulator {
    fn new(index: usize, id: String, name: String) -> Self {
        Self {
            index,
            id,
            name,
            json_buffer: String::new(),
            completed: false,
        }
    }

    /// Append a delta fragment to the JSON input buffer.
    pub fn append_delta(&mut self, delta: &str) {
        self.json_buffer.push_str(delta);
    }

    /// Mark this block as complete.
    pub fn finalize(&mut self) {
        self.completed = true;
    }

    /// Try to parse the accumulated JSON buffer.
    ///
    /// Returns `Some(Value)` if the buffer contains valid JSON, `None` otherwise.
    /// This allows early parsing attempts before `ContentBlockStop`.
    #[must_use]
    pub fn try_parse_input(&self) -> Option<Value> {
        serde_json::from_str(&self.json_buffer).ok()
    }

    /// Return the raw accumulated JSON string.
    #[must_use]
    pub fn raw_json(&self) -> &str {
        &self.json_buffer
    }

    /// Parse the final input JSON. Returns empty object on parse failure.
    #[must_use]
    pub fn parse_input(&self) -> Value {
        serde_json::from_str(&self.json_buffer)
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    }

    /// Parse the final input JSON, preserving the error on failure.
    ///
    /// An empty buffer is treated as an empty object (a tool taking no
    /// arguments). A non-empty buffer that fails to parse yields an `Err`
    /// describing the failure plus a truncated copy of the raw buffer, so the
    /// caller can report it back to the model instead of silently degrading
    /// the input to `{}`.
    ///
    /// # Errors
    ///
    /// Returns the formatted parse error when the buffer is non-empty and not
    /// valid JSON.
    pub fn parse_input_checked(&self) -> std::result::Result<Value, String> {
        if self.json_buffer.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        serde_json::from_str(&self.json_buffer).map_err(|e| {
            let preview: String = self.json_buffer.chars().take(2000).collect();
            format!("tool input JSON was malformed or truncated: {e}; raw input: {preview}")
        })
    }
}

/// Incrementally parses `tool_use` blocks from streaming events.
///
/// Feed each `StreamEvent` into `process()`. When a `tool_use` block completes,
/// it becomes available via `completed_tools()`. Partial tool blocks can be
/// inspected via `in_progress_tools()` for progress reporting.
#[derive(Debug, Default)]
pub struct StreamingToolParser {
    /// Active (in-progress) tool accumulators, keyed by block index.
    active: Vec<ToolUseAccumulator>,
    /// Completed tool accumulators.
    completed: Vec<ToolUseAccumulator>,
    /// Text content accumulated from non-tool blocks.
    text_buffer: String,
    /// Tracks which block indices are text vs `tool_use`.
    block_types: Vec<BlockType>,
}

/// Type of a content block in the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    ToolUse,
    Unknown,
}

impl StreamingToolParser {
    /// Create a new parser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a stream event, updating internal state.
    ///
    /// Returns `Some(CompletedToolBlock)` when this event finalizes a
    /// `tool_use` block (`ContentBlockStop` for a tool index), and `None`
    /// otherwise.
    pub fn process(&mut self, event: &StreamEvent) -> Option<CompletedToolBlock> {
        match event {
            StreamEvent::ContentBlockStart {
                index,
                content_type,
                tool_id,
                tool_name,
            } => {
                while self.block_types.len() <= *index {
                    self.block_types.push(BlockType::Unknown);
                }

                if content_type == "tool_use" {
                    self.block_types[*index] = BlockType::ToolUse;
                    self.active.push(ToolUseAccumulator::new(
                        *index,
                        tool_id.clone().unwrap_or_default(),
                        tool_name.clone().unwrap_or_default(),
                    ));
                } else {
                    self.block_types[*index] = BlockType::Text;
                }
                None
            }
            StreamEvent::ContentDelta { index, delta } => {
                let block_type = self
                    .block_types
                    .get(*index)
                    .copied()
                    .unwrap_or(BlockType::Unknown);

                match block_type {
                    BlockType::ToolUse => {
                        if let Some(acc) = self.active.iter_mut().find(|a| a.index == *index) {
                            acc.append_delta(delta);
                        }
                    }
                    BlockType::Text | BlockType::Unknown => {
                        self.text_buffer.push_str(delta);
                    }
                }
                None
            }
            StreamEvent::ContentBlockStop { index } => {
                let is_tool = self
                    .block_types
                    .get(*index)
                    .copied()
                    .unwrap_or(BlockType::Unknown)
                    == BlockType::ToolUse;

                if is_tool && let Some(pos) = self.active.iter().position(|a| a.index == *index) {
                    let mut acc = self.active.remove(pos);
                    acc.finalize();
                    let block = CompletedToolBlock {
                        id: acc.id.clone(),
                        name: acc.name.clone(),
                        input: acc.parse_input_checked(),
                    };
                    self.completed.push(acc);
                    return Some(block);
                }
                None
            }
            _ => None,
        }
    }

    /// Process a `ContentBlockStart` with explicit tool metadata.
    ///
    /// Call this when the stream provides tool ID and name separately from
    /// the `content_type` string (e.g., Anthropic's native format).
    pub fn set_tool_metadata(&mut self, index: usize, id: String, name: String) {
        if let Some(acc) = self.active.iter_mut().find(|a| a.index == index) {
            acc.id = id;
            acc.name = name;
        }
    }

    /// Whether the block at `index` was declared a `tool_use` block.
    #[must_use]
    pub fn is_tool_block(&self, index: usize) -> bool {
        self.block_types.get(index).copied() == Some(BlockType::ToolUse)
    }

    /// Get all completed tool accumulators (draining the internal buffer).
    pub fn take_completed(&mut self) -> Vec<ToolUseAccumulator> {
        std::mem::take(&mut self.completed)
    }

    /// Peek at completed tools without draining.
    #[must_use]
    pub fn completed_tools(&self) -> &[ToolUseAccumulator] {
        &self.completed
    }

    /// Get in-progress tool accumulators for progress reporting.
    #[must_use]
    pub fn in_progress_tools(&self) -> &[ToolUseAccumulator] {
        &self.active
    }

    /// Get accumulated text content.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text_buffer
    }

    /// Whether any `tool_use` blocks have been detected.
    #[must_use]
    pub fn has_tool_use(&self) -> bool {
        !self.active.is_empty() || !self.completed.is_empty()
    }

    /// Total number of `tool_use` blocks detected (active + completed).
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.active.len() + self.completed.len()
    }

    /// Reset parser state for a new assistant turn.
    pub fn reset(&mut self) {
        self.active.clear();
        self.completed.clear();
        self.text_buffer.clear();
        self.block_types.clear();
    }
}

/// Collects multiple `tool_use` blocks from a single assistant turn for parallel execution.
///
/// After a complete assistant turn (signaled by `MessageStop` or a non-tool-use
/// stop reason), call `into_tool_calls()` to get all tool invocations.
#[derive(Debug, Default)]
pub struct ParallelToolCollector {
    tool_calls: Vec<ToolCall>,
}

/// A parsed tool call ready for execution.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Tool use ID (for correlating results).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Parsed JSON input.
    pub input: Value,
    /// Block index in the original stream.
    pub index: usize,
}

impl ParallelToolCollector {
    /// Create a new collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a completed tool accumulator as a tool call.
    pub fn add(&mut self, acc: &ToolUseAccumulator) {
        self.tool_calls.push(ToolCall {
            id: acc.id.clone(),
            name: acc.name.clone(),
            input: acc.parse_input(),
            index: acc.index,
        });
    }

    /// Add all completed tools from a parser.
    pub fn add_all(&mut self, tools: &[ToolUseAccumulator]) {
        for acc in tools {
            self.add(acc);
        }
    }

    /// Number of collected tool calls.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tool_calls.len()
    }

    /// Whether no tool calls have been collected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_calls.is_empty()
    }

    /// Whether multiple tool calls are available for parallel execution.
    #[must_use]
    pub fn is_parallel(&self) -> bool {
        self.tool_calls.len() > 1
    }

    /// Get tool calls as a slice.
    #[must_use]
    pub fn tool_calls(&self) -> &[ToolCall] {
        &self.tool_calls
    }

    /// Consume and return all tool calls.
    pub fn into_tool_calls(self) -> Vec<ToolCall> {
        self.tool_calls
    }

    /// Reset for the next turn.
    pub fn reset(&mut self) {
        self.tool_calls.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ToolUseAccumulator ───

    #[test]
    fn accumulator_basic() {
        let acc = ToolUseAccumulator::new(0, "tc_1".into(), "read_file".into());
        assert_eq!(acc.index, 0);
        assert_eq!(acc.id, "tc_1");
        assert_eq!(acc.name, "read_file");
        assert!(!acc.completed);
        assert_eq!(acc.raw_json(), "");
    }

    #[test]
    fn accumulator_append_and_parse() {
        let mut acc = ToolUseAccumulator::new(0, "tc_1".into(), "read_file".into());
        acc.append_delta(r#"{"pa"#);
        assert!(acc.try_parse_input().is_none()); // incomplete JSON
        acc.append_delta(r#"th": "/tmp/x"}"#);
        let val = acc.try_parse_input().unwrap();
        assert_eq!(val["path"], "/tmp/x");
    }

    #[test]
    fn accumulator_finalize() {
        let mut acc = ToolUseAccumulator::new(1, "tc_2".into(), "bash".into());
        acc.append_delta(r#"{"cmd": "ls"}"#);
        acc.finalize();
        assert!(acc.completed);
        assert_eq!(acc.parse_input()["cmd"], "ls");
    }

    #[test]
    fn accumulator_parse_input_fallback() {
        let acc = ToolUseAccumulator::new(0, "tc_1".into(), "test".into());
        // Empty buffer parses to empty object
        let val = acc.parse_input();
        assert!(val.is_object());
        assert!(val.as_object().unwrap().is_empty());
    }

    #[test]
    fn accumulator_parse_input_checked_empty_is_ok() {
        let acc = ToolUseAccumulator::new(0, "tc_1".into(), "test".into());
        let val = acc
            .parse_input_checked()
            .expect("empty buffer is empty object");
        assert!(val.as_object().unwrap().is_empty());
    }

    #[test]
    fn accumulator_parse_input_checked_valid() {
        let mut acc = ToolUseAccumulator::new(0, "tc_1".into(), "test".into());
        acc.append_delta(r#"{"path": "/tmp/x"}"#);
        let val = acc.parse_input_checked().unwrap();
        assert_eq!(val["path"], "/tmp/x");
    }

    #[test]
    fn accumulator_parse_input_checked_malformed_errs() {
        let mut acc = ToolUseAccumulator::new(0, "tc_1".into(), "test".into());
        // Truncated mid-stream — not valid JSON.
        acc.append_delta(r#"{"path": "/tmp/x"#);
        let err = acc.parse_input_checked().unwrap_err();
        assert!(err.contains("malformed or truncated"));
        assert!(
            err.contains("/tmp/x"),
            "raw input preview should be included"
        );
    }

    // ─── StreamingToolParser ───

    #[test]
    fn parser_text_only() {
        let mut parser = StreamingToolParser::new();
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: "Hello ".into(),
        });
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: "world".into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 0 });

        assert!(!parser.has_tool_use());
        assert_eq!(parser.text(), "Hello world");
        assert_eq!(parser.tool_count(), 0);
    }

    #[test]
    fn parser_single_tool_use() {
        let mut parser = StreamingToolParser::new();

        // Text block
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: "Let me read that file.".into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 0 });

        // Tool use block
        parser.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(1, "toolu_01".into(), "read_file".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 1,
            delta: r#"{"path":"#.into(),
        });
        parser.process(&StreamEvent::ContentDelta {
            index: 1,
            delta: r#" "/tmp/test.rs"}"#.into(),
        });

        assert!(parser.has_tool_use());
        assert_eq!(parser.in_progress_tools().len(), 1);
        assert_eq!(
            parser.in_progress_tools()[0].raw_json(),
            r#"{"path": "/tmp/test.rs"}"#
        );

        let completed = parser.process(&StreamEvent::ContentBlockStop { index: 1 });
        let block = completed.expect("tool block should complete on ContentBlockStop");
        assert_eq!(block.id, "toolu_01");
        assert_eq!(block.name, "read_file");
        assert_eq!(block.input.as_ref().unwrap()["path"], "/tmp/test.rs");
        assert_eq!(parser.completed_tools().len(), 1);
        assert_eq!(parser.completed_tools()[0].name, "read_file");
        assert_eq!(
            parser.completed_tools()[0].raw_json(),
            r#"{"path": "/tmp/test.rs"}"#
        );
        assert_eq!(
            parser.completed_tools()[0].parse_input()["path"],
            "/tmp/test.rs"
        );
    }

    #[test]
    fn parser_multiple_tool_uses() {
        let mut parser = StreamingToolParser::new();

        // First tool
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(0, "tc_1".into(), "read_file".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#"{"path": "a.rs"}"#.into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 0 });

        // Second tool
        parser.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(1, "tc_2".into(), "read_file".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 1,
            delta: r#"{"path": "b.rs"}"#.into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 1 });

        assert_eq!(parser.tool_count(), 2);
        assert_eq!(parser.completed_tools().len(), 2);
    }

    #[test]
    fn parser_take_completed_drains() {
        let mut parser = StreamingToolParser::new();
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(0, "tc_1".into(), "bash".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#"{"cmd": "ls"}"#.into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 0 });

        let tools = parser.take_completed();
        assert_eq!(tools.len(), 1);
        assert!(parser.completed_tools().is_empty());
    }

    #[test]
    fn parser_is_tool_block() {
        let mut parser = StreamingToolParser::new();
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "thinking".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "text".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.process(&StreamEvent::ContentBlockStart {
            index: 2,
            content_type: "tool_use".into(),
            tool_id: Some("tc_1".into()),
            tool_name: Some("bash".into()),
        });

        assert!(!parser.is_tool_block(0));
        assert!(!parser.is_tool_block(1));
        assert!(parser.is_tool_block(2));
        assert!(!parser.is_tool_block(99));

        // Block type persists after the block completes, so late events
        // (ContentBlockStop) still classify correctly.
        parser.process(&StreamEvent::ContentBlockStop { index: 2 });
        assert!(parser.is_tool_block(2));
    }

    #[test]
    fn parser_reset() {
        let mut parser = StreamingToolParser::new();
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: "hello".into(),
        });
        parser.reset();
        assert_eq!(parser.text(), "");
        assert!(!parser.has_tool_use());
        assert_eq!(parser.tool_count(), 0);
    }

    #[test]
    fn parser_ignores_non_content_events() {
        let mut parser = StreamingToolParser::new();
        assert!(
            parser
                .process(&StreamEvent::MessageStart {
                    id: "m1".into(),
                    usage: crab_core::model::TokenUsage::default(),
                })
                .is_none()
        );
        assert!(
            parser
                .process(&StreamEvent::MessageDelta {
                    usage: crab_core::model::TokenUsage::default(),
                    stop_reason: None,
                })
                .is_none()
        );
        assert!(parser.process(&StreamEvent::MessageStop).is_none());
        assert!(
            parser
                .process(&StreamEvent::Error {
                    message: "err".into(),
                })
                .is_none()
        );
    }

    #[test]
    fn parser_interleaved_text_and_tools() {
        let mut parser = StreamingToolParser::new();

        // Text block
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: "I'll help.".into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 0 });

        // Tool block
        parser.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(1, "tc_1".into(), "bash".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 1,
            delta: "{}".into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 1 });

        assert_eq!(parser.text(), "I'll help.");
        assert_eq!(parser.tool_count(), 1);
    }

    // ─── ParallelToolCollector ───

    #[test]
    fn collector_empty() {
        let collector = ParallelToolCollector::new();
        assert!(collector.is_empty());
        assert!(!collector.is_parallel());
        assert_eq!(collector.len(), 0);
    }

    #[test]
    fn collector_single_tool() {
        let mut collector = ParallelToolCollector::new();
        let mut acc = ToolUseAccumulator::new(0, "tc_1".into(), "read_file".into());
        acc.append_delta(r#"{"path": "/tmp/x"}"#);
        acc.finalize();
        collector.add(&acc);

        assert_eq!(collector.len(), 1);
        assert!(!collector.is_parallel());
        assert_eq!(collector.tool_calls()[0].name, "read_file");
    }

    #[test]
    fn collector_parallel_tools() {
        let mut collector = ParallelToolCollector::new();

        let mut acc1 = ToolUseAccumulator::new(0, "tc_1".into(), "read_file".into());
        acc1.append_delta(r#"{"path": "a.rs"}"#);
        acc1.finalize();

        let mut acc2 = ToolUseAccumulator::new(1, "tc_2".into(), "read_file".into());
        acc2.append_delta(r#"{"path": "b.rs"}"#);
        acc2.finalize();

        collector.add(&acc1);
        collector.add(&acc2);

        assert!(collector.is_parallel());
        assert_eq!(collector.len(), 2);
    }

    #[test]
    fn collector_add_all() {
        let mut collector = ParallelToolCollector::new();
        let mut acc1 = ToolUseAccumulator::new(0, "tc_1".into(), "bash".into());
        acc1.append_delta(r#"{"cmd": "ls"}"#);
        acc1.finalize();
        let mut acc2 = ToolUseAccumulator::new(1, "tc_2".into(), "bash".into());
        acc2.append_delta(r#"{"cmd": "pwd"}"#);
        acc2.finalize();

        collector.add_all(&[acc1, acc2]);
        assert_eq!(collector.len(), 2);
    }

    #[test]
    fn collector_into_tool_calls() {
        let mut collector = ParallelToolCollector::new();
        let mut acc = ToolUseAccumulator::new(0, "tc_1".into(), "glob".into());
        acc.append_delta(r#"{"pattern": "*.rs"}"#);
        acc.finalize();
        collector.add(&acc);

        let calls = collector.into_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tc_1");
        assert_eq!(calls[0].input["pattern"], "*.rs");
    }

    #[test]
    fn collector_reset() {
        let mut collector = ParallelToolCollector::new();
        let mut acc = ToolUseAccumulator::new(0, "tc_1".into(), "bash".into());
        acc.append_delta("{}");
        acc.finalize();
        collector.add(&acc);
        assert_eq!(collector.len(), 1);

        collector.reset();
        assert!(collector.is_empty());
    }

    #[test]
    fn tool_call_fields() {
        let call = ToolCall {
            id: "tc_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
            index: 0,
        };
        assert_eq!(call.id, "tc_1");
        assert_eq!(call.name, "read_file");
        assert_eq!(call.input["path"], "/tmp/x");
        assert_eq!(call.index, 0);
    }

    // ─── Integration: parser → collector ───

    #[test]
    fn parser_to_collector_integration() {
        let mut parser = StreamingToolParser::new();

        // Simulate two tool_use blocks
        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(0, "tc_1".into(), "read_file".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#"{"path": "src/main.rs"}"#.into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 0 });

        parser.process(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(1, "tc_2".into(), "read_file".into());
        parser.process(&StreamEvent::ContentDelta {
            index: 1,
            delta: r#"{"path": "src/lib.rs"}"#.into(),
        });
        parser.process(&StreamEvent::ContentBlockStop { index: 1 });

        // Feed completed tools into collector
        let mut collector = ParallelToolCollector::new();
        collector.add_all(parser.completed_tools());

        assert!(collector.is_parallel());
        let calls = collector.into_tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].input["path"], "src/main.rs");
        assert_eq!(calls[1].input["path"], "src/lib.rs");
    }

    #[test]
    fn parser_incremental_json_parsing() {
        let mut parser = StreamingToolParser::new();

        parser.process(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
            tool_id: None,
            tool_name: None,
        });
        parser.set_tool_metadata(0, "tc_1".into(), "write_file".into());

        // Feed JSON incrementally
        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: "{".into(),
        });
        // Can't parse yet
        assert!(parser.in_progress_tools()[0].try_parse_input().is_none());

        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#""path": "/tmp/out.txt","#.into(),
        });
        // Still incomplete
        assert!(parser.in_progress_tools()[0].try_parse_input().is_none());

        parser.process(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#""content": "hello"}"#.into(),
        });
        // Now it's valid JSON even before ContentBlockStop
        let val = parser.in_progress_tools()[0].try_parse_input().unwrap();
        assert_eq!(val["path"], "/tmp/out.txt");
        assert_eq!(val["content"], "hello");
    }
}
