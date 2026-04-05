//! Streaming response aggregator.
//!
//! `StreamAggregator` consumes `StreamEvent` chunks and assembles a complete
//! response — text content, tool calls, and performance metrics.

use std::time::{Duration, Instant};

use crate::streaming::ToolCall;
use crate::types::StreamEvent;

// ---------------------------------------------------------------------------
// StreamMetrics
// ---------------------------------------------------------------------------

/// Performance metrics collected during stream aggregation.
#[derive(Debug, Clone)]
pub struct StreamMetrics {
    /// Time from stream start to first content token (ms).
    pub first_token_ms: Option<u64>,
    /// Total stream duration (ms).
    pub total_ms: u64,
    /// Number of content delta chunks received.
    pub chunk_count: u32,
    /// Total tokens reported by the final usage event.
    pub total_tokens: u64,
}

// ---------------------------------------------------------------------------
// StallDetector
// ---------------------------------------------------------------------------

/// Detects when a stream has stalled (no events for a configured duration).
#[derive(Debug)]
struct StallDetector {
    timeout: Duration,
    last_event: Instant,
}

impl StallDetector {
    fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            last_event: Instant::now(),
        }
    }

    fn record_event(&mut self) {
        self.last_event = Instant::now();
    }

    fn is_stalled(&self) -> bool {
        !self.timeout.is_zero() && self.last_event.elapsed() >= self.timeout
    }
}

// ---------------------------------------------------------------------------
// StreamAggregator
// ---------------------------------------------------------------------------

/// Aggregates SSE stream chunks into a complete response.
#[derive(Debug)]
pub struct StreamAggregator {
    /// Accumulated text content.
    text_buffer: String,
    /// Accumulated tool call chunks, keyed by block index.
    tool_buffers: Vec<ToolCallBuffer>,
    /// Stream start time.
    start_time: Option<Instant>,
    /// Time of first content token.
    first_token_time: Option<Instant>,
    /// Number of content delta chunks.
    chunk_count: u32,
    /// Final token usage from `MessageDelta`.
    total_tokens: u64,
    /// Stop reason from `MessageDelta`.
    stop_reason: Option<String>,
    /// Message ID from `MessageStart`.
    message_id: Option<String>,
    /// Stall detector.
    stall_detector: StallDetector,
    /// Whether the stream is complete (`MessageStop` received).
    complete: bool,
}

/// In-progress tool call being assembled from stream chunks.
#[derive(Debug, Clone)]
struct ToolCallBuffer {
    index: usize,
    id: String,
    name: String,
    json_buffer: String,
    completed: bool,
}

impl StreamAggregator {
    /// Create a new aggregator with the given stall timeout.
    #[must_use]
    pub fn new(stall_timeout: Duration) -> Self {
        Self {
            text_buffer: String::new(),
            tool_buffers: Vec::new(),
            start_time: None,
            first_token_time: None,
            chunk_count: 0,
            total_tokens: 0,
            stop_reason: None,
            message_id: None,
            stall_detector: StallDetector::new(stall_timeout),
            complete: false,
        }
    }

