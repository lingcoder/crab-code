pub mod config;
pub mod gitignore;
pub mod hooks;
pub mod loader;
pub mod merge;
pub mod migration;
pub mod plugin_loader;
pub mod runtime;
pub mod validation;
pub mod writer;

pub use config::{
    Config, ConfigLayer, DefaultShell, EnabledPluginValue, GitContextConfig, PermissionsConfig,
};
pub use crab_core::permission::stored::{
    AuditEntry, AuditSource, PermissionRuleSet, PermissionStore, RuleScope, RuleVerdict,
    StoredPermissionRule, load_permission_store, save_permission_store,
};
pub use loader::{CliFlags, ResolveContext, overlay_config, resolve};
pub use merge::{dedup_preserving_order, merge_toml_values};
pub use migration::migrate_settings;
pub use validation::{
    ValidationError, validate_all_config_files, validate_config, validate_raw_file,
};
pub use writer::{WriteTarget, set_value};
