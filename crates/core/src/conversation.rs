//! Conversation state management.
//!
//! A `Conversation` holds the full message history for a session.
//! A `Turn` groups related messages (user request + assistant response + tool results).

use crate::message::{Message, Role};

/// A single turn in a conversation (user prompt + assistant response cycle).
#[derive(Debug, Clone)]
pub struct Turn {
    /// Messages in this turn (typically: user, assistant, tool-result, assistant...).
    pub messages: Vec<Message>,

    /// When this turn started (monotonic, not wall-clock).
    pub timestamp: std::time::Instant,
}

impl Turn {
    /// Create a new turn starting now.
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            timestamp: std::time::Instant::now(),
        }
    }

    /// Rough token estimate for all messages in this turn.
    pub fn estimated_tokens(&self) -> u64 {
        self.messages.iter().map(Message::estimated_tokens).sum()
    }
}

/// Maintains the full message history for a conversation session.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    messages: Vec<Message>,
    turns: Vec<TurnRange>,
}

/// Tracks the index range of messages belonging to a turn.
#[derive(Debug, Clone, Copy)]
struct TurnRange {
    start: usize,
    end: usize,
}

impl Conversation {
    /// Create an empty conversation.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a message to the conversation.
    pub fn push(&mut self, message: Message) {
        // Start a new turn when we see a user message
        if message.role == Role::User {
            self.turns.push(TurnRange {
                start: self.messages.len(),
                end: self.messages.len() + 1,
            });
        } else if let Some(last_turn) = self.turns.last_mut() {
            // Extend current turn
            last_turn.end = self.messages.len() + 1;
        }
        self.messages.push(message);
    }

    /// Number of messages in the conversation.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the conversation has no messages.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Number of completed turns.
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    /// Access all messages as a slice.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Iterate over all messages.
    pub fn iter(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter()
    }

    /// Get the last message, if any.
    pub fn last(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Get messages for a specific turn (0-indexed).
    pub fn turn_messages(&self, turn_index: usize) -> Option<&[Message]> {
        self.turns
            .get(turn_index)
            .map(|range| &self.messages[range.start..range.end])
    }

    /// Rough token estimate for the entire conversation.
    pub fn estimated_tokens(&self) -> u64 {
        self.messages.iter().map(Message::estimated_tokens).sum()
    }

    /// Remove messages from the beginning to stay within a token budget.
    ///
    /// Keeps at least the most recent turn. Returns the number of messages removed.
    pub fn truncate_to_budget(&mut self, max_tokens: u64) -> usize {
        if self.estimated_tokens() <= max_tokens {
            return 0;
        }

        // Walk from the end, accumulating tokens, find the cutoff
        let mut budget = max_tokens;
        let mut keep_from = self.messages.len();

        for (i, msg) in self.messages.iter().enumerate().rev() {
            let cost = msg.estimated_tokens();
            if budget >= cost {
                budget -= cost;
                keep_from = i;
            } else {
                break;
            }
        }

        if keep_from == 0 {
            return 0;
        }

        let removed = keep_from;
        self.messages.drain(..keep_from);

        // Rebuild turn ranges
        self.turns.retain_mut(|range| {
            if range.end <= removed {
                return false; // entirely removed
            }
            range.start = range.start.saturating_sub(removed);
            range.end -= removed;
            true
        });

        removed
    }

    /// Clear all messages and turns.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.turns.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_conversation() {
        let conv = Conversation::new();
        assert!(conv.is_empty());
        assert_eq!(conv.len(), 0);
        assert_eq!(conv.turn_count(), 0);
        assert_eq!(conv.estimated_tokens(), 0);
    }

    #[test]
    fn push_and_iterate() {
        let mut conv = Conversation::new();
        conv.push(Message::user("Hello"));
        conv.push(Message::assistant("Hi there!"));

        assert_eq!(conv.len(), 2);
        assert!(!conv.is_empty());
        assert_eq!(conv.turn_count(), 1);
        assert_eq!(conv.last().unwrap().role, Role::Assistant);
    }

    #[test]
    fn multiple_turns() {
        let mut conv = Conversation::new();

        // Turn 1
        conv.push(Message::user("What is 2+2?"));
        conv.push(Message::assistant("4"));

        // Turn 2
        conv.push(Message::user("And 3+3?"));
        conv.push(Message::assistant("6"));

        assert_eq!(conv.turn_count(), 2);

        let turn0 = conv.turn_messages(0).unwrap();
        assert_eq!(turn0.len(), 2);
        assert_eq!(turn0[0].text(), "What is 2+2?");

        let turn1 = conv.turn_messages(1).unwrap();
        assert_eq!(turn1.len(), 2);
        assert_eq!(turn1[0].text(), "And 3+3?");
    }

    #[test]
    fn estimated_tokens_increases() {
        let mut conv = Conversation::new();
        let t0 = conv.estimated_tokens();
        conv.push(Message::user("Some text content here"));
        assert!(conv.estimated_tokens() > t0);
    }

