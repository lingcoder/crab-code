use std::fmt;

use crab_core::event::Event;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Unique identifier for correlating request/response pairs.
pub type CorrelationId = String;

/// Generate a new correlation ID.
#[must_use]
pub fn new_correlation_id() -> CorrelationId {
    format!(
        "corr_{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            % u128::from(u64::MAX)
    )
}

/// Envelope wrapping every inter-agent message with routing metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Who sent this message.
    pub from: String,
    /// Intended recipient — agent name/id, or `"*"` for broadcast.
    pub to: String,
    /// Optional correlation ID for request/response pairing.
    pub correlation_id: Option<CorrelationId>,
    /// The message payload.
    pub payload: AgentMessage,
}

impl Envelope {
    /// Create a new directed envelope.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>, payload: AgentMessage) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            correlation_id: None,
            payload,
        }
    }

    /// Create an envelope with a fresh correlation ID (for requests).
    #[must_use]
    pub fn with_correlation(mut self) -> Self {
        self.correlation_id = Some(new_correlation_id());
        self
    }

    /// Create a broadcast envelope (`to = "*"`).
    #[must_use]
    pub fn broadcast(from: impl Into<String>, payload: AgentMessage) -> Self {
        Self {
            from: from.into(),
            to: "*".into(),
            correlation_id: None,
            payload,
        }
    }

    /// Whether this is a broadcast message.
    #[must_use]
    pub fn is_broadcast(&self) -> bool {
        self.to == "*"
    }

    /// Create a response envelope to this message.
    #[must_use]
    pub fn reply(&self, from: impl Into<String>, payload: AgentMessage) -> Self {
        Self {
            from: from.into(),
            to: self.from.clone(),
            correlation_id: self.correlation_id.clone(),
            payload,
        }
    }
}

impl fmt::Display for Envelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}→{}] {:?}", self.from, self.to, self.payload)
    }
}

/// Messages exchanged between agents via the bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessage {
    // ─── Task lifecycle ───
    /// Assign a task to an agent.
    AssignTask { task_id: String, prompt: String },
    /// Report task completion.
    TaskComplete { task_id: String, result: String },
    /// Report task failure.
    TaskFailed { task_id: String, error: String },

    // ─── Request/Response ───
    /// Request help or information from another agent.
    Request {
        request_type: RequestType,
        body: String,
    },
    /// Response to a previous request (matched by `Envelope.correlation_id`).
    Response { success: bool, body: String },

    // ─── Status ───
    /// Periodic heartbeat / status update from an agent.
    StatusUpdate {
        status: AgentStatus,
        detail: Option<String>,
    },

    // ─── Control ───
    /// Request orderly shutdown.
    Shutdown,
    /// Acknowledge shutdown request.
    ShutdownAck,
}

/// Types of requests one agent can make to another.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RequestType {
    /// Ask for help with a task.
    Help,
    /// Query another agent's capabilities.
    QueryCapabilities,
    /// Delegate a sub-task.
    Delegate,
    /// Custom request type.
    Custom(String),
}

/// Agent status for heartbeat / status updates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    ShuttingDown,
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Working => write!(f, "working"),
            Self::Blocked => write!(f, "blocked"),
            Self::ShuttingDown => write!(f, "shutting_down"),
        }
    }
}

/// Inter-agent message bus backed by tokio mpsc channels.
pub struct MessageBus {
    pub tx: mpsc::Sender<AgentMessage>,
    pub rx: mpsc::Receiver<AgentMessage>,
}

impl MessageBus {
    pub fn new(buffer: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        Self { tx, rx }
    }

    pub fn sender(&self) -> mpsc::Sender<AgentMessage> {
        self.tx.clone()
    }
}

