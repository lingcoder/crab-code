//! Event bus — publish/subscribe event distribution with priority support.

use crate::event::Event;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU64, Ordering};

// ── Priority ───────────────────────────────────────────────────────────

/// Priority level for events.
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EventPriority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl std::fmt::Display for EventPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => f.write_str("low"),
            Self::Normal => f.write_str("normal"),
            Self::High => f.write_str("high"),
            Self::Critical => f.write_str("critical"),
        }
    }
}

// ── Metadata ───────────────────────────────────────────────────────────

/// Metadata attached to a published event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventMetadata {
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Source component that emitted the event.
    pub source: String,
    /// Priority level.
    pub priority: EventPriority,
    /// Optional correlation ID for tracing related events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl Default for EventMetadata {
    fn default() -> Self {
        Self {
            timestamp_ms: now_ms(),
            source: String::new(),
            priority: EventPriority::Normal,
            correlation_id: None,
        }
    }
}

impl EventMetadata {
    /// Create metadata with a source name and default priority.
    #[must_use]
    pub fn from_source(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ..Default::default()
        }
    }

    /// Set priority (builder pattern).
    #[must_use]
    pub fn with_priority(mut self, priority: EventPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set correlation ID (builder pattern).
    #[must_use]
    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }
}

// ── Envelope ───────────────────────────────────────────────────────────

/// An event paired with its metadata, used internally by the bus.
#[derive(Debug, Clone)]
pub struct EventEnvelope {
    /// The event payload.
    pub event: Event,
    /// Associated metadata.
    pub metadata: EventMetadata,
    /// Sequence number for stable ordering within the same priority.
    pub(crate) sequence: u64,
}

impl EventEnvelope {
    pub(crate) fn new(event: Event, metadata: EventMetadata, sequence: u64) -> Self {
        Self {
            event,
            metadata,
            sequence,
        }
    }
}

// Ordering: higher priority first, then lower sequence first (FIFO within priority).
impl Eq for EventEnvelope {}

impl PartialEq for EventEnvelope {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence
    }
}

impl PartialOrd for EventEnvelope {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EventEnvelope {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.metadata
            .priority
            .cmp(&other.metadata.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

// ── Subscription ───────────────────────────────────────────────────────

/// Opaque handle to a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sub_{}", self.0)
    }
}

/// A subscription entry in the bus.
struct Subscription {
    id: SubscriptionId,
    /// Callback invoked for each matching event.
    handler: Box<dyn Fn(&EventEnvelope) + Send + Sync>,
}

// ── EventBus ───────────────────────────────────────────────────────────

/// Synchronous publish/subscribe event bus with priority queue support.
pub struct EventBus {
    subscriptions: Vec<Subscription>,
    next_sub_id: AtomicU64,
    next_seq: AtomicU64,
    /// Priority queue of pending events (for `drain_ordered`).
    queue: BinaryHeap<EventEnvelope>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("subscriptions", &self.subscriptions.len())
            .field("queue_len", &self.queue.len())
            .finish_non_exhaustive()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    #[must_use]
    pub fn new() -> Self {
        Self {
            subscriptions: Vec::new(),
            next_sub_id: AtomicU64::new(1),
            next_seq: AtomicU64::new(1),
            queue: BinaryHeap::new(),
        }
    }

    /// Subscribe with a callback. Returns a handle for unsubscribing.
    pub fn subscribe(
        &mut self,
        handler: impl Fn(&EventEnvelope) + Send + Sync + 'static,
    ) -> SubscriptionId {
        let id = SubscriptionId(self.next_sub_id.fetch_add(1, Ordering::Relaxed));
        self.subscriptions.push(Subscription {
            id,
            handler: Box::new(handler),
        });
        id
    }

    /// Remove a subscription.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        let before = self.subscriptions.len();
        self.subscriptions.retain(|s| s.id != id);
        self.subscriptions.len() < before
    }

