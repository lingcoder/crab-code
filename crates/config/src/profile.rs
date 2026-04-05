//! Environment profile system for multi-environment configuration.
//!
//! Supports switching between `dev`, `staging`, `production`, and custom
//! profiles. Each profile has its own settings overlay stored under
//! `~/.crab/profiles/{name}/settings.json`.
//!
//! Merge chain: base settings → profile overrides.

use std::path::{Path, PathBuf};

use crate::settings::{self, Settings};

/// Built-in environment profiles.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Development environment — relaxed settings, verbose logging.
    Dev,
    /// Staging environment — mirrors production with test endpoints.
    Staging,
    /// Production environment — strict settings, minimal logging.
    Production,
    /// User-defined profile with an arbitrary name.
    Custom(String),
}

impl Profile {
    /// The directory name for this profile under `~/.crab/profiles/`.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Dev => "dev",
            Self::Staging => "staging",
            Self::Production => "production",
            Self::Custom(name) => name,
        }
    }

    /// Parse a profile name string into a `Profile`.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "dev" | "development" => Self::Dev,
            "staging" | "stage" => Self::Staging,
            "prod" | "production" => Self::Production,
            other => Self::Custom(other.to_string()),
        }
    }

    /// Whether this is a built-in profile.
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Dev | Self::Staging | Self::Production)
    }
}

impl std::fmt::Display for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Manages profile resolution and settings loading.
pub struct ProfileManager {
    /// The active profile (if any).
    active_profile: Option<Profile>,
    /// Root config directory (`~/.crab/`).
    config_dir: PathBuf,
}

impl ProfileManager {
    /// Create a new `ProfileManager` with the given config directory.
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            active_profile: None,
            config_dir,
        }
    }

    /// Create with the default global config directory (`~/.crab/`).
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(settings::global_config_dir())
    }

    /// Resolve the active profile from (in priority order):
    /// 1. Explicit CLI argument (`--profile`)
    /// 2. `CRAB_PROFILE` environment variable
    /// 3. No profile (returns `None`)
    #[must_use]
    pub fn resolve(mut self, cli_profile: Option<&str>) -> Self {
        self.active_profile = resolve_profile(cli_profile, |k| std::env::var(k));
        self
    }

    /// Resolve with an injectable env lookup (for testing).
    #[must_use]
    pub fn resolve_with_env<F>(mut self, cli_profile: Option<&str>, env_lookup: F) -> Self
    where
        F: Fn(&str) -> Result<String, std::env::VarError>,
    {
        self.active_profile = resolve_profile(cli_profile, env_lookup);
        self
    }

    /// The currently active profile, if any.
    #[must_use]
    pub fn active_profile(&self) -> Option<&Profile> {
        self.active_profile.as_ref()
    }

    /// Directory for a specific profile: `{config_dir}/profiles/{name}/`.
    #[must_use]
    pub fn profile_dir(&self, profile: &Profile) -> PathBuf {
        self.config_dir.join("profiles").join(profile.name())
    }

    /// Settings file path for a specific profile.
    #[must_use]
    pub fn profile_settings_path(&self, profile: &Profile) -> PathBuf {
        self.profile_dir(profile).join("settings.json")
    }

    /// Load profile-specific settings overlay.
    ///
    /// Returns `Settings::default()` if no profile is active or the
    /// profile settings file doesn't exist.
    pub fn load_profile_settings(&self) -> crab_common::Result<Settings> {
        let Some(ref profile) = self.active_profile else {
            return Ok(Settings::default());
        };
        load_profile_settings_from_path(&self.profile_settings_path(profile))
    }

    /// Load base settings and merge with profile overrides.
    ///
    /// Merge chain: `load_merged_settings()` → profile overlay.
    pub fn load_with_profile(
        &self,
        project_dir: Option<&PathBuf>,
    ) -> crab_common::Result<Settings> {
        let base = settings::load_merged_settings(project_dir)?;
        let profile_overlay = self.load_profile_settings()?;
        Ok(base.merge(&profile_overlay))
    }

    /// List all available profiles (directories under `{config_dir}/profiles/`).
    #[must_use]
    pub fn list_profiles(&self) -> Vec<Profile> {
        let profiles_dir = self.config_dir.join("profiles");
        let Ok(entries) = std::fs::read_dir(&profiles_dir) else {
            return Vec::new();
        };

        let mut profiles = Vec::new();
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir())
                && let Some(name) = entry.file_name().to_str()
            {
                profiles.push(Profile::from_name(name));
            }
        }
        profiles.sort_by(|a, b| a.name().cmp(b.name()));
        profiles
    }

    /// Create a profile directory and an empty settings file.
    pub fn create_profile(&self, profile: &Profile) -> crab_common::Result<PathBuf> {
        let dir = self.profile_dir(profile);
        std::fs::create_dir_all(&dir)
            .map_err(|e| crab_common::Error::Config(format!("creating profile dir: {e}")))?;

        let settings_path = dir.join("settings.json");
        if !settings_path.exists() {
            std::fs::write(&settings_path, "{}\n").map_err(|e| {
                crab_common::Error::Config(format!("creating profile settings: {e}"))
            })?;
        }

        Ok(dir)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Resolve profile from CLI arg or env var.
fn resolve_profile<F>(cli_profile: Option<&str>, env_lookup: F) -> Option<Profile>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    // CLI argument takes priority
    if let Some(name) = cli_profile
        && !name.is_empty()
    {
        return Some(Profile::from_name(name));
    }

    // Fall back to environment variable
    if let Ok(name) = env_lookup("CRAB_PROFILE")
        && !name.is_empty()
    {
        return Some(Profile::from_name(&name));
    }

    None
}

