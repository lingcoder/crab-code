//! Default interactive chat mode.
//!
//! Orchestrates TUI + agent session + query loop for the main `crab` command.
//! This module provides the high-level entry point that wires together:
//! - Configuration loading and merging (settings, env, CLI overrides)
//! - LLM backend creation (Anthropic / `OpenAI` / Ollama / etc.)
//! - Tool registry setup (built-in tools + MCP adapters)
//! - TUI terminal UI (when the `tui` feature is enabled)
//! - Fallback line-based REPL (when TUI is unavailable)

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use crab_agent::SessionConfig;
use crab_config::settings;
use crab_core::model::ModelId;
use crab_core::permission::{PermissionMode, PermissionPolicy};

/// Configuration for the interactive chat session.
///
/// Constructed from CLI args and settings files in `main.rs`, then passed
/// to [`run_chat`] to start the interactive loop.
pub struct ChatConfig {
    /// Project root directory (usually the cwd).
    pub project_dir: PathBuf,
    /// Model ID override (e.g. "claude-sonnet-4-6").
    pub model: Option<String>,
    /// Provider name (e.g. "anthropic", "openai").
    pub provider: String,
    /// Maximum output tokens per response.
    pub max_tokens: u32,
    /// Permission mode override.
    pub permission_mode: Option<String>,
    /// Resume a previous session by ID.
    pub resume_session: Option<String>,
    /// Custom system prompt or instructions from settings.
    pub custom_instructions: Option<String>,
    /// Enable verbose/debug logging.
    pub verbose: bool,
    /// Session display name.
    pub session_name: Option<String>,
    /// Additional directories the agent may access.
    pub additional_dirs: Vec<PathBuf>,
    /// Skill directories to scan for /command support.
    pub skill_dirs: Vec<PathBuf>,
    /// Whether to disable session persistence.
    pub no_session_persistence: bool,
    /// Bare mode: skip hooks, plugins, memory, AGENTS.md discovery.
    pub bare_mode: bool,
    /// Fallback model when primary is overloaded.
    pub fallback_model: Option<String>,
    /// Effort level: "low", "medium", "high", "max".
    pub effort: Option<String>,
    /// Thinking mode: "enabled", "adaptive", "disabled".
    pub thinking_mode: Option<String>,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            project_dir: PathBuf::from("."),
            model: None,
            provider: "anthropic".to_string(),
            max_tokens: 4096,
            permission_mode: None,
            resume_session: None,
            custom_instructions: None,
            verbose: false,
            session_name: None,
            additional_dirs: Vec::new(),
            skill_dirs: Vec::new(),
            no_session_persistence: false,
            bare_mode: false,
            fallback_model: None,
            effort: None,
            thinking_mode: None,
        }
    }
}

