pub mod auto_compact;
pub mod compaction;
pub mod context;
pub mod conversation;
pub mod cost;
pub mod history;
pub mod input_expand;
pub mod memory;
pub mod memory_extract;
pub mod micro_compact;
pub mod migration;
pub mod snip_compact;
pub mod template;

pub use auto_compact::{AutoCompactConfig, AutoCompactState, CompactTrigger, should_auto_compact};
pub use compaction::{
    CompactionClient, CompactionConfig, CompactionMode, CompactionReport, CompactionStrategy,
    CompactionTrigger, compact, compact_with_config,
};
pub use context::{ContextAction, ContextManager};
pub use conversation::Conversation;
pub use cost::{CostAccumulator, CostSummary, ModelPricing, lookup_pricing};
pub use history::{
    BoundSessionPersister, ExportFormat, SearchResult, SessionHistory, SessionMetadata,
    SessionPersister, SessionStats,
};
pub use input_expand::expand_at_mentions;
pub use memory::{IndexEntry, MemoryFile, MemoryIndex, MemoryStore};
pub use snip_compact::SnipConfig;
pub use template::{
    SessionKind, SessionSummary, SessionTemplate, builtin_templates, find_template,
    find_template_by_name, quick_resume_list,
};
