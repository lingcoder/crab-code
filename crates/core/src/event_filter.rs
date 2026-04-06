//! Event filters — composable predicates for filtering events.

use crate::event::Event;
use crate::event_bus::{EventEnvelope, EventPriority, SubscriptionId};

// ── EventFilter trait ──────────────────────────────────────────────────

/// Predicate that tests whether an event envelope should be processed.
pub trait EventFilter: Send + Sync {
    /// Returns `true` if the envelope matches this filter.
    fn matches(&self, envelope: &EventEnvelope) -> bool;
}

// ── TypeFilter ─────────────────────────────────────────────────────────

/// Categorization of event variants for filtering purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventCategory {
    Message,
    Tool,
    Permission,
    Compact,
    Token,
    Memory,
    Session,
    Agent,
    Error,
}

impl std::fmt::Display for EventCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message => f.write_str("message"),
            Self::Tool => f.write_str("tool"),
            Self::Permission => f.write_str("permission"),
            Self::Compact => f.write_str("compact"),
            Self::Token => f.write_str("token"),
            Self::Memory => f.write_str("memory"),
            Self::Session => f.write_str("session"),
            Self::Agent => f.write_str("agent"),
            Self::Error => f.write_str("error"),
        }
    }
}

/// Classify an event into its category.
#[must_use]
pub fn event_category(event: &Event) -> EventCategory {
    match event {
        Event::TurnStart { .. }
        | Event::MessageStart { .. }
        | Event::ContentDelta { .. }
        | Event::ContentBlockStop { .. }
        | Event::MessageEnd { .. } => EventCategory::Message,

        Event::ToolUseStart { .. } | Event::ToolUseInput { .. } | Event::ToolResult { .. } => {
            EventCategory::Tool
        }

        Event::PermissionRequest { .. } | Event::PermissionResponse { .. } => {
            EventCategory::Permission
        }

        Event::CompactStart { .. } | Event::CompactEnd { .. } => EventCategory::Compact,

        Event::TokenWarning { .. } => EventCategory::Token,

        Event::MemoryLoaded { .. } | Event::MemorySaved { .. } => EventCategory::Memory,

        Event::SessionSaved { .. } | Event::SessionResumed { .. } => EventCategory::Session,

        Event::AgentWorkerStarted { .. } | Event::AgentWorkerCompleted { .. } => {
            EventCategory::Agent
        }

        Event::Error { .. } => EventCategory::Error,
    }
}

/// Filter events by their category.
#[derive(Debug, Clone)]
pub struct TypeFilter {
    categories: Vec<EventCategory>,
}

impl TypeFilter {
    /// Accept events matching any of the given categories.
    #[must_use]
    pub fn new(categories: Vec<EventCategory>) -> Self {
        Self { categories }
    }

    /// Single-category convenience.
    #[must_use]
    pub fn single(category: EventCategory) -> Self {
        Self {
            categories: vec![category],
        }
    }
}

impl EventFilter for TypeFilter {
    fn matches(&self, envelope: &EventEnvelope) -> bool {
        let cat = event_category(&envelope.event);
        self.categories.contains(&cat)
    }
}

// ── PriorityFilter ─────────────────────────────────────────────────────

/// Filter events by minimum priority level.
#[derive(Debug, Clone)]
pub struct PriorityFilter {
    min_priority: EventPriority,
}

impl PriorityFilter {
    #[must_use]
    pub fn new(min_priority: EventPriority) -> Self {
        Self { min_priority }
    }
}

impl EventFilter for PriorityFilter {
    fn matches(&self, envelope: &EventEnvelope) -> bool {
        envelope.metadata.priority >= self.min_priority
    }
}

// ── SourceFilter ───────────────────────────────────────────────────────

/// Filter events by source component name.
#[derive(Debug, Clone)]
pub struct SourceFilter {
    source: String,
}

impl SourceFilter {
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

impl EventFilter for SourceFilter {
    fn matches(&self, envelope: &EventEnvelope) -> bool {
        envelope.metadata.source == self.source
    }
}

// ── CompositeFilter ────────────────────────────────────────────────────

/// Combinator mode for composite filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// All child filters must match.
    And,
    /// At least one child filter must match.
    Or,
}

/// Combines multiple filters with AND or OR logic.
pub struct CompositeFilter {
    mode: FilterMode,
    filters: Vec<Box<dyn EventFilter>>,
}

impl CompositeFilter {
    #[must_use]
    pub fn new(mode: FilterMode) -> Self {
        Self {
            mode,
            filters: Vec::new(),
        }
    }