    /// Number of active subscriptions.
    #[must_use]
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Publish an event with metadata — immediately dispatches to all subscribers.
    pub fn publish(&self, event: Event, metadata: EventMetadata) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let envelope = EventEnvelope::new(event, metadata, seq);
        for sub in &self.subscriptions {
            (sub.handler)(&envelope);
        }
    }

    /// Publish with default (Normal) metadata.
    pub fn publish_default(&self, event: Event) {
        self.publish(event, EventMetadata::default());
    }

    /// Enqueue an event for priority-ordered processing (via `drain_ordered`).
    pub fn enqueue(&mut self, event: Event, metadata: EventMetadata) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.queue.push(EventEnvelope::new(event, metadata, seq));
    }

    /// Drain the priority queue, dispatching events to subscribers in priority order.
    /// Returns the number of events dispatched.
    pub fn drain_ordered(&mut self) -> usize {
        let mut count = 0;
        while let Some(envelope) = self.queue.pop() {
            for sub in &self.subscriptions {
                (sub.handler)(&envelope);
            }
            count += 1;
        }
        count
    }

    /// Number of events waiting in the priority queue.
    #[must_use]
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Clear all pending events from the queue.
    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }
}

#[allow(clippy::cast_possible_truncation)] // u128 millis won't exceed u64 for ~584 million years
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ── EventPriority ──────────────────────────────────────────────────

    #[test]
    fn priority_ordering() {
        assert!(EventPriority::Critical > EventPriority::High);
        assert!(EventPriority::High > EventPriority::Normal);
        assert!(EventPriority::Normal > EventPriority::Low);
    }

    #[test]
    fn priority_default() {
        assert_eq!(EventPriority::default(), EventPriority::Normal);
    }

    #[test]
    fn priority_display() {
        assert_eq!(EventPriority::Low.to_string(), "low");
        assert_eq!(EventPriority::Normal.to_string(), "normal");
        assert_eq!(EventPriority::High.to_string(), "high");
        assert_eq!(EventPriority::Critical.to_string(), "critical");
    }

    #[test]
    fn priority_serde_roundtrip() {
        let json = serde_json::to_string(&EventPriority::High).unwrap();
        assert_eq!(json, r#""high""#);
        let back: EventPriority = serde_json::from_str(&json).unwrap();
        assert_eq!(back, EventPriority::High);
    }

    // ── EventMetadata ──────────────────────────────────────────────────

    #[test]
    fn metadata_default() {
        let meta = EventMetadata::default();
        assert_eq!(meta.priority, EventPriority::Normal);
        assert!(meta.source.is_empty());
        assert!(meta.correlation_id.is_none());
    }

    #[test]
    fn metadata_from_source() {
        let meta = EventMetadata::from_source("agent");
        assert_eq!(meta.source, "agent");
    }

    #[test]
    fn metadata_builder() {
        let meta = EventMetadata::from_source("tool")
            .with_priority(EventPriority::Critical)
            .with_correlation_id("corr-123");
        assert_eq!(meta.priority, EventPriority::Critical);
        assert_eq!(meta.correlation_id.as_deref(), Some("corr-123"));
    }

    #[test]
    fn metadata_timestamp_is_recent() {
        let meta = EventMetadata::default();
        let now = now_ms();
        assert!(now - meta.timestamp_ms < 1000);
    }

    // ── SubscriptionId ─────────────────────────────────────────────────

    #[test]
    fn subscription_id_display() {
        let id = SubscriptionId(42);
        assert_eq!(id.to_string(), "sub_42");
    }

    // ── EventBus ───────────────────────────────────────────────────────

    #[test]
    fn new_bus_is_empty() {
        let bus = EventBus::new();
        assert_eq!(bus.subscription_count(), 0);
        assert_eq!(bus.queue_len(), 0);
    }

    #[test]
    fn subscribe_and_publish() {
        let mut bus = EventBus::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recv_clone = Arc::clone(&received);

        bus.subscribe(move |env| {
            if let Event::Error { ref message } = env.event {
                recv_clone.lock().unwrap().push(message.clone());
            }
        });

        bus.publish_default(Event::Error {
            message: "test".into(),
        });

        let msgs = received.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0], "test");
    }

    #[test]
    fn multiple_subscribers() {
        let mut bus = EventBus::new();
        let count = Arc::new(Mutex::new(0u32));

        for _ in 0..3 {
            let c = Arc::clone(&count);
            bus.subscribe(move |_| {
                *c.lock().unwrap() += 1;
            });
        }

        bus.publish_default(Event::TurnStart { turn_index: 0 });
        assert_eq!(*count.lock().unwrap(), 3);
    }

    #[test]
    fn unsubscribe() {
        let mut bus = EventBus::new();
        let count = Arc::new(Mutex::new(0u32));
        let c = Arc::clone(&count);

        let id = bus.subscribe(move |_| {
            *c.lock().unwrap() += 1;
        });

        bus.publish_default(Event::TurnStart { turn_index: 0 });
        assert_eq!(*count.lock().unwrap(), 1);

        assert!(bus.unsubscribe(id));
        assert_eq!(bus.subscription_count(), 0);

        bus.publish_default(Event::TurnStart { turn_index: 1 });
        assert_eq!(*count.lock().unwrap(), 1); // unchanged
    }

    #[test]
    fn unsubscribe_nonexistent() {
        let mut bus = EventBus::new();
        assert!(!bus.unsubscribe(SubscriptionId(999)));
    }

    // ── Priority queue ─────────────────────────────────────────────────

    #[test]
    fn enqueue_and_drain_ordered() {
        let mut bus = EventBus::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let o = Arc::clone(&order);

        bus.subscribe(move |env| {
            if let Event::Error { ref message } = env.event {
                o.lock().unwrap().push(message.clone());
            }
        });

        // Enqueue in mixed priority order
        bus.enqueue(
            Event::Error {
                message: "low".into(),
            },
            EventMetadata::default().with_priority(EventPriority::Low),
        );
        bus.enqueue(
            Event::Error {
                message: "critical".into(),
            },
            EventMetadata::default().with_priority(EventPriority::Critical),
        );
        bus.enqueue(
            Event::Error {
                message: "normal".into(),
            },
            EventMetadata::default().with_priority(EventPriority::Normal),
        );

        assert_eq!(bus.queue_len(), 3);

        let dispatched = bus.drain_ordered();
        assert_eq!(dispatched, 3);
        assert_eq!(bus.queue_len(), 0);

        let msgs = order.lock().unwrap();
        assert_eq!(msgs[0], "critical");
        assert_eq!(msgs[1], "normal");
        assert_eq!(msgs[2], "low");
    }

    #[test]
    fn fifo_within_same_priority() {
        let mut bus = EventBus::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let o = Arc::clone(&order);

        bus.subscribe(move |env| {
            if let Event::Error { ref message } = env.event {
                o.lock().unwrap().push(message.clone());
            }
        });

        bus.enqueue(
            Event::Error {
                message: "first".into(),
            },
            EventMetadata::default().with_priority(EventPriority::Normal),
        );
        bus.enqueue(
            Event::Error {
                message: "second".into(),
            },
            EventMetadata::default().with_priority(EventPriority::Normal),
        );

        bus.drain_ordered();
        let msgs = order.lock().unwrap();
        assert_eq!(msgs[0], "first");
        assert_eq!(msgs[1], "second");
    }

    #[test]
    fn clear_queue() {
        let mut bus = EventBus::new();
        bus.enqueue(Event::TurnStart { turn_index: 0 }, EventMetadata::default());
        bus.enqueue(Event::TurnStart { turn_index: 1 }, EventMetadata::default());
        assert_eq!(bus.queue_len(), 2);
        bus.clear_queue();
        assert_eq!(bus.queue_len(), 0);
    }

    #[test]
    fn drain_empty_queue() {
        let mut bus = EventBus::new();
        assert_eq!(bus.drain_ordered(), 0);
    }

    #[test]
    fn bus_debug() {
        let bus = EventBus::new();
        let dbg = format!("{bus:?}");
        assert!(dbg.contains("EventBus"));
    }

    // ── EventEnvelope ordering ─────────────────────────────────────────

    #[test]
    fn envelope_ordering() {
        let high = EventEnvelope::new(
            Event::TurnStart { turn_index: 0 },
            EventMetadata::default().with_priority(EventPriority::High),
            1,
        );
        let low = EventEnvelope::new(
            Event::TurnStart { turn_index: 1 },
            EventMetadata::default().with_priority(EventPriority::Low),
            2,
        );
        assert!(high > low);
    }
}
