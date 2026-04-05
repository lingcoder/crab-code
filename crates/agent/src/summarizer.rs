//! Conversation summarizer: extracts key decisions, code changes, and
//! unresolved issues from a conversation history.
//!
//! Used for context compression and long-term memory storage.

use std::fmt::Write;

use crab_core::message::{ContentBlock, Message, Role};

// ── Summary types ─────────────────────────────────────────────────────

/// A category of extracted item from conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SummaryItemKind {
    /// A decision made during the conversation.
    Decision,
    /// A code change (file written/edited).
    CodeChange,
    /// An unresolved issue or open question.
    UnresolvedIssue,
    /// A tool invocation and its outcome.
    ToolAction,
    /// A key topic discussed.
    Topic,
}

impl std::fmt::Display for SummaryItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decision => write!(f, "decision"),
            Self::CodeChange => write!(f, "code_change"),
            Self::UnresolvedIssue => write!(f, "unresolved"),
            Self::ToolAction => write!(f, "tool_action"),
            Self::Topic => write!(f, "topic"),
        }
    }
}

/// A single extracted summary item.
#[derive(Debug, Clone)]
pub struct SummaryItem {
    pub kind: SummaryItemKind,
    pub content: String,
    /// Which turn (user-assistant pair) this was extracted from.
    pub turn: usize,
}

/// A complete conversation summary.
#[derive(Debug, Clone)]
pub struct ConversationSummary {
    pub items: Vec<SummaryItem>,
    /// Total number of turns summarized.
    pub turns_summarized: usize,
    /// Total number of messages processed.
    pub messages_processed: usize,
}

impl ConversationSummary {
    /// Get items of a specific kind.
    #[must_use]
    pub fn items_of_kind(&self, kind: SummaryItemKind) -> Vec<&SummaryItem> {
        self.items.iter().filter(|i| i.kind == kind).collect()
    }

    /// Get all decisions.
    #[must_use]
    pub fn decisions(&self) -> Vec<&SummaryItem> {
        self.items_of_kind(SummaryItemKind::Decision)
    }

    /// Get all code changes.
    #[must_use]
    pub fn code_changes(&self) -> Vec<&SummaryItem> {
        self.items_of_kind(SummaryItemKind::CodeChange)
    }

    /// Get all unresolved issues.
    #[must_use]
    pub fn unresolved_issues(&self) -> Vec<&SummaryItem> {
        self.items_of_kind(SummaryItemKind::UnresolvedIssue)
    }

    /// Format the summary as a compact text block.
    #[must_use]
    pub fn to_compact_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Conversation summary ({} turns, {} items):\n",
            self.turns_summarized,
            self.items.len(),
        );

        let sections = [
            ("Decisions", SummaryItemKind::Decision),
            ("Code Changes", SummaryItemKind::CodeChange),
            ("Tool Actions", SummaryItemKind::ToolAction),
            ("Topics", SummaryItemKind::Topic),
            ("Unresolved Issues", SummaryItemKind::UnresolvedIssue),
        ];

        for (label, kind) in sections {
            let items = self.items_of_kind(kind);
            if !items.is_empty() {
                let _ = writeln!(out, "{label}:");
                for item in items {
                    let _ = writeln!(out, "  - {} (turn {})", item.content, item.turn);
                }
                let _ = writeln!(out);
            }
        }

        out
    }

    /// Format as a prompt section for injection into system prompt.
    #[must_use]
    pub fn to_prompt_section(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        let _ = writeln!(out, "# Previous Conversation Summary\n");

        let decisions = self.decisions();
        if !decisions.is_empty() {
            let _ = writeln!(out, "Key decisions:");
            for d in decisions {
                let _ = writeln!(out, "- {}", d.content);
            }
            let _ = writeln!(out);
        }

        let changes = self.code_changes();
        if !changes.is_empty() {
            let _ = writeln!(out, "Code changes:");
            for c in changes {
                let _ = writeln!(out, "- {}", c.content);
            }
            let _ = writeln!(out);
        }

        let issues = self.unresolved_issues();
        if !issues.is_empty() {
            let _ = writeln!(out, "Open issues:");
            for i in issues {
                let _ = writeln!(out, "- {}", i.content);
            }
            let _ = writeln!(out);
        }

        out
    }
}

