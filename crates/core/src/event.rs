use crate::model::TokenUsage;
use crate::tool::ToolOutput;
use serde_json::Value;

/// Domain events for agent-to-UI communication.
///
/// All variants are `Clone + Send + 'static` to support
/// `tokio::sync::broadcast` channels.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Event {
    // ─── Message lifecycle ───
    /// A new conversation turn has started.
    TurnStart { turn_index: usize },

    /// API response message started, with a unique message ID.
    MessageStart { id: String },

    /// Incremental text content from the model.
    ContentDelta { index: usize, delta: String },

    /// A content block has finished streaming.
    ContentBlockStop { index: usize },

    /// The full message has completed.
    MessageEnd { usage: TokenUsage },

    // ─── Tool execution ───
    /// A tool call has started.
    ToolUseStart { id: String, name: String },

    /// Incremental tool input (streaming).
    ToolUseInput { id: String, input: Value },

    /// Tool execution has completed.
    ToolResult { id: String, output: ToolOutput },

    // ─── Permission interaction ───
    /// Request user confirmation for a tool execution.
    PermissionRequest {
        tool_name: String,
        input_summary: String,
        request_id: String,
    },

    /// User's response to a permission request.
    PermissionResponse { request_id: String, allowed: bool },

    // ─── Context compaction ───
    /// Context compaction has started.
    CompactStart {
        strategy: String,
        before_tokens: u64,
    },

    /// Context compaction has completed.
    CompactEnd {
        after_tokens: u64,
        removed_messages: usize,
    },

    // ─── Token warnings ───
    /// Token usage has exceeded a threshold.
    TokenWarning {
        usage_pct: f32,
        used: u64,
        limit: u64,
    },

    // ─── Memory ───
    /// Memory files were loaded at session start.
    MemoryLoaded { count: usize },

    /// A memory file was saved during the session.
    MemorySaved { filename: String },

    // ─── Session history ───
    /// Session was saved to disk.
    SessionSaved { session_id: String },

    /// Session was resumed from disk.
    SessionResumed {
        session_id: String,
        message_count: usize,
    },

    // ─── Sub-agent workers ───
    /// A sub-agent worker has started.
    AgentWorkerStarted {
        worker_id: String,
        task_prompt: String,
    },

    /// A sub-agent worker has completed.
    AgentWorkerCompleted {
        worker_id: String,
        /// Final text output from the worker, if any.
        result: Option<String>,
        /// Whether the worker completed successfully.
        success: bool,
        /// Total tokens used by this worker.
        usage: TokenUsage,
    },

    // ─── Errors ───
    /// An error occurred during processing.
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Event>();
    }

    #[test]
    fn event_is_clone() {
        let event = Event::ContentDelta {
            index: 0,
            delta: "hello".into(),
        };
        let cloned = event.clone();
        if let Event::ContentDelta { index, delta } = cloned {
            assert_eq!(index, 0);
            assert_eq!(delta, "hello");
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn event_is_static() {
        fn assert_static<T: 'static>() {}
        assert_static::<Event>();
    }

    #[test]
    fn turn_start_event() {
        let event = Event::TurnStart { turn_index: 3 };
        if let Event::TurnStart { turn_index } = event {
            assert_eq!(turn_index, 3);
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn message_start_event() {
        let event = Event::MessageStart {
            id: "msg_123".into(),
        };
        if let Event::MessageStart { id } = event {
            assert_eq!(id, "msg_123");
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn tool_result_event() {
        let event = Event::ToolResult {
            id: "tu_1".into(),
            output: ToolOutput::success("done"),
        };
        if let Event::ToolResult { id, output } = event {
            assert_eq!(id, "tu_1");
            assert!(!output.is_error);
            assert_eq!(output.text(), "done");
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn permission_request_event() {
        let event = Event::PermissionRequest {
            tool_name: "bash".into(),
            input_summary: "rm -rf /".into(),
            request_id: "req_1".into(),
        };
        if let Event::PermissionRequest {
            tool_name,
            input_summary,
            request_id,
        } = event
        {
            assert_eq!(tool_name, "bash");
            assert_eq!(input_summary, "rm -rf /");
            assert_eq!(request_id, "req_1");
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn token_warning_event() {
        let event = Event::TokenWarning {
            usage_pct: 0.85,
            used: 85000,
            limit: 100_000,
        };
        if let Event::TokenWarning {
            usage_pct,
            used,
            limit,
        } = event
        {
            assert!((usage_pct - 0.85).abs() < f32::EPSILON);
            assert_eq!(used, 85000);
            assert_eq!(limit, 100_000);
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn error_event() {
        let event = Event::Error {
            message: "something broke".into(),
        };
        if let Event::Error { message } = event {
            assert_eq!(message, "something broke");
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn compact_events() {
        let start = Event::CompactStart {
            strategy: "summarize".into(),
            before_tokens: 90000,
        };
        let end = Event::CompactEnd {
            after_tokens: 40000,
            removed_messages: 15,
        };
        assert!(matches!(start, Event::CompactStart { .. }));
        assert!(matches!(end, Event::CompactEnd { .. }));
    }

    // ─── Serde roundtrip tests for all Event variants ───

    fn serde_roundtrip(event: &Event) {
        let json = serde_json::to_string(event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        // Compare JSON representations since Event doesn't derive PartialEq
        let json2 = serde_json::to_string(&parsed).unwrap();
        assert_eq!(json, json2);
    }

    #[test]
    fn event_serde_turn_start() {
        serde_roundtrip(&Event::TurnStart { turn_index: 5 });
    }

    #[test]
    fn event_serde_message_start() {
        serde_roundtrip(&Event::MessageStart {
            id: "msg_abc".into(),
        });
    }

    #[test]
    fn event_serde_content_delta() {
        serde_roundtrip(&Event::ContentDelta {
            index: 2,
            delta: "hello world".into(),
        });
    }

    #[test]
    fn event_serde_content_block_stop() {
        serde_roundtrip(&Event::ContentBlockStop { index: 0 });
    }

    #[test]
    fn event_serde_message_end() {
        serde_roundtrip(&Event::MessageEnd {
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_creation_tokens: 5,
            },
        });
    }

    #[test]
    fn event_serde_tool_use_start() {
        serde_roundtrip(&Event::ToolUseStart {
            id: "tu_1".into(),
            name: "bash".into(),
        });
    }

    #[test]
    fn event_serde_tool_use_input() {
        serde_roundtrip(&Event::ToolUseInput {
            id: "tu_1".into(),
            input: serde_json::json!({"command": "ls"}),
        });
    }

    #[test]
    fn event_serde_tool_result() {
        serde_roundtrip(&Event::ToolResult {
            id: "tu_1".into(),
            output: ToolOutput::success("file1.txt"),
        });
    }

    #[test]
    fn event_serde_tool_result_error() {
        serde_roundtrip(&Event::ToolResult {
            id: "tu_2".into(),
            output: ToolOutput::error("command failed"),
        });
    }

    #[test]
    fn event_serde_permission_request() {
        serde_roundtrip(&Event::PermissionRequest {
            tool_name: "bash".into(),
            input_summary: "rm -rf /tmp/cache".into(),
            request_id: "req_42".into(),
        });
    }

    #[test]
    fn event_serde_permission_response() {
        serde_roundtrip(&Event::PermissionResponse {
            request_id: "req_42".into(),
            allowed: true,
        });
        serde_roundtrip(&Event::PermissionResponse {
            request_id: "req_43".into(),
            allowed: false,
        });
    }

    #[test]
    fn event_serde_compact_start() {
        serde_roundtrip(&Event::CompactStart {
            strategy: "summarize".into(),
            before_tokens: 95000,
        });
    }

    #[test]
    fn event_serde_compact_end() {
        serde_roundtrip(&Event::CompactEnd {
            after_tokens: 40000,
            removed_messages: 12,
        });
    }

    #[test]
    fn event_serde_token_warning() {
        serde_roundtrip(&Event::TokenWarning {
            usage_pct: 0.92,
            used: 92000,
            limit: 100_000,
        });
    }

    #[test]
    fn event_serde_error() {
        serde_roundtrip(&Event::Error {
            message: "rate limit exceeded".into(),
        });
    }

    #[test]
    fn event_serde_memory_loaded() {
        serde_roundtrip(&Event::MemoryLoaded { count: 5 });
    }

    #[test]
    fn event_serde_memory_saved() {
        serde_roundtrip(&Event::MemorySaved {
            filename: "user_role.md".into(),
        });
    }

    #[test]
    fn event_serde_session_saved() {
        serde_roundtrip(&Event::SessionSaved {
            session_id: "sess_abc".into(),
        });
    }

    #[test]
    fn event_serde_session_resumed() {
        serde_roundtrip(&Event::SessionResumed {
            session_id: "sess_abc".into(),
            message_count: 42,
        });
    }
}