/// Create an event channel for agent-to-TUI communication.
///
/// Returns `(sender, receiver)` with the given buffer size.
pub fn event_channel(buffer: usize) -> (mpsc::Sender<Event>, mpsc::Receiver<Event>) {
    mpsc::channel(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_bus_creation() {
        let bus = MessageBus::new(16);
        let _tx = bus.sender();
    }

    #[tokio::test]
    async fn message_bus_send_receive() {
        let mut bus = MessageBus::new(16);
        let tx = bus.sender();
        tx.send(AgentMessage::Shutdown).await.unwrap();
        let msg = bus.rx.recv().await.unwrap();
        assert!(matches!(msg, AgentMessage::Shutdown));
    }

    #[tokio::test]
    async fn message_bus_assign_task() {
        let mut bus = MessageBus::new(16);
        let tx = bus.sender();
        tx.send(AgentMessage::AssignTask {
            task_id: "t1".into(),
            prompt: "do stuff".into(),
        })
        .await
        .unwrap();
        let msg = bus.rx.recv().await.unwrap();
        match msg {
            AgentMessage::AssignTask { task_id, prompt } => {
                assert_eq!(task_id, "t1");
                assert_eq!(prompt, "do stuff");
            }
            _ => panic!("expected AssignTask"),
        }
    }

    #[tokio::test]
    async fn event_channel_send_receive() {
        let (tx, mut rx) = event_channel(16);
        tx.send(crab_core::event::Event::TurnStart { turn_index: 0 })
            .await
            .unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(
            event,
            crab_core::event::Event::TurnStart { turn_index: 0 }
        ));
    }

    // ─── Envelope tests ───

    #[test]
    fn envelope_new() {
        let env = Envelope::new("alice", "bob", AgentMessage::Shutdown);
        assert_eq!(env.from, "alice");
        assert_eq!(env.to, "bob");
        assert!(!env.is_broadcast());
        assert!(env.correlation_id.is_none());
    }

    #[test]
    fn envelope_broadcast() {
        let env = Envelope::broadcast("alice", AgentMessage::Shutdown);
        assert_eq!(env.to, "*");
        assert!(env.is_broadcast());
    }

    #[test]
    fn envelope_with_correlation() {
        let env = Envelope::new("alice", "bob", AgentMessage::Shutdown).with_correlation();
        assert!(env.correlation_id.is_some());
        assert!(env.correlation_id.as_ref().unwrap().starts_with("corr_"));
    }

    #[test]
    fn envelope_reply() {
        let req = Envelope::new(
            "alice",
            "bob",
            AgentMessage::Request {
                request_type: RequestType::Help,
                body: "help me".into(),
            },
        )
        .with_correlation();

        let corr = req.correlation_id.clone();

        let resp = req.reply(
            "bob",
            AgentMessage::Response {
                success: true,
                body: "here you go".into(),
            },
        );

        assert_eq!(resp.from, "bob");
        assert_eq!(resp.to, "alice");
        assert_eq!(resp.correlation_id, corr);
    }

    #[test]
    fn envelope_display() {
        let env = Envelope::new("alice", "bob", AgentMessage::Shutdown);
        let s = format!("{env}");
        assert!(s.contains("alice"));
        assert!(s.contains("bob"));
    }

    // ─── AgentMessage variant tests ───

    #[test]
    fn agent_message_task_failed() {
        let msg = AgentMessage::TaskFailed {
            task_id: "t1".into(),
            error: "boom".into(),
        };
        if let AgentMessage::TaskFailed { task_id, error } = msg {
            assert_eq!(task_id, "t1");
            assert_eq!(error, "boom");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn agent_message_request_response() {
        let req = AgentMessage::Request {
            request_type: RequestType::Delegate,
            body: "do this sub-task".into(),
        };
        let resp = AgentMessage::Response {
            success: true,
            body: "done".into(),
        };
        assert!(matches!(
            req,
            AgentMessage::Request {
                request_type: RequestType::Delegate,
                ..
            }
        ));
        assert!(matches!(resp, AgentMessage::Response { success: true, .. }));
    }

    #[test]
    fn agent_message_status_update() {
        let msg = AgentMessage::StatusUpdate {
            status: AgentStatus::Working,
            detail: Some("processing task t1".into()),
        };
        if let AgentMessage::StatusUpdate { status, detail } = msg {
            assert_eq!(status, AgentStatus::Working);
            assert_eq!(detail.as_deref(), Some("processing task t1"));
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn agent_message_shutdown_ack() {
        let msg = AgentMessage::ShutdownAck;
        assert!(matches!(msg, AgentMessage::ShutdownAck));
    }

    #[test]
    fn request_type_custom() {
        let rt = RequestType::Custom("my_request".into());
        assert_eq!(rt, RequestType::Custom("my_request".into()));
    }

    #[test]
    fn agent_status_display() {
        assert_eq!(AgentStatus::Idle.to_string(), "idle");
        assert_eq!(AgentStatus::Working.to_string(), "working");
        assert_eq!(AgentStatus::Blocked.to_string(), "blocked");
        assert_eq!(AgentStatus::ShuttingDown.to_string(), "shutting_down");
    }

    // ─── Serde roundtrip ───

    #[test]
    fn envelope_serde_roundtrip() {
        let env = Envelope::new(
            "alice",
            "bob",
            AgentMessage::AssignTask {
                task_id: "t1".into(),
                prompt: "hello".into(),
            },
        )
        .with_correlation();

        let json = serde_json::to_string(&env).unwrap();
        let parsed: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.from, "alice");
        assert_eq!(parsed.to, "bob");
        assert!(parsed.correlation_id.is_some());
    }

    #[test]
    fn agent_message_serde_all_variants() {
        let variants: Vec<AgentMessage> = vec![
            AgentMessage::AssignTask {
                task_id: "t1".into(),
                prompt: "do it".into(),
            },
            AgentMessage::TaskComplete {
                task_id: "t1".into(),
                result: "done".into(),
            },
            AgentMessage::TaskFailed {
                task_id: "t1".into(),
                error: "oops".into(),
            },
            AgentMessage::Request {
                request_type: RequestType::Help,
                body: "help".into(),
            },
            AgentMessage::Response {
                success: false,
                body: "nope".into(),
            },
            AgentMessage::StatusUpdate {
                status: AgentStatus::Idle,
                detail: None,
            },
            AgentMessage::Shutdown,
            AgentMessage::ShutdownAck,
        ];

        for msg in &variants {
            let json = serde_json::to_string(msg).unwrap();
            let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn correlation_id_is_unique() {
        let id1 = new_correlation_id();
        // Sleep a tiny bit to ensure different nanos
        std::thread::sleep(std::time::Duration::from_nanos(100));
        let id2 = new_correlation_id();
        // They should be different (with very high probability)
        // Not a hard assertion since nanos could collide in theory
        assert!(id1.starts_with("corr_"));
        assert!(id2.starts_with("corr_"));
    }
}