// ── Summarizer ────────────────────────────────────────────────────────

/// Configuration for the summarizer.
#[derive(Debug, Clone)]
pub struct SummarizerConfig {
    /// Maximum items to extract per kind.
    pub max_items_per_kind: usize,
    /// Whether to track tool actions.
    pub include_tool_actions: bool,
    /// Whether to extract topics from user messages.
    pub include_topics: bool,
}

impl Default for SummarizerConfig {
    fn default() -> Self {
        Self {
            max_items_per_kind: 20,
            include_tool_actions: true,
            include_topics: true,
        }
    }
}

/// Extracts a structured summary from conversation messages.
///
/// This is a heuristic-based summarizer (no LLM call). It scans messages
/// for patterns indicating decisions, code changes, and issues.
#[must_use]
pub fn summarize_conversation(
    messages: &[Message],
    config: &SummarizerConfig,
) -> ConversationSummary {
    let mut items = Vec::new();
    let mut turn = 0;
    let mut decision_count = 0;
    let mut change_count = 0;
    let mut issue_count = 0;
    let mut tool_count = 0;
    let mut topic_count = 0;

    for msg in messages {
        if msg.role == Role::User {
            turn += 1;

            // Extract topics from user messages
            if config.include_topics && topic_count < config.max_items_per_kind
                && let Some(topic) = extract_topic(msg) {
                    items.push(SummaryItem {
                        kind: SummaryItemKind::Topic,
                        content: topic,
                        turn,
                    });
                    topic_count += 1;
                }

            // Check for unresolved issues in user messages
            if issue_count < config.max_items_per_kind {
                for issue in extract_issues_from_user(msg) {
                    items.push(SummaryItem {
                        kind: SummaryItemKind::UnresolvedIssue,
                        content: issue,
                        turn,
                    });
                    issue_count += 1;
                    if issue_count >= config.max_items_per_kind {
                        break;
                    }
                }
            }
        }

        if msg.role == Role::Assistant {
            // Extract decisions from assistant text
            if decision_count < config.max_items_per_kind {
                for decision in extract_decisions(msg) {
                    items.push(SummaryItem {
                        kind: SummaryItemKind::Decision,
                        content: decision,
                        turn,
                    });
                    decision_count += 1;
                    if decision_count >= config.max_items_per_kind {
                        break;
                    }
                }
            }

            // Extract code changes from tool use
            if change_count < config.max_items_per_kind {
                for change in extract_code_changes(msg) {
                    items.push(SummaryItem {
                        kind: SummaryItemKind::CodeChange,
                        content: change,
                        turn,
                    });
                    change_count += 1;
                    if change_count >= config.max_items_per_kind {
                        break;
                    }
                }
            }

            // Track tool actions
            if config.include_tool_actions && tool_count < config.max_items_per_kind {
                for action in extract_tool_actions(msg) {
                    items.push(SummaryItem {
                        kind: SummaryItemKind::ToolAction,
                        content: action,
                        turn,
                    });
                    tool_count += 1;
                    if tool_count >= config.max_items_per_kind {
                        break;
                    }
                }
            }
        }
    }

    ConversationSummary {
        items,
        turns_summarized: turn,
        messages_processed: messages.len(),
    }
}

// ── Extraction helpers ────────────────────────────────────────────────

/// Decision signal phrases in assistant text.
const DECISION_SIGNALS: &[&str] = &[
    "I'll ",
    "I will ",
    "Let's ",
    "We should ",
    "The approach is ",
    "I decided ",
    "Going with ",
    "The plan is ",
    "Instead, ",
    "Rather than ",
];

/// Extract decision-like statements from assistant messages.
fn extract_decisions(msg: &Message) -> Vec<String> {
    let mut decisions = Vec::new();
    for block in &msg.content {
        if let ContentBlock::Text { text } = block {
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.len() < 10 {
                    continue;
                }
                for signal in DECISION_SIGNALS {
                    if trimmed.contains(signal) {
                        let decision = truncate_line(trimmed, 120);
                        decisions.push(decision);
                        break;
                    }
                }
            }
        }
    }
    decisions
}