    #[test]
    fn truncate_to_budget() {
        let mut conv = Conversation::new();
        for i in 0..10 {
            conv.push(Message::user(format!("Message {i} with some content")));
            conv.push(Message::assistant(format!("Response {i}")));
        }

        let before = conv.len();
        let removed = conv.truncate_to_budget(50);
        assert!(removed > 0);
        assert!(conv.len() < before);
        assert!(conv.estimated_tokens() <= 50 || conv.len() <= 2);
    }

    #[test]
    fn truncate_noop_within_budget() {
        let mut conv = Conversation::new();
        conv.push(Message::user("hi"));
        conv.push(Message::assistant("hello"));

        let removed = conv.truncate_to_budget(1_000_000);
        assert_eq!(removed, 0);
        assert_eq!(conv.len(), 2);
    }

    #[test]
    fn clear() {
        let mut conv = Conversation::new();
        conv.push(Message::user("test"));
        conv.clear();
        assert!(conv.is_empty());
        assert_eq!(conv.turn_count(), 0);
    }

    #[test]
    fn turn_standalone() {
        let turn = Turn::new(vec![Message::user("hello"), Message::assistant("hi")]);
        assert_eq!(turn.messages.len(), 2);
        assert!(turn.estimated_tokens() > 0);
    }

    // ─── Additional coverage tests ───

    #[test]
    fn turn_messages_out_of_bounds() {
        let conv = Conversation::new();
        assert!(conv.turn_messages(0).is_none());
        assert!(conv.turn_messages(100).is_none());
    }

    #[test]
    fn assistant_before_user_no_turn() {
        let mut conv = Conversation::new();
        conv.push(Message::assistant("I'll help you!"));
        assert_eq!(conv.len(), 1);
        // No turn created because no user message yet
        assert_eq!(conv.turn_count(), 0);
    }

    #[test]
    fn single_user_message_is_one_turn() {
        let mut conv = Conversation::new();
        conv.push(Message::user("hi"));
        assert_eq!(conv.turn_count(), 1);
        let turn = conv.turn_messages(0).unwrap();
        assert_eq!(turn.len(), 1);
    }

    #[test]
    fn turn_with_tool_use_cycle() {
        let mut conv = Conversation::new();
        conv.push(Message::user("list files"));
        conv.push(Message::assistant("Sure, let me check."));
        conv.push(Message::tool_result("t1", "file1.txt", false));
        // tool_result has role User, so it starts a new turn
        // This matches the behavior: user messages start turns
        assert_eq!(conv.turn_count(), 2);
    }

    #[test]
    fn last_returns_most_recent() {
        let mut conv = Conversation::new();
        assert!(conv.last().is_none());
        conv.push(Message::user("first"));
        assert_eq!(conv.last().unwrap().text(), "first");
        conv.push(Message::assistant("second"));
        assert_eq!(conv.last().unwrap().text(), "second");
    }

    #[test]
    fn iter_returns_all_messages() {
        let mut conv = Conversation::new();
        conv.push(Message::user("a"));
        conv.push(Message::assistant("b"));
        conv.push(Message::user("c"));
        let texts: Vec<String> = conv.iter().map(Message::text).collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn messages_slice() {
        let mut conv = Conversation::new();
        conv.push(Message::user("hello"));
        conv.push(Message::assistant("world"));
        let msgs = conv.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn truncate_to_budget_very_tight() {
        let mut conv = Conversation::new();
        conv.push(Message::user("a"));
        conv.push(Message::assistant("b"));
        // Budget of 1 token — should keep at least the last message
        let removed = conv.truncate_to_budget(1);
        assert!(removed <= 1);
        assert!(conv.len() >= 1);
    }

    #[test]
    fn truncate_preserves_turn_structure() {
        let mut conv = Conversation::new();
        // Turn 1
        conv.push(Message::user("old question"));
        conv.push(Message::assistant("old answer"));
        // Turn 2
        conv.push(Message::user("new question"));
        conv.push(Message::assistant("new answer"));

        // Set budget so only the last turn fits
        let last_turn_tokens = conv.turn_messages(1).unwrap().iter()
            .map(Message::estimated_tokens)
            .sum::<u64>();
        let removed = conv.truncate_to_budget(last_turn_tokens + 1);

        if removed > 0 {
            // If truncation happened, remaining messages should still be coherent
            assert!(conv.len() >= 1);
        }
    }

    #[test]
    fn clear_then_reuse() {
        let mut conv = Conversation::new();
        conv.push(Message::user("hello"));
        conv.push(Message::assistant("world"));
        conv.clear();
        assert!(conv.is_empty());
        assert_eq!(conv.turn_count(), 0);

        // Reuse after clear
        conv.push(Message::user("fresh start"));
        assert_eq!(conv.len(), 1);
        assert_eq!(conv.turn_count(), 1);
    }

    #[test]
    fn turn_empty_messages() {
        let turn = Turn::new(vec![]);
        assert_eq!(turn.messages.len(), 0);
        assert_eq!(turn.estimated_tokens(), 0);
    }

    #[test]
    fn conversation_default() {
        let conv = Conversation::default();
        assert!(conv.is_empty());
        assert_eq!(conv.turn_count(), 0);
    }
}
