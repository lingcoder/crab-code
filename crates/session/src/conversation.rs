use crab_core::conversation::Conversation as CoreConversation;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::TokenUsage;

/// Session-level conversation: wraps the core `Conversation` and adds
/// session metadata (id, system prompt, context window, cumulative usage).
#[derive(Debug)]
pub struct Conversation {
    pub id: String,
    pub system_prompt: String,
    /// Underlying core conversation (message history + turn tracking).
    pub inner: CoreConversation,
    /// Cumulative token usage across all API calls in this session.
    pub total_usage: TokenUsage,
    /// Maximum context window size (in tokens) for the active model.
    pub context_window: u64,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new(String::new(), String::new(), 0)
    }
}

impl Conversation {
    pub fn new(id: String, system_prompt: String, context_window: u64) -> Self {
        Self {
            id,
            system_prompt,
            inner: CoreConversation::new(),
            total_usage: TokenUsage::default(),
            context_window,
        }
    }

    /// Append a message (delegates to core conversation).
    pub fn push(&mut self, msg: Message) {
        self.inner.push(msg);
    }

    /// Push a user text message.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.inner.push(Message::user(text));
    }

    /// Push an assistant text message.
    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.inner.push(Message::assistant(text));
    }

    /// Push an assistant message containing tool use blocks.
    pub fn push_assistant_tool_use(&mut self, blocks: Vec<ContentBlock>) {
        self.inner.push(Message::new(Role::Assistant, blocks));
    }

    /// Push a tool result as a user message.
    pub fn push_tool_result(
        &mut self,
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) {
        self.inner
            .push(Message::tool_result(tool_use_id, content, is_error));
    }

    /// Access all messages as a slice.
    pub fn messages(&self) -> &[Message] {
        self.inner.messages()
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the conversation is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Number of completed turns.
    pub fn turn_count(&self) -> usize {
        self.inner.turn_count()
    }

    /// Rough token estimate for the entire conversation (delegates to core).
    pub fn estimated_tokens(&self) -> u64 {
        self.inner.estimated_tokens()
    }

    /// Whether the context window usage exceeds 80%, triggering compaction.
    pub fn needs_compaction(&self) -> bool {
        if self.context_window == 0 {
            return false;
        }
        self.estimated_tokens() > self.context_window * 80 / 100
    }

    /// Record token usage from an API response.
    pub fn record_usage(&mut self, usage: TokenUsage) {
        self.total_usage += usage;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_conv() -> Conversation {
        Conversation::new("sess-1".into(), "You are helpful.".into(), 100_000)
    }

    #[test]
    fn push_user_and_assistant() {
        let mut c = make_conv();
        c.push_user("Hello");
        c.push_assistant("Hi there!");
        assert_eq!(c.len(), 2);
        assert_eq!(c.turn_count(), 1);
        assert_eq!(c.messages()[0].role, Role::User);
        assert_eq!(c.messages()[1].role, Role::Assistant);
    }

    #[test]
    fn push_tool_result_creates_user_message() {
        let mut c = make_conv();
        c.push_user("Do something");
        c.push_tool_result("tc_1", "file contents", false);
        assert_eq!(c.len(), 2);
        assert_eq!(c.messages()[1].role, Role::User);
        assert!(c.messages()[1].content[0].is_tool_result());
    }

    #[test]
    fn push_assistant_tool_use() {
        let mut c = make_conv();
        c.push_user("Read a file");
        c.push_assistant_tool_use(vec![
            ContentBlock::text("Let me read that."),
            ContentBlock::tool_use("tc_1", "read_file", serde_json::json!({"path": "/tmp/x"})),
        ]);
        assert_eq!(c.len(), 2);
        assert!(c.messages()[1].has_tool_use());
    }

    #[test]
    fn record_usage_accumulates() {
        let mut c = make_conv();
        c.record_usage(TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        });
        c.record_usage(TokenUsage {
            input_tokens: 200,
            output_tokens: 100,
            ..Default::default()
        });
        assert_eq!(c.total_usage.input_tokens, 300);
        assert_eq!(c.total_usage.output_tokens, 150);
    }

    #[test]
    fn needs_compaction_false_when_empty() {
        let c = make_conv();
        assert!(!c.needs_compaction());
    }

    #[test]
    fn needs_compaction_false_when_zero_window() {
        let c = Conversation::new("s".into(), String::new(), 0);
        assert!(!c.needs_compaction());
    }

    #[test]
    fn is_empty_and_len() {
        let mut c = make_conv();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        c.push_user("hi");
        assert!(!c.is_empty());
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn default_conversation() {
        let c = Conversation::default();
        assert!(c.id.is_empty());
        assert!(c.system_prompt.is_empty());
        assert_eq!(c.context_window, 0);
        assert!(c.is_empty());
    }

    #[test]
    fn estimated_tokens_increases_with_content() {
        let mut c = make_conv();
        let t0 = c.estimated_tokens();
        c.push_user("Hello world, this is a test message");
        let t1 = c.estimated_tokens();
        assert!(t1 > t0);
        c.push_assistant("Thank you for your message, I'll help with that.");
        let t2 = c.estimated_tokens();
        assert!(t2 > t1);
    }

    #[test]
    fn needs_compaction_true_for_large_conversation() {
        // context_window = 100, fill with lots of content
        let mut c = Conversation::new("s".into(), String::new(), 100);
        let big_text = "x".repeat(500); // ~125 tokens >> 80% of 100
        c.push_user(&big_text);
        assert!(c.needs_compaction());
    }

    #[test]
    fn turn_count_tracks_user_assistant_pairs() {
        let mut c = make_conv();
        assert_eq!(c.turn_count(), 0);
        c.push_user("q1");
        c.push_assistant("a1");
        assert_eq!(c.turn_count(), 1);
        c.push_user("q2");
        c.push_assistant("a2");
        assert_eq!(c.turn_count(), 2);
    }

    #[test]
    fn messages_returns_correct_slice() {
        let mut c = make_conv();
        c.push_user("first");
        c.push_assistant("second");
        let msgs = c.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }
}
