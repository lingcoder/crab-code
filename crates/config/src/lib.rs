pub mod agents_md;
pub mod change_detector;
pub mod config;
pub mod feature_flag;
pub mod global_state;
pub mod hooks;
pub mod hot_reload;
pub mod keybinding;
pub mod mdm;
pub mod merge;
pub mod migration;
pub mod permissions;
pub mod policy;
pub mod settings_cache;
pub mod validation;

pub use config::{Config, ConfigSource, GitContextConfig, PermissionsConfig};
pub use feature_flag::FeatureFlags;
pub use global_state::GlobalState;
pub use hot_reload::ConfigWatcher;
pub use keybinding::{KeybindingAction, KeybindingContext, KeybindingResolver};
pub use merge::{dedup_preserving_order, merge_toml_values};
pub use migration::migrate_settings;
pub use permissions::PermissionRuleSet;
pub use settings_cache::SettingsCache;
pub use validation::{
    ValidationError, validate_all_config_files, validate_config, validate_raw_file,
};
