pub mod agents_md;
pub mod change_detector;
pub mod config;
pub mod feature_flag;
pub mod gitignore;
pub mod global_state;
pub mod hooks;
pub mod hot_reload;
pub mod keybinding;
pub mod loader;
pub mod mdm;
pub mod merge;
pub mod migration;
pub mod permissions;
pub mod plugin_loader;
pub mod policy;
pub mod runtime;
pub mod settings_cache;
pub mod validation;
pub mod writer;

pub use config::{Config, ConfigSource, EnabledPluginValue, GitContextConfig, PermissionsConfig};
pub use feature_flag::FeatureFlags;
pub use global_state::GlobalState;
pub use hot_reload::ConfigWatcher;
pub use keybinding::{KeybindingAction, KeybindingContext, KeybindingResolver};
pub use loader::{CliFlags, ResolveContext, overlay_config, resolve};
pub use merge::{dedup_preserving_order, merge_toml_values};
pub use migration::migrate_settings;
pub use permissions::PermissionRuleSet;
pub use settings_cache::SettingsCache;
pub use validation::{
    ValidationError, validate_all_config_files, validate_config, validate_raw_file,
};
pub use writer::{WriteTarget, set_value};