/// Run the interactive chat session.
///
/// This is the main orchestration function for interactive mode. It:
/// 1. Loads and merges configuration (global → project → env → CLI)
/// 2. Creates the LLM backend from the effective settings
/// 3. Builds the tool registry with all built-in tools
/// 4. Constructs a `SessionConfig` with permissions, memory, sessions
/// 5. Launches the TUI (or fallback REPL) and runs until the user exits
///
/// # Errors
///
/// Returns an error if configuration loading, backend creation, or the
/// TUI/REPL loop fails.
pub async fn run_chat(config: ChatConfig) -> anyhow::Result<()> {
    // 1. Load merged settings
    let merged_settings = settings::load_merged_settings(Some(&config.project_dir))
        .context("failed to load settings")?;

    // 2. Resolve effective model and provider
    let provider = if config.provider == "anthropic" {
        merged_settings
            .api_provider
            .clone()
            .unwrap_or_else(|| config.provider.clone())
    } else {
        config.provider.clone()
    };

    let model_id = config
        .model
        .clone()
        .or_else(|| merged_settings.model.clone())
        .unwrap_or_else(|| {
            if provider == "openai" {
                "gpt-4o".to_string()
            } else {
                "claude-sonnet-4-6".to_string()
            }
        });

    // 3. Build effective settings for backend creation
    let effective_settings = crab_config::Settings {
        api_provider: Some(provider.clone()),
        api_base_url: merged_settings.api_base_url.clone(),
        api_key: merged_settings.api_key.clone(),
        model: Some(model_id.clone()),
        ..merged_settings.clone()
    };

    let backend = Arc::new(crab_api::create_backend(&effective_settings));

    // 4. Resolve permission mode
    let permission_mode = if let Some(ref mode_str) = config.permission_mode {
        mode_str
            .parse::<PermissionMode>()
            .map_err(|e| anyhow::anyhow!(e))?
    } else {
        merged_settings
            .permission_mode
            .as_deref()
            .and_then(|s| s.parse::<PermissionMode>().ok())
            .unwrap_or(PermissionMode::Default)
    };

    // 5. Set up directories
    let global_dir = settings::global_config_dir();
    let sessions_dir = global_dir.join("sessions");

    let effective_sessions_dir = if config.no_session_persistence || config.bare_mode {
        None
    } else {
        Some(sessions_dir.clone())
    };

    let effective_memory_dir = if config.bare_mode {
        None
    } else {
        Some(global_dir.join("memory"))
    };

    // 6. Resolve resume ID
    let resume_id = if config.resume_session.is_some() {
        config.resume_session.clone()
    } else {
        None
    };

    let session_id = crab_core::common::utils::id::new_ulid();

    // 7. Build system prompt
    let registry = crab_tools::builtin::create_default_registry();
    let system_prompt = crab_agent::build_system_prompt(
        &config.project_dir,
        &registry,
        effective_settings.system_prompt.as_deref(),
    );

    // 8. Build session config
    let coordinator_mode = crate::coordinator_mode_enabled();
    let session_config = SessionConfig {
        session_id,
        system_prompt,
        model: ModelId::from(model_id.as_str()),
        max_tokens: config.max_tokens,
        temperature: None,
        context_window: 200_000,
        working_dir: config.project_dir.clone(),
        permission_policy: PermissionPolicy {
            mode: permission_mode,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
        },
        memory_dir: effective_memory_dir,
        sessions_dir: effective_sessions_dir,
        resume_session_id: resume_id,
        effort: config.effort,
        thinking_mode: config.thinking_mode,
        additional_dirs: config.additional_dirs,
        session_name: config.session_name,
        max_turns: None,
        max_budget_usd: None,
        fallback_model: config.fallback_model,
        bare_mode: config.bare_mode,
        worktree_name: None,
        fork_session: false,
        from_pr: None,
        custom_session_id: None,
        json_schema: None,
        plugin_dirs: Vec::new(),
        disable_skills: false,
        beta_headers: Vec::new(),
        ide_connect: false,
        coordinator_mode,
    };

    // 9. Launch TUI or fallback REPL
    #[cfg(feature = "tui")]
    {
        let tui_config = crab_tui::TuiConfig {
            session_config,
            backend,
            skill_dirs: config.skill_dirs,
            mcp_servers: merged_settings.mcp_servers.clone(),
            settings_warnings: Vec::new(),
        };
        let exit_info = crab_tui::run(tui_config).await?;
        crate::print_exit_info(&exit_info);
        Ok(())
    }

    #[cfg(not(feature = "tui"))]
    {
        use crab_agent::AgentSession;
        use std::io::{BufRead, Write};

        let mut session = AgentSession::new(session_config, backend, registry);
        // In non-TUI mode, use a simple stdin-based permission handler.
        // The actual handler is defined in main.rs — here we just start the REPL.
        eprintln!("Interactive mode (no TUI). Type /exit or Ctrl+D to quit.\n");
        eprintln!("Provider: {provider}, Model: {model_id}, Permissions: {permission_mode}");

        // Minimal REPL loop
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();

        loop {
            print!("crab> ");
            stdout.flush()?;

            let mut line = String::new();
            let bytes = stdin.lock().read_line(&mut line)?;
            if bytes == 0 {
                eprintln!("\nGoodbye!");
                break;
            }

            let input = line.trim();
            if input.is_empty() {
                continue;
            }
            if input == "/exit" || input == "/quit" {
                eprintln!("Goodbye!");
                break;
            }

            match session.handle_user_input(input).await {
                Ok(()) => {}
                Err(e) => eprintln!("[error] {e}"),
            }
            println!();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_config_default() {
        let config = ChatConfig::default();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.max_tokens, 4096);
        assert!(!config.verbose);
        assert!(!config.bare_mode);
        assert!(config.model.is_none());
        assert!(config.resume_session.is_none());
    }

    #[test]
    fn chat_config_with_overrides() {
        let config = ChatConfig {
            project_dir: PathBuf::from("/my/project"),
            model: Some("claude-opus-4-6".into()),
            provider: "openai".into(),
            max_tokens: 8192,
            permission_mode: Some("dangerously".into()),
            verbose: true,
            bare_mode: true,
            ..Default::default()
        };
        assert_eq!(config.project_dir, PathBuf::from("/my/project"));
        assert_eq!(config.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(config.provider, "openai");
        assert_eq!(config.max_tokens, 8192);
        assert!(config.verbose);
        assert!(config.bare_mode);
    }
}