/// Load settings from a profile-specific path.
fn load_profile_settings_from_path(path: &Path) -> crab_common::Result<Settings> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed = jsonc_parser::parse_to_serde_value::<serde_json::Value>(
                &content,
                &jsonc_parser::ParseOptions::default(),
            )
            .map_err(|e| crab_common::Error::Config(format!("profile JSONC parse error: {e}")))?;
            serde_json::from_value(parsed)
                .map_err(|e| crab_common::Error::Config(format!("profile settings error: {e}")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(crab_common::Error::Config(format!(
            "reading profile settings {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(
        map: HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Result<String, std::env::VarError> {
        move |key: &str| {
            map.get(key)
                .map(|v| (*v).to_string())
                .ok_or(std::env::VarError::NotPresent)
        }
    }

    fn no_env(_key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }

    // ── Profile enum ───────────────────────────────────────────────────

    #[test]
    fn profile_from_name_dev() {
        assert_eq!(Profile::from_name("dev"), Profile::Dev);
        assert_eq!(Profile::from_name("development"), Profile::Dev);
        assert_eq!(Profile::from_name("DEV"), Profile::Dev);
    }

    #[test]
    fn profile_from_name_staging() {
        assert_eq!(Profile::from_name("staging"), Profile::Staging);
        assert_eq!(Profile::from_name("stage"), Profile::Staging);
    }

    #[test]
    fn profile_from_name_production() {
        assert_eq!(Profile::from_name("prod"), Profile::Production);
        assert_eq!(Profile::from_name("production"), Profile::Production);
    }

    #[test]
    fn profile_from_name_custom() {
        let p = Profile::from_name("my-team");
        assert_eq!(p, Profile::Custom("my-team".into()));
        assert_eq!(p.name(), "my-team");
    }

    #[test]
    fn profile_name_builtin() {
        assert_eq!(Profile::Dev.name(), "dev");
        assert_eq!(Profile::Staging.name(), "staging");
        assert_eq!(Profile::Production.name(), "production");
    }

    #[test]
    fn profile_is_builtin() {
        assert!(Profile::Dev.is_builtin());
        assert!(Profile::Staging.is_builtin());
        assert!(Profile::Production.is_builtin());
        assert!(!Profile::Custom("x".into()).is_builtin());
    }

    #[test]
    fn profile_display() {
        assert_eq!(format!("{}", Profile::Dev), "dev");
        assert_eq!(format!("{}", Profile::Custom("test".into())), "test");
    }

    #[test]
    fn profile_eq() {
        assert_eq!(Profile::Dev, Profile::Dev);
        assert_ne!(Profile::Dev, Profile::Staging);
        assert_eq!(Profile::Custom("a".into()), Profile::Custom("a".into()));
        assert_ne!(Profile::Custom("a".into()), Profile::Custom("b".into()));
    }

    // ── resolve_profile ────────────────────────────────────────────────

    #[test]
    fn resolve_cli_takes_priority() {
        let env = fake_env(HashMap::from([("CRAB_PROFILE", "staging")]));
        let result = resolve_profile(Some("dev"), env);
        assert_eq!(result, Some(Profile::Dev));
    }

    #[test]
    fn resolve_env_fallback() {
        let env = fake_env(HashMap::from([("CRAB_PROFILE", "production")]));
        let result = resolve_profile(None, env);
        assert_eq!(result, Some(Profile::Production));
    }

    #[test]
    fn resolve_none_when_nothing_set() {
        let result = resolve_profile(None, no_env);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_empty_cli_falls_through() {
        let env = fake_env(HashMap::from([("CRAB_PROFILE", "dev")]));
        let result = resolve_profile(Some(""), env);
        assert_eq!(result, Some(Profile::Dev));
    }

    #[test]
    fn resolve_empty_env_returns_none() {
        let env = fake_env(HashMap::from([("CRAB_PROFILE", "")]));
        let result = resolve_profile(None, env);
        assert!(result.is_none());
    }

    // ── ProfileManager ─────────────────────────────────────────────────

    #[test]
    fn manager_no_profile_by_default() {
        let mgr = ProfileManager::new(PathBuf::from("/tmp/crab"));
        assert!(mgr.active_profile().is_none());
    }

    #[test]
    fn manager_resolve_with_cli() {
        let mgr = ProfileManager::new(PathBuf::from("/tmp/crab"))
            .resolve_with_env(Some("staging"), no_env);
        assert_eq!(mgr.active_profile(), Some(&Profile::Staging));
    }

    #[test]
    fn manager_resolve_with_env() {
        let env = fake_env(HashMap::from([("CRAB_PROFILE", "prod")]));
        let mgr = ProfileManager::new(PathBuf::from("/tmp/crab")).resolve_with_env(None, env);
        assert_eq!(mgr.active_profile(), Some(&Profile::Production));
    }

    #[test]
    fn manager_profile_dir() {
        let mgr = ProfileManager::new(PathBuf::from("/home/user/.crab"));
        let dir = mgr.profile_dir(&Profile::Dev);
        assert!(dir.ends_with("profiles/dev"));
        assert!(dir.starts_with("/home/user/.crab"));
    }

    #[test]
    fn manager_profile_settings_path() {
        let mgr = ProfileManager::new(PathBuf::from("/home/user/.crab"));
        let path = mgr.profile_settings_path(&Profile::Production);
        assert!(path.ends_with("profiles/production/settings.json"));
    }

    #[test]
    fn manager_load_no_profile_returns_default() {
        let mgr = ProfileManager::new(PathBuf::from("/nonexistent/path"));
        let settings = mgr.load_profile_settings().unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn manager_load_missing_profile_file_returns_default() {
        let mgr = ProfileManager::new(PathBuf::from("/nonexistent"))
            .resolve_with_env(Some("dev"), no_env);
        let settings = mgr.load_profile_settings().unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn manager_create_and_load_profile() {
        let dir = std::env::temp_dir().join("crab-profile-test-create");
        let _ = std::fs::remove_dir_all(&dir);

        let mgr = ProfileManager::new(dir.clone());
        let profile = Profile::Dev;

        // Create profile
        let profile_dir = mgr.create_profile(&profile).unwrap();
        assert!(profile_dir.exists());
        assert!(mgr.profile_settings_path(&profile).exists());

        // Load empty profile settings
        let mgr = mgr.resolve_with_env(Some("dev"), no_env);
        let settings = mgr.load_profile_settings().unwrap();
        assert_eq!(settings, Settings::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_create_profile_idempotent() {
        let dir = std::env::temp_dir().join("crab-profile-test-idempotent");
        let _ = std::fs::remove_dir_all(&dir);

        let mgr = ProfileManager::new(dir.clone());
        let profile = Profile::Staging;

        mgr.create_profile(&profile).unwrap();
        // Second create should not fail
        mgr.create_profile(&profile).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_load_profile_with_overrides() {
        let dir = std::env::temp_dir().join("crab-profile-test-overrides");
        let _ = std::fs::remove_dir_all(&dir);

        let mgr = ProfileManager::new(dir.clone());
        mgr.create_profile(&Profile::Dev).unwrap();

        // Write profile-specific settings
        let settings_path = mgr.profile_settings_path(&Profile::Dev);
        std::fs::write(&settings_path, r#"{"model": "dev-model", "theme": "dark"}"#).unwrap();

        let mgr = mgr.resolve_with_env(Some("dev"), no_env);
        let settings = mgr.load_profile_settings().unwrap();
        assert_eq!(settings.model.as_deref(), Some("dev-model"));
        assert_eq!(settings.theme.as_deref(), Some("dark"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_list_profiles_empty() {
        let dir = std::env::temp_dir().join("crab-profile-test-list-empty");
        let _ = std::fs::remove_dir_all(&dir);

        let mgr = ProfileManager::new(dir.clone());
        assert!(mgr.list_profiles().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_list_profiles() {
        let dir = std::env::temp_dir().join("crab-profile-test-list");
        let _ = std::fs::remove_dir_all(&dir);

        let mgr = ProfileManager::new(dir.clone());
        mgr.create_profile(&Profile::Dev).unwrap();
        mgr.create_profile(&Profile::Production).unwrap();
        mgr.create_profile(&Profile::Custom("team-a".into()))
            .unwrap();

        let profiles = mgr.list_profiles();
        let names: Vec<&str> = profiles.iter().map(|p| p.name()).collect();
        assert!(names.contains(&"dev"));
        assert!(names.contains(&"production"));
        assert!(names.contains(&"team-a"));
        assert_eq!(profiles.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_load_with_profile_merges() {
        let dir = std::env::temp_dir().join("crab-profile-test-merge");
        let _ = std::fs::remove_dir_all(&dir);

        let mgr = ProfileManager::new(dir.clone());
        mgr.create_profile(&Profile::Staging).unwrap();

        // Write staging overrides
        let settings_path = mgr.profile_settings_path(&Profile::Staging);
        std::fs::write(
            &settings_path,
            r#"{"apiProvider": "openai", "model": "gpt-4o"}"#,
        )
        .unwrap();

        let mgr = mgr.resolve_with_env(Some("staging"), no_env);
        let merged = mgr.load_with_profile(None).unwrap();

        // Profile overrides should be present
        assert_eq!(merged.api_provider.as_deref(), Some("openai"));
        assert_eq!(merged.model.as_deref(), Some("gpt-4o"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_with_defaults_uses_global_dir() {
        let mgr = ProfileManager::with_defaults();
        // Should use ~/.crab/ as the config dir
        let dev_dir = mgr.profile_dir(&Profile::Dev);
        assert!(dev_dir.to_string_lossy().contains(".crab"));
        assert!(dev_dir.to_string_lossy().contains("profiles"));
    }

    #[test]
    fn load_profile_settings_from_path_invalid_json() {
        let dir = std::env::temp_dir().join("crab-profile-test-invalid");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ broken json }").unwrap();

        let result = load_profile_settings_from_path(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_profile_settings_from_path_jsonc() {
        let dir = std::env::temp_dir().join("crab-profile-test-jsonc");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{
            // Profile comment
            "model": "test-model"
        }"#,
        )
        .unwrap();

        let settings = load_profile_settings_from_path(&path).unwrap();
        assert_eq!(settings.model.as_deref(), Some("test-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_profile_dir_name() {
        let mgr = ProfileManager::new(PathBuf::from("/config"));
        let dir = mgr.profile_dir(&Profile::Custom("my-team-env".into()));
        assert!(dir.ends_with("profiles/my-team-env"));
    }

    #[test]
    fn profile_clone() {
        let p = Profile::Custom("test".into());
        let p2 = p.clone();
        assert_eq!(p, p2);
    }

    #[test]
    fn profile_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Profile::Dev);
        set.insert(Profile::Dev);
        assert_eq!(set.len(), 1);
    }
}