/// Extract code change descriptions from tool use blocks.
fn extract_code_changes(msg: &Message) -> Vec<String> {
    let mut changes = Vec::new();
    for block in &msg.content {
        if let ContentBlock::ToolUse { name, input, .. } = block {
            match name.as_str() {
                "write" | "write_file" => {
                    if let Some(path) = input.get("file_path").or(input.get("path")).and_then(|v| v.as_str()) {
                        changes.push(format!("Wrote file: {path}"));
                    }
                }
                "edit" | "edit_file" => {
                    if let Some(path) = input.get("file_path").or(input.get("path")).and_then(|v| v.as_str()) {
                        changes.push(format!("Edited file: {path}"));
                    }
                }
                "bash" => {
                    if let Some(cmd) = input.get("command").and_then(|v| v.as_str())
                        && (cmd.starts_with("git commit") || cmd.starts_with("git add")) {
                            changes.push(format!("Git operation: {}", truncate_line(cmd, 80)));
                        }
                }
                _ => {}
            }
        }
    }
    changes
}

/// Extract tool action descriptions.
fn extract_tool_actions(msg: &Message) -> Vec<String> {
    let mut actions = Vec::new();
    for block in &msg.content {
        if let ContentBlock::ToolUse { name, input, .. } = block {
            let detail = match name.as_str() {
                "read" | "read_file" => {
                    input.get("file_path").or(input.get("path")).and_then(|v| v.as_str())
                        .map(|p| format!("Read: {p}"))
                }
                "glob" => {
                    input.get("pattern").and_then(|v| v.as_str())
                        .map(|p| format!("Glob: {p}"))
                }
                "grep" => {
                    input.get("pattern").and_then(|v| v.as_str())
                        .map(|p| format!("Grep: {}", truncate_line(p, 60)))
                }
                "bash" => {
                    input.get("command").and_then(|v| v.as_str())
                        .map(|c| format!("Bash: {}", truncate_line(c, 60)))
                }
                _ => Some(format!("Tool: {name}")),
            };
            if let Some(d) = detail {
                actions.push(d);
            }
        }
    }
    actions
}

/// Issue signal phrases in user messages.
const ISSUE_SIGNALS: &[&str] = &[
    "still broken",
    "doesn't work",
    "not working",
    "fails",
    "error",
    "bug",
    "problem",
    "issue",
    "TODO",
    "FIXME",
    "HACK",
];

/// Extract potential unresolved issues from user messages.
fn extract_issues_from_user(msg: &Message) -> Vec<String> {
    let mut issues = Vec::new();
    for block in &msg.content {
        if let ContentBlock::Text { text } = block {
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.len() < 8 {
                    continue;
                }
                let lower = trimmed.to_lowercase();
                for signal in ISSUE_SIGNALS {
                    if lower.contains(&signal.to_lowercase()) {
                        issues.push(truncate_line(trimmed, 120));
                        break;
                    }
                }
            }
        }
    }
    issues
}

/// Extract a topic description from user message (first substantive line).
fn extract_topic(msg: &Message) -> Option<String> {
    for block in &msg.content {
        if let ContentBlock::Text { text } = block {
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.len() >= 10 && !trimmed.starts_with('/') {
                    return Some(truncate_line(trimmed, 100));
                }
            }
        }
    }
    None
}