    /// Add a filter (builder pattern).
    #[must_use]
    pub fn with(mut self, filter: impl EventFilter + 'static) -> Self {
        self.filters.push(Box::new(filter));
        self
    }

    /// Add a filter by reference.
    pub fn add(&mut self, filter: impl EventFilter + 'static) {
        self.filters.push(Box::new(filter));
    }

    /// Number of child filters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    /// Whether there are no child filters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}

impl EventFilter for CompositeFilter {
    fn matches(&self, envelope: &EventEnvelope) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        match self.mode {
            FilterMode::And => self.filters.iter().all(|f| f.matches(envelope)),
            FilterMode::Or => self.filters.iter().any(|f| f.matches(envelope)),
        }
    }
}

impl std::fmt::Debug for CompositeFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeFilter")
            .field("mode", &self.mode)
            .field("count", &self.filters.len())
            .finish()
    }
}

// ── FilteredSubscription ───────────────────────────────────────────────

/// A subscription that only fires when a filter matches.
pub struct FilteredSubscription {
    /// The subscription ID in the bus.
    pub id: SubscriptionId,
}

impl crate::event_bus::EventBus {
    /// Subscribe with a filter — the handler is only called for matching events.
    pub fn subscribe_filtered(
        &mut self,
        filter: impl EventFilter + 'static,
        handler: impl Fn(&EventEnvelope) + Send + Sync + 'static,
    ) -> FilteredSubscription {
        let id = self.subscribe(move |env| {
            if filter.matches(env) {
                handler(env);
            }
        });
        FilteredSubscription { id }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::{EventBus, EventMetadata};
    use std::sync::{Arc, Mutex};

    fn make_envelope(event: Event, priority: EventPriority, source: &str) -> EventEnvelope {
        EventEnvelope {
            event,
            metadata: EventMetadata::from_source(source).with_priority(priority),
            sequence: 1,
        }
    }

    // ── EventCategory ──────────────────────────────────────────────────

    #[test]
    fn event_category_classification() {
        assert_eq!(
            event_category(&Event::TurnStart { turn_index: 0 }),
            EventCategory::Message,
        );
        assert_eq!(
            event_category(&Event::ToolUseStart {
                id: "t".into(),
                name: "bash".into()
            }),
            EventCategory::Tool,
        );
        assert_eq!(
            event_category(&Event::Error {
                message: "x".into()
            }),
            EventCategory::Error,
        );
        assert_eq!(
            event_category(&Event::MemoryLoaded { count: 1 }),
            EventCategory::Memory,
        );
        assert_eq!(
            event_category(&Event::SessionSaved {
                session_id: "s".into()
            }),
            EventCategory::Session,
        );
        assert_eq!(
            event_category(&Event::AgentWorkerStarted {
                worker_id: "w".into(),
                task_prompt: "p".into()
            }),
            EventCategory::Agent,
        );
    }

    #[test]
    fn category_display() {
        assert_eq!(EventCategory::Message.to_string(), "message");
        assert_eq!(EventCategory::Tool.to_string(), "tool");
        assert_eq!(EventCategory::Error.to_string(), "error");
    }

    // ── TypeFilter ─────────────────────────────────────────────────────

    #[test]
    fn type_filter_matches() {
        let filter = TypeFilter::single(EventCategory::Error);
        let env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Normal,
            "",
        );
        assert!(filter.matches(&env));
    }

