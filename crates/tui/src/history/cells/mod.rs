//! Built-in `HistoryCell` implementations.

pub mod assistant;
pub mod collapsed_read_search;
pub mod compact_boundary;
pub mod plan_step;
pub mod system;
pub mod thinking;
pub mod tool_call;
pub mod tool_rejected;
pub mod tool_result;
pub mod user;

pub use assistant::AssistantCell;
pub use collapsed_read_search::CollapsedReadSearchCell;
pub use compact_boundary::CompactBoundaryCell;
pub use plan_step::PlanStepCell;
pub use system::SystemCell;
pub use thinking::ThinkingCell;
pub use tool_call::ToolCallCell;
pub use tool_rejected::ToolRejectedCell;
pub use tool_result::ToolResultCell;
pub use user::UserCell;
