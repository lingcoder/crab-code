use std::io::Write;

use clap::Subcommand;

/// Auth management subcommands.
#[derive(Subcommand)]
pub enum AuthAction {
    /// Show how to configure API keys
    Login,
    /// Show current authentication status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove API key from settings file
    Logout,
    /// Interactively set an API key and save to ~/.crab/settings.json
    SetupToken,
}

/// Provider auth status check result.
#[derive(Debug, Clone)]
struct ProviderStatus {
    provider: &'static str,
    env_var: &'static str,
    has_env: bool,
    has_settings: bool,
}

impl ProviderStatus {
    fn is_configured(&self) -> bool {
        self.has_env || self.has_settings
    }
}

/// Check which providers have API keys available.
fn check_providers(settings: &crab_config::Settings) -> Vec<ProviderStatus> {
    let checks = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
    ];

    let settings_key = settings.api_key.as_deref().unwrap_or("");
    let settings_provider = settings.api_provider.as_deref().unwrap_or("anthropic");

    checks
        .iter()
        .map(|&(provider, env_var)| {
            let has_env = std::env::var(env_var)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            let has_settings = !settings_key.is_empty() && settings_provider == provider;

            ProviderStatus {
                provider,
                env_var,
                has_env,
                has_settings,
            }
        })
        .collect()
}

pub fn run(action: &AuthAction) -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().unwrap_or_default();
    let settings =
        crab_config::settings::load_merged_settings(Some(&working_dir)).unwrap_or_default();

    match action {
        AuthAction::Login => run_login(&settings),
        AuthAction::Status { json } => run_status(&settings, *json),
        AuthAction::Logout => run_logout(),
        AuthAction::SetupToken => run_setup_token(),
    }
}

fn run_login(settings: &crab_config::Settings) -> anyhow::Result<()> {
    let providers = check_providers(settings);
    let any_configured = providers.iter().any(ProviderStatus::is_configured);

    if any_configured {
        eprintln!("API key(s) already configured:");
        for p in &providers {
            if p.is_configured() {
                let source = if p.has_settings {
                    "settings"
                } else {
                    "env var"
                };
                eprintln!("  {} — configured via {}", p.provider, source);
            }
        }
        eprintln!();
    }

    eprintln!("To configure an API key, use one of these methods:");
    eprintln!();
    eprintln!("  1. Environment variable:");
    eprintln!("     export ANTHROPIC_API_KEY=sk-ant-...");
    eprintln!("     export OPENAI_API_KEY=sk-...");
    eprintln!();
    eprintln!("  2. Settings file (~/.crab/settings.json):");
    eprintln!("     crab config set apiKey sk-ant-...");
    eprintln!();
    eprintln!("  3. Interactive setup:");
    eprintln!("     crab auth setup-token");
    eprintln!();

    Ok(())
}

fn run_status(settings: &crab_config::Settings, json_output: bool) -> anyhow::Result<()> {
    let providers = check_providers(settings);

    if json_output {
        let items: Vec<serde_json::Value> = providers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "provider": p.provider,
                    "configured": p.is_configured(),
                    "source": if p.has_settings { "settings" }
                              else if p.has_env { "env" }
                              else { "none" },
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        eprintln!("Authentication status:");
        for p in &providers {
            let (icon, detail) = if p.has_settings {
                ("ok", format!("configured (settings, env={})", p.env_var))
            } else if p.has_env {
                ("ok", format!("configured (env={})", p.env_var))
            } else {
                ("--", format!("not configured (set {})", p.env_var))
            };
            eprintln!("  [{}] {} — {}", icon, p.provider, detail);
        }
    }

    Ok(())
}

fn run_logout() -> anyhow::Result<()> {
    let global_dir = crab_config::settings::global_config_dir();
    let settings_path = global_dir.join("settings.json");

    if !settings_path.exists() {
        eprintln!("No settings file found. Nothing to do.");
        return Ok(());
    }

    let content = std::fs::read_to_string(&settings_path)?;
    let mut value: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(obj) = value.as_object_mut() {
        if obj.remove("apiKey").is_some() {
            let updated = serde_json::to_string_pretty(&value)?;
            std::fs::write(&settings_path, updated)?;
            eprintln!("API key removed from {}", settings_path.display());
        } else {
            eprintln!("No API key found in settings file.");
        }
    }

    Ok(())
}

fn run_setup_token() -> anyhow::Result<()> {
    eprint!("Enter your API key: ");
    std::io::stderr().flush()?;

    let mut key = String::new();
    std::io::stdin().read_line(&mut key)?;
    let key = key.trim();

    if key.is_empty() {
        eprintln!("No key entered. Aborted.");
        return Ok(());
    }

    let global_dir = crab_config::settings::global_config_dir();
    std::fs::create_dir_all(&global_dir)?;
    let settings_path = global_dir.join("settings.json");

    let mut value: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = value.as_object_mut() {
        obj.insert("apiKey".into(), serde_json::Value::String(key.to_string()));
    }

    let updated = serde_json::to_string_pretty(&value)?;
    std::fs::write(&settings_path, updated)?;
    eprintln!("API key saved to {}", settings_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_providers_returns_two() {
        let settings = crab_config::Settings::default();
        let providers = check_providers(&settings);
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].provider, "anthropic");
        assert_eq!(providers[1].provider, "openai");
    }

    #[test]
    fn provider_status_with_settings_key() {
        let settings = crab_config::Settings {
            api_key: Some("sk-test".into()),
            api_provider: Some("anthropic".into()),
            ..Default::default()
        };
        let providers = check_providers(&settings);
        let anthropic = &providers[0];
        assert!(anthropic.has_settings);
        assert!(anthropic.is_configured());
    }

    #[test]
    fn provider_status_no_key() {
        let settings = crab_config::Settings::default();
        let providers = check_providers(&settings);
        // Without env vars set, both should show not configured via settings
        assert!(!providers[0].has_settings);
        assert!(!providers[1].has_settings);
    }

    #[test]
    fn run_status_json_output() {
        let settings = crab_config::Settings {
            api_key: Some("sk-test".into()),
            api_provider: Some("anthropic".into()),
            ..Default::default()
        };
        // Just verify it doesn't panic — actual output goes to stdout
        let result = run_status(&settings, true);
        assert!(result.is_ok());
    }

    #[test]
    fn run_status_text_output() {
        let settings = crab_config::Settings::default();
        let result = run_status(&settings, false);
        assert!(result.is_ok());
    }

    #[test]
    fn run_login_doesnt_panic() {
        let settings = crab_config::Settings::default();
        let result = run_login(&settings);
        assert!(result.is_ok());
    }
}
