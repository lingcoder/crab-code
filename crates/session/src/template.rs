//! Session templates and quick resume.
//!
//! Provides [`SessionTemplate`] — predefined session configurations for common
//! workflows (code review, bug fix, feature development, research). Also
//! provides [`QuickResume`] for listing and resuming recently-used sessions
//! with summary previews.

use serde::{Deserialize, Serialize};

use crab_tools::builtin::bash::BASH_TOOL_NAME;
use crab_tools::builtin::edit::EDIT_TOOL_NAME;
use crab_tools::builtin::glob::GLOB_TOOL_NAME;
use crab_tools::builtin::grep::GREP_TOOL_NAME;
use crab_tools::builtin::read::READ_TOOL_NAME;
use crab_tools::builtin::write::WRITE_TOOL_NAME;

use crate::history::SessionHistory;

// ── Session template ─────────────────────────────────────────────────

/// Predefined session workflow type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Code review workflow — focuses on reading, analyzing, and commenting.
    CodeReview,
    /// Bug fix workflow — focuses on reproducing, diagnosing, and patching.
    BugFix,
    /// Feature development — focuses on planning, implementing, and testing.
    Feature,
    /// Research / exploration — focuses on understanding code and gathering info.
    Research,
    /// Custom / unclassified.
    Custom,
}

impl std::fmt::Display for SessionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CodeReview => write!(f, "code_review"),
            Self::BugFix => write!(f, "bug_fix"),
            Self::Feature => write!(f, "feature"),
            Self::Research => write!(f, "research"),
            Self::Custom => write!(f, "custom"),
        }
    }
}

/// A session template that pre-configures the system prompt and suggested tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTemplate {
    /// Template kind.
    pub kind: SessionKind,
    /// Human-readable name.
    pub name: String,
    /// Description of what this template is for.
    pub description: String,
    /// System prompt preamble injected at the start of the session.
    pub system_prompt_prefix: String,
    /// Suggested tools to prioritize (informational, not enforced).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_tools: Vec<String>,
}

impl SessionTemplate {
    /// Create a new custom template.
    #[must_use]
    pub fn new(
        kind: SessionKind,
        name: impl Into<String>,
        description: impl Into<String>,
        system_prompt_prefix: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            name: name.into(),
            description: description.into(),
            system_prompt_prefix: system_prompt_prefix.into(),
            suggested_tools: Vec::new(),
        }
    }

    /// Add suggested tools to this template.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.suggested_tools = tools;
        self
    }
}

/// Return all built-in session templates.
#[must_use]
pub fn builtin_templates() -> Vec<SessionTemplate> {
    vec![
        SessionTemplate::new(
            SessionKind::CodeReview,
            "Code Review",
            "Review code changes for correctness, style, and potential issues",
            "You are performing a code review. Focus on correctness, readability, \
             performance, and security. Point out issues and suggest improvements. \
             Be constructive and specific.",
        )
        .with_tools(vec![
            READ_TOOL_NAME.into(),
            GREP_TOOL_NAME.into(),
            GLOB_TOOL_NAME.into(),
            BASH_TOOL_NAME.into(),
        ]),
        SessionTemplate::new(
            SessionKind::BugFix,
            "Bug Fix",
            "Diagnose and fix a bug in the codebase",
            "You are diagnosing and fixing a bug. Start by understanding the symptoms, \
             reproduce the issue if possible, identify the root cause, implement the fix, \
             and verify with tests.",
        )
        .with_tools(vec![
            READ_TOOL_NAME.into(),
            GREP_TOOL_NAME.into(),
            BASH_TOOL_NAME.into(),
            EDIT_TOOL_NAME.into(),
            WRITE_TOOL_NAME.into(),
        ]),
        SessionTemplate::new(
            SessionKind::Feature,
            "Feature Development",
            "Plan and implement a new feature",
            "You are implementing a new feature. Start with understanding requirements, \
             plan the approach, implement incrementally, write tests, and ensure the code \
             compiles and passes linting.",
        )
        .with_tools(vec![
            READ_TOOL_NAME.into(),
            WRITE_TOOL_NAME.into(),
            EDIT_TOOL_NAME.into(),
            BASH_TOOL_NAME.into(),
            GLOB_TOOL_NAME.into(),
            GREP_TOOL_NAME.into(),
        ]),
        SessionTemplate::new(
            SessionKind::Research,
            "Research",
            "Explore and understand a codebase or topic",
            "You are researching and exploring. Focus on understanding the architecture, \
             finding relevant code, summarizing patterns, and answering questions. \
             Avoid making changes unless explicitly asked.",
        )
        .with_tools(vec![
            READ_TOOL_NAME.into(),
            GREP_TOOL_NAME.into(),
            GLOB_TOOL_NAME.into(),
            BASH_TOOL_NAME.into(),
        ]),
    ]
}