/// Truncate a line to max length, adding "..." if truncated.
fn truncate_line(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant_msg(text: &str) -> Message {
        Message::assistant(text)
    }

    fn assistant_with_tool(name: &str, input: serde_json::Value) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use("tc_1", name, input)],
        )
    }

    fn assistant_text_and_tool(text: &str, name: &str, input: serde_json::Value) -> Message {
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text(text),
                ContentBlock::tool_use("tc_1", name, input),
            ],
        )
    }

    // ── SummaryItemKind ────────────────────────────────────────────

    #[test]
    fn summary_item_kind_display() {
        assert_eq!(SummaryItemKind::Decision.to_string(), "decision");
        assert_eq!(SummaryItemKind::CodeChange.to_string(), "code_change");
        assert_eq!(SummaryItemKind::UnresolvedIssue.to_string(), "unresolved");
        assert_eq!(SummaryItemKind::ToolAction.to_string(), "tool_action");
        assert_eq!(SummaryItemKind::Topic.to_string(), "topic");
    }

    // ── extract_decisions ──────────────────────────────────────────

    #[test]
    fn extract_decisions_basic() {
        let msg = assistant_msg("I'll refactor the module to use a trait-based approach.");
        let decisions = extract_decisions(&msg);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].contains("refactor"));
    }

    #[test]
    fn extract_decisions_multiple_signals() {
        let msg = assistant_msg("I'll fix the bug.\nLet's also add tests for edge cases.");
        let decisions = extract_decisions(&msg);
        assert_eq!(decisions.len(), 2);
    }

    #[test]
    fn extract_decisions_ignores_short_lines() {
        let msg = assistant_msg("I'll do.");
        let decisions = extract_decisions(&msg);
        assert!(decisions.is_empty());
    }

    #[test]
    fn extract_decisions_no_signals() {
        let msg = assistant_msg("The function returns a Result<(), Error> type.");
        let decisions = extract_decisions(&msg);
        assert!(decisions.is_empty());
    }

    // ── extract_code_changes ───────────────────────────────────────

    #[test]
    fn extract_write_change() {
        let msg = assistant_with_tool(
            "write",
            serde_json::json!({"file_path": "src/main.rs", "content": "fn main() {}"}),
        );
        let changes = extract_code_changes(&msg);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("Wrote file: src/main.rs"));
    }

    #[test]
    fn extract_edit_change() {
        let msg = assistant_with_tool(
            "edit",
            serde_json::json!({"file_path": "src/lib.rs", "old_string": "a", "new_string": "b"}),
        );
        let changes = extract_code_changes(&msg);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("Edited file: src/lib.rs"));
    }

    #[test]
    fn extract_git_commit_change() {
        let msg = assistant_with_tool(
            "bash",
            serde_json::json!({"command": "git commit -m 'fix bug'"}),
        );
        let changes = extract_code_changes(&msg);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("Git operation"));
    }

    #[test]
    fn extract_no_change_for_read() {
        let msg = assistant_with_tool(
            "read",
            serde_json::json!({"file_path": "src/main.rs"}),
        );
        let changes = extract_code_changes(&msg);
        assert!(changes.is_empty());
    }

    // ── extract_tool_actions ───────────────────────────────────────

    #[test]
    fn extract_read_action() {
        let msg = assistant_with_tool(
            "read",
            serde_json::json!({"file_path": "src/lib.rs"}),
        );
        let actions = extract_tool_actions(&msg);
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("Read: src/lib.rs"));
    }

    #[test]
    fn extract_grep_action() {
        let msg = assistant_with_tool(
            "grep",
            serde_json::json!({"pattern": "fn main"}),
        );
        let actions = extract_tool_actions(&msg);
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("Grep: fn main"));
    }

    #[test]
    fn extract_glob_action() {
        let msg = assistant_with_tool(
            "glob",
            serde_json::json!({"pattern": "**/*.rs"}),
        );
        let actions = extract_tool_actions(&msg);
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("Glob: **/*.rs"));
    }

    #[test]
    fn extract_unknown_tool_action() {
        let msg = assistant_with_tool(
            "custom_tool",
            serde_json::json!({}),
        );
        let actions = extract_tool_actions(&msg);
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("Tool: custom_tool"));
    }

    // ── extract_issues_from_user ───────────────────────────────────

    #[test]
    fn extract_issue_error() {
        let msg = user_msg("The build still shows an error after the fix");
        let issues = extract_issues_from_user(&msg);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn extract_issue_not_working() {
        let msg = user_msg("The search feature is not working correctly");
        let issues = extract_issues_from_user(&msg);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn extract_no_issue_from_normal_text() {
        let msg = user_msg("Can you explain how the agent loop works?");
        let issues = extract_issues_from_user(&msg);
        assert!(issues.is_empty());
    }

    #[test]
    fn extract_issue_short_line_ignored() {
        let msg = user_msg("error");
        let issues = extract_issues_from_user(&msg);
        assert!(issues.is_empty());
    }

    // ── extract_topic ──────────────────────────────────────────────

    #[test]
    fn extract_topic_basic() {
        let msg = user_msg("Can you help me refactor the authentication module?");
        let topic = extract_topic(&msg);
        assert!(topic.is_some());
        assert!(topic.unwrap().contains("refactor"));
    }

    #[test]
    fn extract_topic_skips_commands() {
        let msg = user_msg("/undo 3\nNow fix the remaining tests");
        let topic = extract_topic(&msg);
        assert!(topic.is_some());
        assert!(topic.unwrap().contains("fix the remaining"));
    }

    #[test]
    fn extract_topic_none_for_short() {
        let msg = user_msg("hi");
        let topic = extract_topic(&msg);
        assert!(topic.is_none());
    }

    // ── truncate_line ──────────────────────────────────────────────

    #[test]
    fn truncate_short_line() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_line() {
        let long = "a".repeat(50);
        let result = truncate_line(&long, 20);
        assert!(result.len() <= 20);
        assert!(result.ends_with("..."));
    }

    // ── summarize_conversation ─────────────────────────────────────

    #[test]
    fn summarize_empty_conversation() {
        let summary = summarize_conversation(&[], &SummarizerConfig::default());
        assert!(summary.items.is_empty());
        assert_eq!(summary.turns_summarized, 0);
        assert_eq!(summary.messages_processed, 0);
    }

    #[test]
    fn summarize_single_turn() {
        let messages = vec![
            user_msg("Fix the bug in the authentication module"),
            assistant_msg("I'll fix the auth bug by updating the token validation logic."),
        ];
        let summary = summarize_conversation(&messages, &SummarizerConfig::default());
        assert_eq!(summary.turns_summarized, 1);
        assert_eq!(summary.messages_processed, 2);
        assert!(!summary.decisions().is_empty());
        assert!(!summary.items_of_kind(SummaryItemKind::Topic).is_empty());
    }

    #[test]
    fn summarize_with_tool_use() {
        let messages = vec![
            user_msg("Read the main.rs file"),
            assistant_text_and_tool(
                "I'll read the file for you.",
                "read",
                serde_json::json!({"file_path": "src/main.rs"}),
            ),
        ];
        let summary = summarize_conversation(&messages, &SummarizerConfig::default());
        assert!(!summary.items_of_kind(SummaryItemKind::ToolAction).is_empty());
    }

    #[test]
    fn summarize_with_code_changes() {
        let messages = vec![
            user_msg("Create a new config module"),
            assistant_with_tool(
                "write",
                serde_json::json!({"file_path": "src/config.rs", "content": "pub struct Config {}"}),
            ),
        ];
        let summary = summarize_conversation(&messages, &SummarizerConfig::default());
        assert!(!summary.code_changes().is_empty());
    }

    #[test]
    fn summarize_with_unresolved_issues() {
        let messages = vec![
            user_msg("The tests are still broken after your changes"),
            assistant_msg("I'll investigate the test failures."),
        ];
        let summary = summarize_conversation(&messages, &SummarizerConfig::default());
        assert!(!summary.unresolved_issues().is_empty());
    }

    #[test]
    fn summarize_respects_max_items() {
        let mut messages = Vec::new();
        for i in 0..30 {
            messages.push(user_msg(&format!("Fix error number {i} in the code")));
            messages.push(assistant_msg(&format!("I'll fix error {i} right away.")));
        }
        let config = SummarizerConfig {
            max_items_per_kind: 5,
            ..Default::default()
        };
        let summary = summarize_conversation(&messages, &config);
        assert!(summary.decisions().len() <= 5);
        assert!(summary.unresolved_issues().len() <= 5);
    }

    #[test]
    fn summarize_no_tool_actions_when_disabled() {
        let messages = vec![
            user_msg("Read main.rs"),
            assistant_with_tool("read", serde_json::json!({"file_path": "main.rs"})),
        ];
        let config = SummarizerConfig {
            include_tool_actions: false,
            ..Default::default()
        };
        let summary = summarize_conversation(&messages, &config);
        assert!(summary.items_of_kind(SummaryItemKind::ToolAction).is_empty());
    }

    #[test]
    fn summarize_no_topics_when_disabled() {
        let messages = vec![
            user_msg("Can you explain the architecture of this project?"),
            assistant_msg("The project has four layers."),
        ];
        let config = SummarizerConfig {
            include_topics: false,
            ..Default::default()
        };
        let summary = summarize_conversation(&messages, &config);
        assert!(summary.items_of_kind(SummaryItemKind::Topic).is_empty());
    }

    // ── ConversationSummary formatting ─────────────────────────────

    #[test]
    fn compact_text_empty() {
        let summary = ConversationSummary {
            items: Vec::new(),
            turns_summarized: 0,
            messages_processed: 0,
        };
        let text = summary.to_compact_text();
        assert!(text.contains("0 turns"));
    }

    #[test]
    fn compact_text_with_items() {
        let summary = ConversationSummary {
            items: vec![
                SummaryItem {
                    kind: SummaryItemKind::Decision,
                    content: "Use trait-based approach".into(),
                    turn: 1,
                },
                SummaryItem {
                    kind: SummaryItemKind::CodeChange,
                    content: "Wrote src/lib.rs".into(),
                    turn: 2,
                },
            ],
            turns_summarized: 2,
            messages_processed: 4,
        };
        let text = summary.to_compact_text();
        assert!(text.contains("Decisions:"));
        assert!(text.contains("Code Changes:"));
        assert!(text.contains("trait-based approach"));
    }

    #[test]
    fn prompt_section_empty() {
        let summary = ConversationSummary {
            items: Vec::new(),
            turns_summarized: 0,
            messages_processed: 0,
        };
        assert!(summary.to_prompt_section().is_empty());
    }

    #[test]
    fn prompt_section_with_decisions() {
        let summary = ConversationSummary {
            items: vec![SummaryItem {
                kind: SummaryItemKind::Decision,
                content: "Refactor auth module".into(),
                turn: 1,
            }],
            turns_summarized: 1,
            messages_processed: 2,
        };
        let section = summary.to_prompt_section();
        assert!(section.contains("Previous Conversation Summary"));
        assert!(section.contains("Key decisions:"));
        assert!(section.contains("Refactor auth module"));
    }

    #[test]
    fn prompt_section_with_all_kinds() {
        let summary = ConversationSummary {
            items: vec![
                SummaryItem { kind: SummaryItemKind::Decision, content: "Use async".into(), turn: 1 },
                SummaryItem { kind: SummaryItemKind::CodeChange, content: "Wrote lib.rs".into(), turn: 1 },
                SummaryItem { kind: SummaryItemKind::UnresolvedIssue, content: "Tests fail".into(), turn: 2 },
            ],
            turns_summarized: 2,
            messages_processed: 4,
        };
        let section = summary.to_prompt_section();
        assert!(section.contains("Key decisions:"));
        assert!(section.contains("Code changes:"));
        assert!(section.contains("Open issues:"));
    }

    // ── Config defaults ────────────────────────────────────────────

    #[test]
    fn summarizer_config_defaults() {
        let config = SummarizerConfig::default();
        assert_eq!(config.max_items_per_kind, 20);
        assert!(config.include_tool_actions);
        assert!(config.include_topics);
    }

    // ── Multi-turn integration ─────────────────────────────────────

    #[test]
    fn summarize_multi_turn_conversation() {
        let messages = vec![
            user_msg("Help me build a REST API with authentication"),
            assistant_msg("I'll create a REST API using axum with JWT authentication."),
            user_msg("The JWT validation doesn't work for expired tokens"),
            assistant_text_and_tool(
                "I'll fix the expiry check.",
                "edit",
                serde_json::json!({"file_path": "src/auth.rs", "old_string": "a", "new_string": "b"}),
            ),
            user_msg("Now add rate limiting"),
            assistant_with_tool(
                "write",
                serde_json::json!({"file_path": "src/rate_limit.rs", "content": "pub fn limit() {}"}),
            ),
        ];
        let summary = summarize_conversation(&messages, &SummarizerConfig::default());
        assert_eq!(summary.turns_summarized, 3);
        assert_eq!(summary.messages_processed, 6);
        assert!(!summary.decisions().is_empty());
        assert!(!summary.code_changes().is_empty());
    }
}
