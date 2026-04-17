//! Permission system — the gate between LLM-requested tool invocations
//! and their execution.
//!
//! ## Sub-modules
//!
//! - [`mode`]      — [`PermissionMode`] (the 6 agent-wide behaviour modes)
//! - [`policy`]    — [`PermissionPolicy`] (user-configured allow/deny lists)
//! - [`decision`]  — [`PermissionDecision`] (`Allow` / `Deny` / `AskUser`)
//! - [`filter`]    — `matches_tool_filter` + glob engine shared by policy matchers
//! - [`auto_mode`] — [`RiskLevel`] + [`AutoModeClassifier`] + [`auto_mode_decision`]
//! - [`denial_tracker`] — history of denied invocations (for UI)
//! - [`explainer`] — human-readable explanation of a decision
//! - [`path_validator`] — filesystem-path allow/deny
//! - [`rule_parser`] — the textual rule grammar
//! - [`shadowed_rules`] — detect rules that shadow earlier ones

pub mod auto_mode;
pub mod decision;
pub mod denial_tracker;
pub mod explainer;
pub mod filter;
pub mod mode;
pub mod path_validator;
pub mod policy;
pub mod rule_parser;
pub mod shadowed_rules;

pub use auto_mode::{AutoModeClassifier, RiskLevel, auto_mode_decision};
pub use decision::PermissionDecision;
pub use denial_tracker::{DenialRecord, DenialTracker};
pub use explainer::{PermissionExplanation, explain_decision};
pub use filter::matches_tool_filter;
pub use mode::PermissionMode;
pub use path_validator::{PathError, PathPermission, PathValidator};
pub use policy::PermissionPolicy;
pub use rule_parser::{
    BashPattern, ParseError, PermissionRule, RuleContent, matches_rule, parse_bash_pattern,
    parse_rule,
};
pub use shadowed_rules::{ShadowedRule, detect_shadowed_rules};
