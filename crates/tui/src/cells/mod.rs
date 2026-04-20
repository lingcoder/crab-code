pub mod assistant_message;
pub mod diff;
pub mod error;
pub mod plan_approval;
pub mod progress;
pub mod thinking;
pub mod tool_call;
pub mod tool_rejected;
pub mod user_message;

pub use assistant_message::AssistantMessage;
pub use diff::{DiffBlock, DiffHunk};
pub use error::ErrorInfo;
pub use plan_approval::PlanApproval;
pub use progress::ProgressInfo;
pub use thinking::ThinkingBlock;
pub use tool_call::{ToolCallState, ToolStatus};
pub use tool_rejected::ToolRejected;
pub use user_message::UserMessage;

use ratatui::text::Line;

#[derive(Debug, Clone)]
pub enum Cell {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolCall(ToolCallState),
    Thinking(ThinkingBlock),
    Diff(DiffBlock),
    Progress(ProgressInfo),
    Error(ErrorInfo),
    PlanApproval(PlanApproval),
    ToolRejected(ToolRejected),
}

impl Cell {
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self {
            Self::User(m) => m.render_lines(width),
            Self::Assistant(m) => m.render_lines(width),
            Self::ToolCall(m) => m.render_lines(width),
            Self::Thinking(m) => m.render_lines(width),
            Self::Diff(m) => m.render_lines(width),
            Self::Progress(m) => m.render_lines(width),
            Self::Error(m) => m.render_lines(width),
            Self::PlanApproval(m) => m.render_lines(width),
            Self::ToolRejected(m) => m.render_lines(width),
        }
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        match self {
            Self::User(m) => m.desired_height(width),
            Self::Assistant(m) => m.desired_height(width),
            Self::ToolCall(m) => m.desired_height(width),
            Self::Thinking(m) => m.desired_height(width),
            Self::Diff(m) => m.desired_height(width),
            Self::Progress(m) => m.desired_height(width),
            Self::Error(m) => m.desired_height(width),
            Self::PlanApproval(m) => m.desired_height(width),
            Self::ToolRejected(m) => m.desired_height(width),
        }
    }

    pub fn is_streaming(&self) -> bool {
        match self {
            Self::User(_) => false,
            Self::Assistant(m) => m.is_streaming(),
            Self::ToolCall(m) => m.is_streaming(),
            Self::Thinking(m) => m.is_streaming(),
            Self::Diff(m) => m.is_streaming(),
            Self::Progress(m) => m.is_streaming(),
            Self::Error(m) => m.is_streaming(),
            Self::PlanApproval(m) => m.is_streaming(),
            Self::ToolRejected(m) => m.is_streaming(),
        }
    }

    pub fn search_text(&self) -> String {
        match self {
            Self::User(m) => m.search_text(),
            Self::Assistant(m) => m.search_text(),
            Self::ToolCall(m) => m.search_text(),
            Self::Thinking(m) => m.search_text(),
            Self::Diff(m) => m.search_text(),
            Self::Progress(m) => m.search_text(),
            Self::Error(m) => m.search_text(),
            Self::PlanApproval(m) => m.search_text(),
            Self::ToolRejected(m) => m.search_text(),
        }
    }

    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Progress(_))
    }
}

impl Cell {
    #[must_use]
    pub fn from_chat_message(msg: &crate::app::ChatMessage) -> Self {
        use crate::app::ChatMessage;
        match msg {
            ChatMessage::User { text } => Self::User(UserMessage::new(text.clone())),
            ChatMessage::Assistant { text } => Self::Assistant(AssistantMessage::new(text.clone())),
            ChatMessage::ToolUse { name, summary } => {
                let mut tc = ToolCallState::new(name.clone());
                if let Some(s) = summary {
                    tc = tc.with_summary(s.clone());
                }
                tc.status = ToolStatus::Complete;
                Self::ToolCall(tc)
            }
            ChatMessage::ToolResult {
                tool_name,
                output,
                is_error,
                ..
            } => {
                let tc = ToolCallState::new(tool_name.clone())
                    .with_output(output.clone())
                    .with_status(if *is_error {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Complete
                    });
                Self::ToolCall(tc)
            }
            ChatMessage::System { text } => Self::Progress(ProgressInfo::new(text.clone())),
        }
    }
}