/// Find a built-in template by kind.
#[must_use]
pub fn find_template(kind: SessionKind) -> Option<SessionTemplate> {
    builtin_templates().into_iter().find(|t| t.kind == kind)
}

/// Find a template by name (case-insensitive).
#[must_use]
pub fn find_template_by_name(name: &str) -> Option<SessionTemplate> {
    let lower = name.to_lowercase();
    builtin_templates()
        .into_iter()
        .find(|t| t.name.to_lowercase() == lower)
}

// ── Quick resume ─────────────────────────────────────────────────────

/// A summary of a session suitable for display in a "quick resume" list.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    /// The session ID.
    pub session_id: String,
    /// Number of messages in the session.
    pub message_count: usize,
    /// First user message text (truncated preview).
    pub preview: String,
    /// File size in bytes (proxy for recency / activity).
    pub file_size: u64,
}

/// List recent sessions with preview summaries for quick resume.
///
/// Returns up to `limit` sessions, sorted by file size descending (as a
/// rough proxy for most active / most recent sessions).
///
/// # Errors
///
/// Returns an error if the session directory cannot be read.
pub fn quick_resume_list(
    history: &SessionHistory,
    limit: usize,
) -> crab_common::Result<Vec<SessionSummary>> {
    let session_ids = history.list_sessions()?;
    let mut summaries = Vec::new();

    for sid in &session_ids {
        let path = history.base_dir.join(format!("{sid}.json"));
        let file_size = path.metadata().map_or(0, |m| m.len());

        let preview = match history.load(sid)? {
            Some(messages) => {
                let message_count = messages.len();
                let first_user = messages
                    .iter()
                    .find(|m| m.role == crab_core::message::Role::User)
                    .and_then(|m| {
                        m.content.iter().find_map(|b| match b {
                            crab_core::message::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                    })
                    .unwrap_or("");
                let preview = if first_user.len() > 100 {
                    format!("{}...", &first_user[..100])
                } else {
                    first_user.to_string()
                };
                SessionSummary {
                    session_id: sid.clone(),
                    message_count,
                    preview,
                    file_size,
                }
            }
            None => continue,
        };
        summaries.push(preview);
    }

    // Sort by file size descending (largest / most active first)
    summaries.sort_by(|a, b| b.file_size.cmp(&a.file_size));
    summaries.truncate(limit);
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Message;

    // ── SessionKind ──────────────────────────────────────────────────

    #[test]
    fn session_kind_display() {
        assert_eq!(SessionKind::CodeReview.to_string(), "code_review");
        assert_eq!(SessionKind::BugFix.to_string(), "bug_fix");
        assert_eq!(SessionKind::Feature.to_string(), "feature");
        assert_eq!(SessionKind::Research.to_string(), "research");
        assert_eq!(SessionKind::Custom.to_string(), "custom");
    }

    #[test]
    fn session_kind_serde_roundtrip() {
        let json = serde_json::to_string(&SessionKind::BugFix).unwrap();
        assert_eq!(json, r#""bug_fix""#);
        let back: SessionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SessionKind::BugFix);
    }

    // ── SessionTemplate ──────────────────────────────────────────────

    #[test]
    fn template_new_and_with_tools() {
        let t = SessionTemplate::new(
            SessionKind::Custom,
            "My Template",
            "Does stuff",
            "Be helpful.",
        )
        .with_tools(vec!["bash".into()]);

        assert_eq!(t.kind, SessionKind::Custom);
        assert_eq!(t.name, "My Template");
        assert_eq!(t.suggested_tools, vec!["bash"]);
    }

    #[test]
    fn template_serde_roundtrip() {
        let t = SessionTemplate::new(
            SessionKind::Feature,
            "Feature",
            "Build stuff",
            "You are building.",
        );
        let json = serde_json::to_string(&t).unwrap();
        let back: SessionTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, SessionKind::Feature);
        assert_eq!(back.name, "Feature");
    }

    // ── Built-in templates ───────────────────────────────────────────

    #[test]
    fn builtin_templates_has_four() {
        let templates = builtin_templates();
        assert_eq!(templates.len(), 4);
    }

    #[test]
    fn builtin_template_kinds_are_unique() {
        let templates = builtin_templates();
        let kinds: Vec<SessionKind> = templates.iter().map(|t| t.kind).collect();
        let mut deduped = kinds.clone();
        deduped.dedup();
        assert_eq!(kinds.len(), deduped.len());
    }

    #[test]
    fn find_template_by_kind() {
        let t = find_template(SessionKind::CodeReview).unwrap();
        assert_eq!(t.kind, SessionKind::CodeReview);
        assert!(!t.system_prompt_prefix.is_empty());
    }

    #[test]
    fn find_template_custom_returns_none() {
        assert!(find_template(SessionKind::Custom).is_none());
    }

    #[test]
    fn find_template_by_name_case_insensitive() {
        let t = find_template_by_name("bug fix").unwrap();
        assert_eq!(t.kind, SessionKind::BugFix);
    }

    #[test]
    fn find_template_by_name_not_found() {
        assert!(find_template_by_name("nonexistent").is_none());
    }

    #[test]
    fn all_builtin_templates_have_suggested_tools() {
        for t in builtin_templates() {
            assert!(!t.suggested_tools.is_empty(), "{} has no tools", t.name);
        }
    }

    #[test]
    fn all_builtin_templates_have_nonempty_fields() {
        for t in builtin_templates() {
            assert!(!t.name.is_empty());
            assert!(!t.description.is_empty());
            assert!(!t.system_prompt_prefix.is_empty());
        }
    }

    // ── Quick resume ─────────────────────────────────────────────────

    #[test]
    fn quick_resume_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let summaries = quick_resume_list(&history, 5).unwrap();
        assert!(summaries.is_empty());
    }

    #[test]
    fn quick_resume_returns_summaries() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save("s1", &[Message::user("Fix the login bug")])
            .unwrap();
        history
            .save(
                "s2",
                &[
                    Message::user("Add feature X"),
                    Message::assistant("On it"),
                    Message::user("Thanks"),
                ],
            )
            .unwrap();

        let summaries = quick_resume_list(&history, 10).unwrap();
        assert_eq!(summaries.len(), 2);
    }

    #[test]
    fn quick_resume_preview_from_first_user_message() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save("s1", &[Message::user("My specific request")])
            .unwrap();

        let summaries = quick_resume_list(&history, 5).unwrap();
        assert_eq!(summaries[0].preview, "My specific request");
    }

    #[test]
    fn quick_resume_preview_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        let long = "x".repeat(200);
        history.save("s1", &[Message::user(&long)]).unwrap();

        let summaries = quick_resume_list(&history, 5).unwrap();
        assert!(summaries[0].preview.ends_with("..."));
        assert!(summaries[0].preview.len() <= 104);
    }

    #[test]
    fn quick_resume_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        for i in 0..5 {
            history
                .save(&format!("s{i}"), &[Message::user("hi")])
                .unwrap();
        }

        let summaries = quick_resume_list(&history, 3).unwrap();
        assert_eq!(summaries.len(), 3);
    }

    #[test]
    fn quick_resume_sorted_by_file_size_desc() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        // s1 = small, s2 = large
        history.save("s1", &[Message::user("hi")]).unwrap();
        let big_msgs: Vec<Message> = (0..20)
            .map(|i| Message::user(format!("msg {i} with some longer content to increase size")))
            .collect();
        history.save("s2", &big_msgs).unwrap();

        let summaries = quick_resume_list(&history, 10).unwrap();
        assert_eq!(summaries.len(), 2);
        // Largest file first
        assert!(summaries[0].file_size >= summaries[1].file_size);
    }

    #[test]
    fn session_summary_message_count() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save(
                "s1",
                &[
                    Message::user("q"),
                    Message::assistant("a"),
                    Message::user("q2"),
                ],
            )
            .unwrap();

        let summaries = quick_resume_list(&history, 5).unwrap();
        assert_eq!(summaries[0].message_count, 3);
    }
}
