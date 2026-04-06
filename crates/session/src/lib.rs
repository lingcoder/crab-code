pub mod compaction;
pub mod context;
pub mod conversation;
pub mod cost;
pub mod history;
pub mod memory;
pub mod template;

pub use compaction::{
    CompactionClient, CompactionConfig, CompactionMode, CompactionReport, CompactionStrategy,
    CompactionTrigger, compact, compact_with_config,
};
pub use context::{ContextAction, ContextManager};
pub use conversation::Conversation;
pub use cost::{CostAccumulator, CostSummary, ModelPricing, lookup_pricing};
pub use history::{ExportFormat, SearchResult, SessionHistory, SessionMetadata, SessionStats};
pub use memory::{MemoryFile, MemoryIndexEntry, MemoryStore};
pub use template::{
    SessionKind, SessionSummary, SessionTemplate, builtin_templates, find_template,
    find_template_by_name, quick_resume_list,
};
