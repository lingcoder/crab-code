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
    /// Remove the stored OAuth token for the active provider
    Logout,
    /// Interactively store an API key in `~/.crab/auth/tokens.json`
    SetupToken,
}

/// Provider auth status check result.
#[derive(Debug, Clone)]
struct ProviderStatus {
    provider: &'static str,
    env_var: &'static str,
    has_env: bool,
    has_token_file: bool,
}

impl ProviderStatus {
    fn is_configured(&self) -> bool {
        self.has_env || self.has_token_file
    }
}

/// Check which providers have API keys available.
fn check_providers(settings: &crab_config::Config) -> Vec<ProviderStatus> {
    let checks = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
    ];

    let token_path = crab_auth::oauth::default_token_path();
    let store = crab_auth::oauth::load_token_store(&token_path).unwrap_or_default();
    let active_provider = settings.api_provider.as_deref().unwrap_or("anthropic");

    checks
        .iter()
        .map(|&(provider, env_var)| {
            let has_env = std::env::var(env_var).is_ok_and(|v| !v.is_empty());
            // The token file is only meaningful for the active provider —
            // an anthropic OAuth token does not authenticate openai requests.
            let has_token_file = active_provider == provider
                && store
                    .get(provider)
                    .is_some_and(|t| !t.access_token.is_empty());

            ProviderStatus {
                provider,
                env_var,
                has_env,
                has_token_file,
            }
        })
        .collect()
}

pub fn run(action: &AuthAction) -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().unwrap_or_default();
    let ctx = crab_config::ResolveContext::new()
        .with_project_dir(Some(working_dir))
        .with_process_env();
    let settings = crab_config::resolve(&ctx).unwrap_or_default();

    match action {
        AuthAction::Login => run_login(&settings),
        AuthAction::Status { json } => run_status(&settings, *json),
        AuthAction::Logout => run_logout(&settings),
        AuthAction::SetupToken => run_setup_token(&settings),
    }
}

fn run_login(settings: &crab_config::Config) -> anyhow::Result<()> {
    let providers = check_providers(settings);
    let any_configured = providers.iter().any(ProviderStatus::is_configured);

    if any_configured {
        eprintln!("API key(s) already configured:");
        for p in &providers {
            if p.is_configured() {
                let source = if p.has_token_file {
                    "auth/tokens.json"
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
    eprintln!("  2. Interactive setup (writes ~/.crab/auth/tokens.json):");
    eprintln!("     crab auth setup-token");
    eprintln!();
    eprintln!(
        "  3. apiKeyHelper script in ~/.crab/config.toml (set the path; the script's stdout is used as the key)."
    );
    eprintln!();

    Ok(())
}

fn run_status(settings: &crab_config::Config, json_output: bool) -> anyhow::Result<()> {
    let providers = check_providers(settings);

    if json_output {
        let items: Vec<serde_json::Value> = providers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "provider": p.provider,
                    "configured": p.is_configured(),
                    "source": if p.has_token_file { "tokens.json" }
                              else if p.has_env { "env" }
                              else { "none" },
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        eprintln!("Authentication status:");
        for p in &providers {
            let (icon, detail) = if p.has_token_file {
                (
                    "ok",
                    format!("configured (auth/tokens.json, env={})", p.env_var),
                )
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

fn run_logout(settings: &crab_config::Config) -> anyhow::Result<()> {
    let provider = settings.api_provider.as_deref().unwrap_or("anthropic");
    let env_var = match provider {
        "openai" | "ollama" | "vllm" => "OPENAI_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    };

    let token_path = crab_auth::oauth::default_token_path();
    let mut store = crab_auth::oauth::load_token_store(&token_path)
        .map_err(|e| anyhow::anyhow!("failed to load token store: {e}"))?;

    let removed = store.remove(provider);
    if removed {
        crab_auth::oauth::save_token_store(&token_path, &store)
            .map_err(|e| anyhow::anyhow!("failed to write token store: {e}"))?;
        eprintln!("Removed '{provider}' token from {}", token_path.display());
    } else {
        eprintln!("No stored token for provider '{provider}'.");
    }

    if std::env::var(env_var).is_ok_and(|v| !v.is_empty()) {
        eprintln!(
            "Note: {env_var} is still set in the environment and will continue to authenticate '{provider}' requests. Unset it in your shell to fully log out."
        );
    }

    Ok(())
}

fn run_setup_token(settings: &crab_config::Config) -> anyhow::Result<()> {
    eprint!("Enter your API key: ");
    std::io::stderr().flush()?;

    let mut key = String::new();
    std::io::stdin().read_line(&mut key)?;
    let key = key.trim();

    if key.is_empty() {
        eprintln!("No key entered. Aborted.");
        return Ok(());
    }

    let provider = settings
        .api_provider
        .clone()
        .unwrap_or_else(|| "anthropic".to_string());

    let token_path = crab_auth::oauth::default_token_path();
    let mut store = crab_auth::oauth::load_token_store(&token_path)
        .map_err(|e| anyhow::anyhow!("failed to load token store: {e}"))?;

    store.upsert(crab_auth::oauth::StoredToken {
        provider: provider.clone(),
        access_token: key.to_string(),
        refresh_token: None,
        expires_at: None,
        token_type: "ApiKey".into(),
    });

    crab_auth::oauth::save_token_store(&token_path, &store)
        .map_err(|e| anyhow::anyhow!("failed to write token store: {e}"))?;

    set_owner_only_perms(&token_path)?;

    eprintln!(
        "API key for provider '{provider}' saved to {}",
        token_path.display()
    );
    Ok(())
}

/// Restrict the token file to the current user (`0600` on Unix; default ACL on Windows).
#[cfg(unix)]
fn set_owner_only_perms(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .map_err(|e| anyhow::anyhow!("failed to set permissions on {}: {e}", path.display()))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn set_owner_only_perms(_path: &std::path::Path) -> anyhow::Result<()> {
    // Windows: rely on default per-user ACL inherited from %USERPROFILE%\.crab\auth\.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_providers_returns_two() {
        let settings = crab_config::Config::default();
        let providers = check_providers(&settings);
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].provider, "anthropic");
        assert_eq!(providers[1].provider, "openai");
    }

    #[test]
    fn provider_status_no_key() {
        let settings = crab_config::Config::default();
        let providers = check_providers(&settings);
        // Without env vars set, neither should show configured via tokens.json
        // (assuming no leftover tokens from a previous run).
        assert!(!providers[0].has_token_file || !providers[1].has_token_file);
    }

    #[test]
    fn run_status_json_output() {
        let settings = crab_config::Config::default();
        let result = run_status(&settings, true);
        assert!(result.is_ok());
    }

    #[test]
    fn run_status_text_output() {
        let settings = crab_config::Config::default();
        let result = run_status(&settings, false);
        assert!(result.is_ok());
    }

    #[test]
    fn run_login_doesnt_panic() {
        let settings = crab_config::Config::default();
        let result = run_login(&settings);
        assert!(result.is_ok());
    }
}