    #[test]
    fn type_filter_rejects() {
        let filter = TypeFilter::single(EventCategory::Tool);
        let env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Normal,
            "",
        );
        assert!(!filter.matches(&env));
    }

    #[test]
    fn type_filter_multiple_categories() {
        let filter = TypeFilter::new(vec![EventCategory::Error, EventCategory::Tool]);
        let err_env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Normal,
            "",
        );
        let msg_env = make_envelope(
            Event::TurnStart { turn_index: 0 },
            EventPriority::Normal,
            "",
        );
        assert!(filter.matches(&err_env));
        assert!(!filter.matches(&msg_env));
    }

    // ── PriorityFilter ─────────────────────────────────────────────────

    #[test]
    fn priority_filter_matches_above() {
        let filter = PriorityFilter::new(EventPriority::High);
        let env = make_envelope(
            Event::TurnStart { turn_index: 0 },
            EventPriority::Critical,
            "",
        );
        assert!(filter.matches(&env));
    }

    #[test]
    fn priority_filter_matches_equal() {
        let filter = PriorityFilter::new(EventPriority::High);
        let env = make_envelope(Event::TurnStart { turn_index: 0 }, EventPriority::High, "");
        assert!(filter.matches(&env));
    }

    #[test]
    fn priority_filter_rejects_below() {
        let filter = PriorityFilter::new(EventPriority::High);
        let env = make_envelope(
            Event::TurnStart { turn_index: 0 },
            EventPriority::Normal,
            "",
        );
        assert!(!filter.matches(&env));
    }

    // ── SourceFilter ───────────────────────────────────────────────────

    #[test]
    fn source_filter_matches() {
        let filter = SourceFilter::new("agent");
        let env = make_envelope(
            Event::TurnStart { turn_index: 0 },
            EventPriority::Normal,
            "agent",
        );
        assert!(filter.matches(&env));
    }

    #[test]
    fn source_filter_rejects() {
        let filter = SourceFilter::new("agent");
        let env = make_envelope(
            Event::TurnStart { turn_index: 0 },
            EventPriority::Normal,
            "tool",
        );
        assert!(!filter.matches(&env));
    }

    // ── CompositeFilter ────────────────────────────────────────────────

    #[test]
    fn composite_and_all_match() {
        let filter = CompositeFilter::new(FilterMode::And)
            .with(TypeFilter::single(EventCategory::Error))
            .with(PriorityFilter::new(EventPriority::High));
        let env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Critical,
            "",
        );
        assert!(filter.matches(&env));
    }

    #[test]
    fn composite_and_partial_match() {
        let filter = CompositeFilter::new(FilterMode::And)
            .with(TypeFilter::single(EventCategory::Error))
            .with(PriorityFilter::new(EventPriority::High));
        let env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Low,
            "",
        );
        assert!(!filter.matches(&env));
    }

    #[test]
    fn composite_or_one_matches() {
        let filter = CompositeFilter::new(FilterMode::Or)
            .with(TypeFilter::single(EventCategory::Error))
            .with(TypeFilter::single(EventCategory::Tool));
        let env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Normal,
            "",
        );
        assert!(filter.matches(&env));
    }

    #[test]
    fn composite_or_none_match() {
        let filter = CompositeFilter::new(FilterMode::Or)
            .with(TypeFilter::single(EventCategory::Tool))
            .with(TypeFilter::single(EventCategory::Session));
        let env = make_envelope(
            Event::Error {
                message: "x".into(),
            },
            EventPriority::Normal,
            "",
        );
        assert!(!filter.matches(&env));
    }

    #[test]
    fn composite_empty_matches_all() {
        let filter = CompositeFilter::new(FilterMode::And);
        let env = make_envelope(
            Event::TurnStart { turn_index: 0 },
            EventPriority::Normal,
            "",
        );
        assert!(filter.matches(&env));
    }

    #[test]
    fn composite_len() {
        let filter =
            CompositeFilter::new(FilterMode::And).with(TypeFilter::single(EventCategory::Error));
        assert_eq!(filter.len(), 1);
        assert!(!filter.is_empty());
    }

    #[test]
    fn composite_debug() {
        let filter = CompositeFilter::new(FilterMode::Or);
        let dbg = format!("{filter:?}");
        assert!(dbg.contains("CompositeFilter"));
    }

    // ── FilteredSubscription ───────────────────────────────────────────

    #[test]
    fn filtered_subscription_only_fires_on_match() {
        let mut bus = EventBus::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recv = Arc::clone(&received);

        bus.subscribe_filtered(TypeFilter::single(EventCategory::Error), move |env| {
            if let Event::Error { ref message } = env.event {
                recv.lock().unwrap().push(message.clone());
            }
        });

        // Publish non-matching event
        bus.publish_default(Event::TurnStart { turn_index: 0 });
        assert!(received.lock().unwrap().is_empty());

        // Publish matching event
        bus.publish_default(Event::Error {
            message: "boom".into(),
        });
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[test]
    fn filtered_subscription_can_unsubscribe() {
        let mut bus = EventBus::new();
        let count = Arc::new(Mutex::new(0u32));
        let c = Arc::clone(&count);

        let fs = bus.subscribe_filtered(PriorityFilter::new(EventPriority::Normal), move |_| {
            *c.lock().unwrap() += 1;
        });

        bus.publish_default(Event::TurnStart { turn_index: 0 });
        assert_eq!(*count.lock().unwrap(), 1);

        bus.unsubscribe(fs.id);
        bus.publish_default(Event::TurnStart { turn_index: 1 });
        assert_eq!(*count.lock().unwrap(), 1);
    }
}