    /// Create an aggregator with a 30-second default stall timeout.
    #[must_use]
    pub fn with_default_timeout() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Feed a stream event into the aggregator.
    ///
    /// Returns `true` if the stream is complete after this event.
    pub fn feed(&mut self, event: &StreamEvent) -> bool {
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
        self.stall_detector.record_event();

        match event {
            StreamEvent::MessageStart { id, usage } => {
                self.message_id = Some(id.clone());
                self.total_tokens = usage.total();
            }
            StreamEvent::ContentBlockStart {
                index,
                content_type,
            } => {
                if content_type == "tool_use" {
                    self.tool_buffers.push(ToolCallBuffer {
                        index: *index,
                        id: String::new(),
                        name: String::new(),
                        json_buffer: String::new(),
                        completed: false,
                    });
                }
            }
            StreamEvent::ContentDelta { index, delta } => {
                self.chunk_count += 1;
                if self.first_token_time.is_none() {
                    self.first_token_time = Some(Instant::now());
                }

                if let Some(buf) = self.tool_buffers.iter_mut().find(|b| b.index == *index) {
                    buf.json_buffer.push_str(delta);
                } else {
                    self.text_buffer.push_str(delta);
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                if let Some(buf) = self.tool_buffers.iter_mut().find(|b| b.index == *index) {
                    buf.completed = true;
                }
            }
            StreamEvent::MessageDelta { usage, stop_reason } => {
                self.total_tokens = usage.total();
                self.stop_reason.clone_from(stop_reason);
            }
            StreamEvent::MessageStop => {
                self.complete = true;
            }
            StreamEvent::Error { .. } => {
                self.complete = true;
            }
        }

        self.complete
    }

    /// Set tool metadata for a block index (id and name).
    pub fn set_tool_metadata(&mut self, index: usize, id: String, name: String) {
        if let Some(buf) = self.tool_buffers.iter_mut().find(|b| b.index == index) {
            buf.id = id;
            buf.name = name;
        }
    }

    /// Aggregate all accumulated text into a single string.
    #[must_use]
    pub fn aggregate_text(&self) -> String {
        self.text_buffer.clone()
    }

    /// Extract completed tool calls from the stream.
    #[must_use]
    pub fn aggregate_tool_calls(&self) -> Vec<ToolCall> {
        self.tool_buffers
            .iter()
            .filter(|b| b.completed)
            .map(|b| ToolCall {
                id: b.id.clone(),
                name: b.name.clone(),
                input: serde_json::from_str(&b.json_buffer)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
                index: b.index,
            })
            .collect()
    }

    /// Get stream performance metrics.
    #[must_use]
    pub fn metrics(&self) -> StreamMetrics {
        let total_ms = self
            .start_time
            .map_or(0, |t| t.elapsed().as_millis() as u64);
        let first_token_ms = self
            .first_token_time
            .zip(self.start_time)
            .map(|(ft, st)| ft.duration_since(st).as_millis() as u64);

        StreamMetrics {
            first_token_ms,
            total_ms,
            chunk_count: self.chunk_count,
            total_tokens: self.total_tokens,
        }
    }

    /// Whether the stream has stalled (no events within the timeout).
    #[must_use]
    pub fn is_stalled(&self) -> bool {
        self.stall_detector.is_stalled()
    }

    /// Whether the stream is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The stop reason, if any.
    #[must_use]
    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    /// The message ID, if received.
    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    /// Reset the aggregator for reuse.
    pub fn reset(&mut self) {
        self.text_buffer.clear();
        self.tool_buffers.clear();
        self.start_time = None;
        self.first_token_time = None;
        self.chunk_count = 0;
        self.total_tokens = 0;
        self.stop_reason = None;
        self.message_id = None;
        self.complete = false;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::model::TokenUsage;

    fn usage(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }
    }

    #[test]
    fn aggregate_text_only() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::MessageStart {
            id: "msg_1".into(),
            usage: usage(10, 0),
        });
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
        });
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: "Hello ".into(),
        });
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: "world!".into(),
        });
        agg.feed(&StreamEvent::ContentBlockStop { index: 0 });
        agg.feed(&StreamEvent::MessageDelta {
            usage: usage(10, 5),
            stop_reason: Some("end_turn".into()),
        });
        agg.feed(&StreamEvent::MessageStop);

        assert_eq!(agg.aggregate_text(), "Hello world!");
        assert!(agg.aggregate_tool_calls().is_empty());
        assert!(agg.is_complete());
        assert_eq!(agg.stop_reason(), Some("end_turn"));
        assert_eq!(agg.message_id(), Some("msg_1"));
    }

    #[test]
    fn aggregate_tool_calls() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::MessageStart {
            id: "msg_2".into(),
            usage: usage(0, 0),
        });
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
        });
        agg.set_tool_metadata(0, "tc_1".into(), "read_file".into());
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#"{"path": "/tmp/x"}"#.into(),
        });
        agg.feed(&StreamEvent::ContentBlockStop { index: 0 });
        agg.feed(&StreamEvent::MessageStop);

        let tools = agg.aggregate_tool_calls();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[0].input["path"], "/tmp/x");
    }

    #[test]
    fn aggregate_mixed_text_and_tools() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::MessageStart {
            id: "msg_3".into(),
            usage: usage(0, 0),
        });
        // Text block
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
        });
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: "Let me read that.".into(),
        });
        agg.feed(&StreamEvent::ContentBlockStop { index: 0 });
        // Tool block
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "tool_use".into(),
        });
        agg.set_tool_metadata(1, "tc_1".into(), "bash".into());
        agg.feed(&StreamEvent::ContentDelta {
            index: 1,
            delta: r#"{"cmd": "ls"}"#.into(),
        });
        agg.feed(&StreamEvent::ContentBlockStop { index: 1 });
        agg.feed(&StreamEvent::MessageStop);

        assert_eq!(agg.aggregate_text(), "Let me read that.");
        assert_eq!(agg.aggregate_tool_calls().len(), 1);
    }

    #[test]
    fn metrics_chunk_count() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::MessageStart {
            id: "m".into(),
            usage: usage(0, 0),
        });
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
        });
        for i in 0..5 {
            agg.feed(&StreamEvent::ContentDelta {
                index: 0,
                delta: format!("chunk{i}"),
            });
        }
        agg.feed(&StreamEvent::ContentBlockStop { index: 0 });
        agg.feed(&StreamEvent::MessageDelta {
            usage: usage(20, 10),
            stop_reason: None,
        });

        let m = agg.metrics();
        assert_eq!(m.chunk_count, 5);
        assert_eq!(m.total_tokens, 30);
        assert!(m.first_token_ms.is_some());
    }

    #[test]
    fn stall_detection_zero_timeout() {
        let agg = StreamAggregator::new(Duration::ZERO);
        // Zero timeout means stall detection is disabled
        assert!(!agg.is_stalled());
    }

    #[test]
    fn error_event_completes_stream() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::Error {
            message: "rate limited".into(),
        });
        assert!(agg.is_complete());
    }

    #[test]
    fn reset_clears_state() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::MessageStart {
            id: "m".into(),
            usage: usage(0, 0),
        });
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "text".into(),
        });
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: "hello".into(),
        });
        agg.feed(&StreamEvent::MessageStop);

        agg.reset();
        assert_eq!(agg.aggregate_text(), "");
        assert!(!agg.is_complete());
        assert!(agg.message_id().is_none());
        assert_eq!(agg.metrics().chunk_count, 0);
    }

    #[test]
    fn incomplete_tool_not_in_aggregate() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
        });
        agg.set_tool_metadata(0, "tc_1".into(), "bash".into());
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#"{"cmd": "ls"}"#.into(),
        });
        // No ContentBlockStop — tool is incomplete
        assert!(agg.aggregate_tool_calls().is_empty());
    }

    #[test]
    fn multiple_tool_calls() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        // Tool 1
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 0,
            content_type: "tool_use".into(),
        });
        agg.set_tool_metadata(0, "tc_1".into(), "read_file".into());
        agg.feed(&StreamEvent::ContentDelta {
            index: 0,
            delta: r#"{"path": "a.rs"}"#.into(),
        });
        agg.feed(&StreamEvent::ContentBlockStop { index: 0 });
        // Tool 2
        agg.feed(&StreamEvent::ContentBlockStart {
            index: 1,
            content_type: "tool_use".into(),
        });
        agg.set_tool_metadata(1, "tc_2".into(), "read_file".into());
        agg.feed(&StreamEvent::ContentDelta {
            index: 1,
            delta: r#"{"path": "b.rs"}"#.into(),
        });
        agg.feed(&StreamEvent::ContentBlockStop { index: 1 });

        let tools = agg.aggregate_tool_calls();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].input["path"], "a.rs");
        assert_eq!(tools[1].input["path"], "b.rs");
    }

    #[test]
    fn with_default_timeout_creates_aggregator() {
        let agg = StreamAggregator::with_default_timeout();
        assert!(!agg.is_complete());
        assert!(!agg.is_stalled());
    }

    #[test]
    fn feed_returns_true_on_complete() {
        let mut agg = StreamAggregator::new(Duration::from_secs(30));
        assert!(!agg.feed(&StreamEvent::MessageStart {
            id: "m".into(),
            usage: usage(0, 0),
        }));
        assert!(agg.feed(&StreamEvent::MessageStop));
    }
}