impl From<UserMessage> for Cell {
    fn from(m: UserMessage) -> Self {
        Self::User(m)
    }
}

impl From<AssistantMessage> for Cell {
    fn from(m: AssistantMessage) -> Self {
        Self::Assistant(m)
    }
}

impl From<ToolCallState> for Cell {
    fn from(m: ToolCallState) -> Self {
        Self::ToolCall(m)
    }
}

impl From<ThinkingBlock> for Cell {
    fn from(m: ThinkingBlock) -> Self {
        Self::Thinking(m)
    }
}

impl From<DiffBlock> for Cell {
    fn from(m: DiffBlock) -> Self {
        Self::Diff(m)
    }
}

impl From<ProgressInfo> for Cell {
    fn from(m: ProgressInfo) -> Self {
        Self::Progress(m)
    }
}

impl From<ErrorInfo> for Cell {
    fn from(m: ErrorInfo) -> Self {
        Self::Error(m)
    }
}

impl From<PlanApproval> for Cell {
    fn from(m: PlanApproval) -> Self {
        Self::PlanApproval(m)
    }
}

impl From<ToolRejected> for Cell {
    fn from(m: ToolRejected) -> Self {
        Self::ToolRejected(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_variants_render_nonempty() {
        let cells = vec![
            Cell::User(UserMessage::new("hi")),
            Cell::Assistant(AssistantMessage::new("hello")),
            Cell::ToolCall(ToolCallState::new("Bash").with_status(ToolStatus::Complete)),
            Cell::Thinking(ThinkingBlock::new("hmm")),
            Cell::Diff(DiffBlock {
                file_path: "f.rs".into(),
                hunks: vec![DiffHunk {
                    header: "@@ @@".into(),
                    lines: vec!["+added".into()],
                }],
                collapsed: false,
            }),
            Cell::Progress(ProgressInfo::new("loading")),
            Cell::Error(ErrorInfo::new("bad")),
            Cell::PlanApproval(PlanApproval::new("plan")),
            Cell::ToolRejected(ToolRejected::new("Bash")),
        ];

        for cell in &cells {
            let lines = cell.render_lines(80);
            assert!(!lines.is_empty(), "Cell {:?} produced no lines", cell);
            assert!(cell.desired_height(80) > 0);
            assert!(!cell.search_text().is_empty());
        }
    }

    #[test]
    fn streaming_cells() {
        assert!(!Cell::User(UserMessage::new("x")).is_streaming());
        assert!(Cell::Assistant(AssistantMessage::streaming("x")).is_streaming());
        assert!(Cell::Thinking(ThinkingBlock::streaming("x")).is_streaming());
        assert!(
            Cell::ToolCall(ToolCallState::new("B").with_status(ToolStatus::Running)).is_streaming()
        );
    }

    #[test]
    fn progress_is_progress() {
        assert!(Cell::Progress(ProgressInfo::new("x")).is_progress());
        assert!(!Cell::User(UserMessage::new("x")).is_progress());
    }

    #[test]
    fn from_chat_message_covers_all_variants() {
        use crate::app::ChatMessage;
        let cases = [
            ChatMessage::User { text: "hi".into() },
            ChatMessage::Assistant {
                text: "hello".into(),
            },
            ChatMessage::ToolUse {
                name: "Bash".into(),
                summary: Some("ls".into()),
            },
            ChatMessage::ToolResult {
                tool_name: "Bash".into(),
                output: "ok".into(),
                is_error: false,
                display: None,
            },
            ChatMessage::System {
                text: "note".into(),
            },
        ];
        for msg in &cases {
            let cell = Cell::from_chat_message(msg);
            assert!(!cell.render_lines(80).is_empty());
        }
    }

    #[test]
    fn from_conversions() {
        let _: Cell = UserMessage::new("x").into();
        let _: Cell = AssistantMessage::new("x").into();
        let _: Cell = ToolCallState::new("B").into();
        let _: Cell = ThinkingBlock::new("x").into();
        let _: Cell = DiffBlock::new("f").into();
        let _: Cell = ProgressInfo::new("x").into();
        let _: Cell = ErrorInfo::new("x").into();
        let _: Cell = PlanApproval::new("x").into();
        let _: Cell = ToolRejected::new("x").into();
    }
}
