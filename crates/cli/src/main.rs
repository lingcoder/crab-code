mod acp_mode;
#[allow(dead_code, clippy::doc_markdown)]
mod commands;
#[allow(
    dead_code,
    clippy::let_unit_value,
    clippy::ignored_unit_patterns,
    clippy::map_unwrap_or
)]
mod setup;

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, Subcommand, ValueEnum};
use owo_colors::OwoColorize;
use serde_json::{Value, json};

use crab_agent::{AgentSession, SessionConfig, build_system_prompt};
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::PermissionHandler;
use tokio::sync::mpsc;

/// Output format for CLI results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable colored output (default).
    Text,
    /// Single JSON result at the end.
    Json,
    /// NDJSON real-time stream — one JSON object per event per line.
    StreamJson,
}

/// Crab Code -- Rust-native Agentic Coding CLI
#[derive(Parser)]
#[command(name = "crab", version, about)]
struct Cli {
    /// User prompt (if provided, runs single-shot mode then exits)
    prompt: Option<String>,

    /// LLM provider: "anthropic" (default) or "openai"
    #[arg(long, default_value = "anthropic")]
    provider: String,

    /// Model ID override (e.g. "claude-sonnet-4-6", "gpt-4o").
    /// Supports aliases: "sonnet", "opus", "haiku".
    #[arg(long, short)]
    model: Option<String>,

    /// Maximum output tokens
    #[arg(long, default_value = "4096")]
    max_tokens: u32,

    /// Trust in-project file operations (skip confirmation for project writes)
    #[arg(long, short = 't')]
    trust_project: bool,

    /// Skip ALL permission checks (dangerous!)
    #[arg(long)]
    dangerously_skip_permissions: bool,

    /// Run as an ACP (Agent Client Protocol) agent over stdio. The
    /// editor (Zed, Neovim, …) spawns crab as a child process and
    /// drives it via JSON-RPC; all other flags are ignored in this
    /// mode. See <https://agentclientprotocol.com>.
    #[arg(long)]
    acp: bool,

    /// Output format: text (human-readable), json (single JSON result),
    /// stream-json (NDJSON real-time stream).
    #[arg(long, value_enum, default_value = "text")]
    output_format: OutputFormat,

    /// Alias for --output-format json (backward compatible).
    #[arg(long)]
    json: bool,

    /// Include partial message chunks in stream-json output.
    #[arg(long)]
    include_partial_messages: bool,

    /// Include hook lifecycle events in stream-json output.
    #[arg(long)]
    include_hook_events: bool,

    /// Load MCP server configuration from JSON file(s).
    #[arg(long = "mcp-config", num_args = 1..)]
    mcp_config: Vec<PathBuf>,

    /// Ignore MCP servers from settings files, use only --mcp-config.
    #[arg(long)]
    strict_mcp_config: bool,

    /// Load additional settings from a file path or inline JSON string.
    #[arg(long)]
    settings: Option<String>,

    /// Resume a previous session by ID
    #[arg(long)]
    resume: Option<String>,

    /// Print mode: run a single prompt and print the result (non-interactive).
    /// If no prompt is given, reads from stdin.
    #[arg(short = 'p', long)]
    print: bool,

    /// Continue the most recent session for the current directory.
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,

    /// Permission mode: "default", "acceptEdits", "dontAsk", "bypassPermissions", "plan",
    /// "trust-project", "dangerously".
    #[arg(long)]
    permission_mode: Option<String>,

    /// Enable debug logging. Optionally specify a filter (e.g. -d api).
    /// Use without a value for global debug output.
    #[arg(short = 'd', long, num_args = 0..=1, default_missing_value = "")]
    debug: Option<String>,

    /// Write debug logs to a file (in addition to stderr).
    #[arg(long)]
    debug_file: Option<PathBuf>,

    /// Enable verbose output.
    #[arg(long)]
    verbose: bool,

    /// Allowed tools (comma-separated). Supports glob patterns like `Bash(git:*)`.
    #[arg(long = "allowed-tools", alias = "allowedTools", value_delimiter = ',')]
    allowed_tools: Vec<String>,

    /// Disallowed tools (comma-separated). Supports glob patterns like `mcp__*`.
    #[arg(
        long = "disallowed-tools",
        alias = "disallowedTools",
        value_delimiter = ','
    )]
    disallowed_tools: Vec<String>,

    /// Available tool set: "" (disable all), "default" (all), or comma-separated names.
    #[arg(long)]
    tools: Option<String>,

    /// Effort level for reasoning: low, medium, high, max.
    #[arg(long)]
    effort: Option<String>,

    /// Extended thinking mode: enabled, adaptive, disabled.
    #[arg(long)]
    thinking: Option<String>,

    /// Override the default system prompt entirely.
    #[arg(long = "system-prompt")]
    system_prompt_override: Option<String>,

    /// Override the default system prompt from a file.
    #[arg(long = "system-prompt-file")]
    system_prompt_file: Option<PathBuf>,

    /// Append text to the default system prompt.
    #[arg(long = "append-system-prompt")]
    append_system_prompt: Option<String>,

    /// Append text from a file to the default system prompt.
    #[arg(long = "append-system-prompt-file")]
    append_system_prompt_file: Option<PathBuf>,

    /// Additional directories the agent may access (repeatable).
    #[arg(long = "add-dir", num_args = 1..)]
    add_dir: Vec<PathBuf>,

    /// Session display name (shown in /resume list and terminal title).
    #[arg(short = 'n', long)]
    name: Option<String>,

    /// Maximum agent turns in print mode.
    #[arg(long = "max-turns")]
    max_turns: Option<u32>,

    /// Maximum spend in USD in print mode.
    #[arg(long = "max-budget-usd")]
    max_budget_usd: Option<f64>,

    /// Fallback model to use when the primary model is overloaded.
    #[arg(long = "fallback-model")]
    fallback_model: Option<String>,

    // ─── Step 10: bare + no-session-persistence ───
    /// Minimal mode — skip hooks, LSP, plugins, auto-memory, CRAB.md discovery.
    #[arg(long)]
    bare: bool,

    /// Disable session persistence (useful in print mode).
    #[arg(long)]
    no_session_persistence: bool,

    // ─── Step 11: worktree + tmux ───
    /// Create a git worktree. Optionally provide a branch name.
    #[arg(short = 'w', long, num_args = 0..=1, default_missing_value = "")]
    worktree: Option<String>,

    /// Open the worktree in a tmux session (requires --worktree).
    #[arg(long)]
    tmux: bool,

    // ─── Step 12: fork-session + from-pr + session-id + json-schema ───
    /// When resuming, fork into a new session instead of continuing the old one.
    #[arg(long)]
    fork_session: bool,

    /// Load context from a GitHub PR (number or URL). Optionally provide the value.
    #[arg(long = "from-pr", num_args = 0..=1, default_missing_value = "")]
    from_pr: Option<String>,

    /// Use a custom session UUID instead of auto-generating one.
    #[arg(long = "session-id")]
    session_id: Option<String>,

    /// Validate the final output against a JSON Schema (path or inline JSON).
    #[arg(long = "json-schema")]
    json_schema: Option<String>,

    // ─── Step 13: plugin-dir + disable-slash-commands + betas + ide ───
    /// Additional plugin directories to load at runtime (repeatable).
    #[arg(long = "plugin-dir")]
    plugin_dir: Vec<PathBuf>,

    /// Disable all slash commands / skills.
    #[arg(long)]
    disable_slash_commands: bool,

    /// API beta headers to send (repeatable).
    #[arg(long, num_args = 1..)]
    betas: Vec<String>,

    /// Connect to IDE extension automatically.
    #[arg(long)]
    ide: bool,

    /// Control which settings sources to load (comma-separated: user,project,local).
    /// Default: all sources. Example: --setting-sources user,project
    #[arg(long = "setting-sources")]
    setting_sources: Option<String>,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

/// Subcommands for `crab`.
#[derive(Subcommand)]
enum CliCommand {
    /// Manage saved sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Run as an MCP server (expose tools to external MCP clients)
    Serve(commands::serve::ServeArgs),
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: commands::config::ConfigAction,
    },
    /// Manage authentication (login, status, logout, setup-token)
    Auth {
        #[command(subcommand)]
        action: commands::auth::AuthAction,
    },
    /// Run diagnostic checks
    Doctor,
    /// Check for updates, install, or rollback
    Update {
        #[command(subcommand)]
        action: Option<commands::update::UpdateAction>,
    },
    /// Manage plugins (list, install, remove, enable, disable, validate)
    Plugin {
        #[command(subcommand)]
        action: commands::plugin::PluginAction,
    },
    /// List configured agent definitions
    Agents,
    /// Generate shell completion scripts
    Completion {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

/// Session management actions.
#[derive(Subcommand)]
enum SessionAction {
    /// List all saved sessions
    List,
    /// Show the transcript of a saved session
    Show {
        /// Session ID to display
        id: String,
    },
    /// Resume a saved session (alias for `crab --resume <id>`)
    Resume {
        /// Session ID to resume
        id: String,
    },
    /// Delete a saved session
    Delete {
        /// Session ID to delete
        id: String,
    },
    /// Search history sessions for a keyword
    Search {
        /// Keyword to search for
        keyword: String,
    },
    /// Export a session to JSON or Markdown
    Export {
        /// Session ID to export
        id: String,
        /// Output format: "json" or "markdown" (default: markdown)
        #[arg(long, default_value = "markdown")]
        format: String,
    },
    /// Show statistics for a session
    Stats {
        /// Session ID
        id: String,
    },
}

impl Cli {
    /// Resolve effective output format: `--json` flag overrides `--output-format`.
    fn effective_output_format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output_format
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // ACP mode: bypass all interactive / print-mode plumbing and run
    // as a JSON-RPC stdio child process for the spawning editor. All
    // other CLI flags are ignored. `flavor = "current_thread"` because
    // the upstream SDK requires a LocalSet-compatible runtime.
    if cli.acp {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(acp_mode::run());
    }

    // Handle subcommands
    if let Some(command) = &cli.command {
        return match command {
            CliCommand::Config { action } => commands::config::run(action),
            CliCommand::Serve(args) => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(commands::serve::run(args))
            }
            CliCommand::Session { action } => match action {
                SessionAction::List => commands::session::list_sessions(),
                SessionAction::Show { id } => commands::session::show_session(id),
                SessionAction::Resume { id } => {
                    // Validate, then fall through to run the session
                    let _ = commands::session::validate_resume_id(id)?;
                    let rt = tokio::runtime::Runtime::new()?;
                    rt.block_on(run_with_resume(&cli, Some(id.clone())))
                }
                SessionAction::Delete { id } => commands::session::delete_session(id),
                SessionAction::Search { keyword } => commands::session::search_sessions(keyword),
                SessionAction::Export { id, format } => {
                    commands::session::export_session(id, format)
                }
                SessionAction::Stats { id } => commands::session::show_stats(id),
            },
            CliCommand::Auth { action } => commands::auth::run(action),
            CliCommand::Doctor => commands::doctor::run(),
            CliCommand::Update { action } => match action {
                Some(a) => commands::update::run(a),
                None => commands::update::run_default(),
            },
            CliCommand::Plugin { action } => commands::plugin::run(action),
            CliCommand::Agents => commands::agents::run(),
            CliCommand::Completion { shell } => {
                let mut cmd = <Cli as clap::CommandFactory>::command();
                clap_complete::generate(*shell, &mut cmd, "crab", &mut std::io::stdout());
                Ok(())
            }
        };
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run(&cli, cli.resume.clone()))
}

/// Convenience wrapper for `Session resume` subcommand.
async fn run_with_resume(cli: &Cli, resume_id: Option<String>) -> anyhow::Result<()> {
    run(cli, resume_id).await
}

/// Resolve well-known model aliases to their full model IDs.
/// Unknown strings are returned unchanged.
/// Read Coordinator Mode gate from env (no CLI flag by design — insiders opt
/// in via `CRAB_COORDINATOR_MODE=1`). Agent Teams base infrastructure is
/// unconditional; only Coordinator Mode (tool ACL + prompt overlay) is gated.
pub(crate) fn coordinator_mode_enabled() -> bool {
    coordinator_mode_from(|k| std::env::var(k).ok())
}

fn coordinator_mode_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    lookup("CRAB_COORDINATOR_MODE").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
}

fn resolve_model_alias(model: &str) -> String {
    match model {
        "sonnet" => "claude-sonnet-4-6".to_string(),
        "opus" => "claude-opus-4-6".to_string(),
        "haiku" => "claude-haiku-4-5-20251001".to_string(),
        other => other.to_string(),
    }
}

/// Resolve `--allowed-tools`, `--disallowed-tools`, and `--tools` into effective
/// allowed/denied lists.
///
/// `--tools ""` disables all tools (denied = `["*"]`).
/// `--tools "default"` allows all (no filtering).
/// `--tools "read,write"` restricts to named tools only (allowed = those names).
fn resolve_tool_filters(
    allowed: &[String],
    disallowed: &[String],
    tools: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut effective_allowed = allowed.to_vec();
    let mut effective_denied = disallowed.to_vec();

    if let Some(tools_arg) = tools {
        let trimmed = tools_arg.trim();
        if trimmed.is_empty() {
            // Empty string: disable all tools
            effective_denied = vec!["*".to_string()];
        } else if trimmed == "default" {
            // "default": no filtering
        } else {
            // Explicit list: only these tools are allowed
            let names: Vec<String> = trimmed
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            effective_allowed = names;
        }
    }

    (effective_allowed, effective_denied)
}

/// Load a `--settings` argument: if it looks like JSON (starts with `{`),
/// parse it directly; otherwise treat it as a file path and read it.
fn load_settings_arg(arg: &str) -> anyhow::Result<crab_config::Settings> {
    let content = if arg.trim_start().starts_with('{') {
        arg.to_string()
    } else {
        std::fs::read_to_string(arg)
            .map_err(|e| anyhow::anyhow!("failed to read settings file '{arg}': {e}"))?
    };
    serde_json::from_str(&content).map_err(|e| anyhow::anyhow!("failed to parse settings: {e}"))
}

/// Load MCP server configurations from one or more JSON files and merge them.
fn load_mcp_configs(paths: &[PathBuf]) -> anyhow::Result<Value> {
    let mut merged = serde_json::Map::new();
    for path in paths {
        let content = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!("failed to read MCP config '{}': {}", path.display(), e)
        })?;
        let parsed: Value = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("failed to parse MCP config '{}': {}", path.display(), e)
        })?;
        if let Value::Object(map) = parsed {
            for (k, v) in map {
                merged.insert(k, v);
            }
        } else {
            anyhow::bail!("MCP config '{}' must be a JSON object", path.display());
        }
    }
    Ok(Value::Object(merged))
}

/// Resolve the effective system prompt from CLI flags.
///
/// Priority:
/// 1. `--system-prompt` / `--system-prompt-file` — replaces the default entirely.
/// 2. `--append-system-prompt` / `--append-system-prompt-file` — appends to the default.
/// 3. Default: `build_system_prompt(...)` with optional settings-level custom instructions.
fn resolve_system_prompt(
    cli: &Cli,
    working_dir: &std::path::Path,
    registry: &crab_tools::registry::ToolRegistry,
    settings_system_prompt: Option<&str>,
) -> anyhow::Result<String> {
    // Check for override: --system-prompt or --system-prompt-file
    let override_prompt = if let Some(ref prompt) = cli.system_prompt_override {
        Some(prompt.clone())
    } else if let Some(ref path) = cli.system_prompt_file {
        Some(std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "failed to read --system-prompt-file '{}': {}",
                path.display(),
                e
            )
        })?)
    } else {
        None
    };

    // Check for append: --append-system-prompt or --append-system-prompt-file
    let append_prompt = if let Some(ref prompt) = cli.append_system_prompt {
        Some(prompt.clone())
    } else if let Some(ref path) = cli.append_system_prompt_file {
        Some(std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "failed to read --append-system-prompt-file '{}': {}",
                path.display(),
                e
            )
        })?)
    } else {
        None
    };

    let mut system_prompt = if let Some(override_text) = override_prompt {
        // Full override: skip default prompt entirely
        override_text
    } else {
        build_system_prompt(working_dir, registry, settings_system_prompt)
    };

    // Append if requested
    if let Some(append_text) = append_prompt {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&append_text);
    }

    Ok(system_prompt)
}

#[allow(clippy::too_many_lines)]
async fn run(cli: &Cli, resume_session_id: Option<String>) -> anyhow::Result<()> {
    // Initialise debug/tracing if requested
    let debug_filter = crab_common::utils::debug::resolve_debug_filter(cli.debug.as_deref());
    let debug_config = crab_common::utils::debug::DebugConfig {
        enabled: debug_filter.is_some() || cli.verbose,
        filter: debug_filter,
        file: cli.debug_file.clone(),
    };
    crab_common::utils::debug::init_debug(&debug_config);

    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Load merged settings with optional source control
    let sources = cli
        .setting_sources
        .as_ref()
        .map(|s| crab_config::settings::SettingSource::parse_list(s));
    let mut settings = crab_config::settings::load_merged_settings_with_sources(
        Some(&working_dir),
        sources.as_deref(),
    )?;

    // Apply --settings overlay (higher priority than settings files, lower than CLI flags)
    if let Some(ref settings_arg) = cli.settings {
        let overlay = load_settings_arg(settings_arg)?;
        settings = settings.merge(&overlay);
    }

    // Apply --mcp-config: load MCP server configs from file(s)
    if !cli.mcp_config.is_empty() {
        let mcp = load_mcp_configs(&cli.mcp_config)?;
        if cli.strict_mcp_config {
            settings.mcp_servers = Some(mcp);
        } else {
            // Merge: existing servers + file-loaded servers
            let existing = settings.mcp_servers.take().unwrap_or(json!({}));
            if let (Value::Object(mut base), Value::Object(overlay)) = (existing, mcp) {
                for (k, v) in overlay {
                    base.insert(k, v);
                }
                settings.mcp_servers = Some(Value::Object(base));
            }
        }
    }

    // settings already has config.toml → user → project → local → env merged.
    // CLI --provider/--model override the merged result.
    let provider = if cli.provider == "anthropic" {
        settings
            .api_provider
            .clone()
            .unwrap_or_else(|| "anthropic".to_string())
    } else {
        cli.provider.clone()
    };
    let model_id = cli
        .model
        .as_deref()
        .map(resolve_model_alias)
        .or_else(|| settings.model.clone())
        .unwrap_or_else(|| {
            if provider == "openai" || provider == "deepseek" || provider == "ollama" {
                "deepseek-chat".to_string()
            } else {
                "claude-sonnet-4-6".to_string()
            }
        });

    // Build effective settings — just override provider and model from CLI
    let effective_settings = crab_config::Settings {
        api_provider: Some(provider.clone()),
        model: Some(model_id.clone()),
        ..settings.clone()
    };

    let backend = Arc::new(crab_api::create_backend(&effective_settings));
    let mut registry = create_default_registry();

    // Connect to MCP servers and register their tools
    let mut _mcp_manager = if let Some(ref mcp_value) = settings.mcp_servers {
        let mut mgr = crab_mcp::McpManager::new();
        let failed = mgr.start_all(mcp_value).await.unwrap_or_else(|e| {
            eprintln!("Warning: failed to parse MCP config: {e}");
            Vec::new()
        });
        for name in &failed {
            eprintln!("Warning: MCP server '{name}' failed to connect");
        }
        let count = crab_tools::builtin::mcp_tool::register_mcp_tools(&mgr, &mut registry).await;
        if count > 0 {
            eprintln!("Registered {count} MCP tool(s).");
        }
        Some(mgr)
    } else {
        None
    };

    // Discover skills from global + project directories (--bare and --disable-slash-commands skip)
    let mut skill_dirs = if cli.bare || cli.disable_slash_commands {
        Vec::new()
    } else {
        build_skill_dirs(&working_dir)
    };
    // --plugin-dir adds extra directories
    for dir in &cli.plugin_dir {
        skill_dirs.push(dir.clone());
    }
    let skill_registry = crab_skill::SkillRegistry::discover(&skill_dirs).unwrap_or_default();
    if !skill_registry.is_empty() {
        eprintln!("Loaded {} skill(s).", skill_registry.len());
    }

    // Build system prompt: --system-prompt overrides entirely; --append-system-prompt appends.
    let system_prompt = resolve_system_prompt(
        cli,
        &working_dir,
        &registry,
        effective_settings.system_prompt.as_deref(),
    )?;

    // Resolve permission mode: --permission-mode > legacy flags > settings file > default
    let permission_mode = if let Some(ref mode_str) = cli.permission_mode {
        mode_str
            .parse::<PermissionMode>()
            .map_err(|e| anyhow::anyhow!(e))?
    } else if cli.dangerously_skip_permissions {
        PermissionMode::Dangerously
    } else if cli.trust_project {
        PermissionMode::TrustProject
    } else {
        settings
            .permission_mode
            .as_deref()
            .and_then(|s| s.parse::<PermissionMode>().ok())
            .unwrap_or(PermissionMode::Default)
    };

    let global_dir = crab_config::settings::global_config_dir();
    let sessions_dir = global_dir.join("sessions");

    // --no-session-persistence disables session saving
    let effective_sessions_dir = if cli.no_session_persistence || cli.bare {
        None
    } else {
        Some(sessions_dir.clone())
    };

    // Resolve resume ID: explicit --resume > -c (continue latest) > None
    let effective_resume_id = if resume_session_id.is_some() {
        resume_session_id
    } else if cli.continue_session {
        let history = crab_session::SessionHistory::new(sessions_dir.clone());
        let found = history.find_latest_for_dir(&working_dir);
        if found.is_none() {
            eprintln!("No previous session found to continue.");
        }
        found
    } else {
        None
    };

    // --fork-session: when resuming, generate a new session ID (fork) instead of reusing
    // --session-id: override auto-generated session ID
    let session_id = if let Some(ref id) = cli.session_id {
        id.clone()
    } else if cli.fork_session && effective_resume_id.is_some() {
        crab_common::utils::id::new_ulid()
    } else {
        crab_common::utils::id::new_ulid()
    };

    // Build allowed/denied tool lists from CLI flags
    let (effective_allowed, effective_denied) = resolve_tool_filters(
        &cli.allowed_tools,
        &cli.disallowed_tools,
        cli.tools.as_deref(),
    );

    // Resolve effort level and thinking mode
    let effort = cli.effort.as_deref().map(str::to_lowercase);
    let thinking_mode = cli.thinking.as_deref().map(str::to_lowercase);

    // Validate effort if provided
    if let Some(ref e) = effort
        && !matches!(e.as_str(), "low" | "medium" | "high" | "max")
    {
        anyhow::bail!("invalid --effort value: '{e}'. Valid: low, medium, high, max");
    }
    // Validate thinking if provided
    if let Some(ref t) = thinking_mode
        && !matches!(t.as_str(), "enabled" | "adaptive" | "disabled")
    {
        anyhow::bail!("invalid --thinking value: '{t}'. Valid: enabled, adaptive, disabled");
    }

    // --bare skips memory
    let effective_memory_dir = if cli.bare {
        None
    } else {
        Some(global_dir.join("memory"))
    };

    // Coordinator Mode gate (env only; Teams infra is unconditional).
    let coordinator_mode = coordinator_mode_enabled();

    let session_config = SessionConfig {
        session_id,
        system_prompt,
        model: ModelId::from(model_id.as_str()),
        max_tokens: cli.max_tokens,
        temperature: None,
        context_window: 200_000,
        working_dir,
        permission_policy: PermissionPolicy {
            mode: permission_mode,
            allowed_tools: effective_allowed,
            denied_tools: effective_denied,
        },
        memory_dir: effective_memory_dir,
        sessions_dir: effective_sessions_dir,
        resume_session_id: effective_resume_id,
        effort,
        thinking_mode,
        additional_dirs: cli.add_dir.clone(),
        session_name: cli.name.clone(),
        max_turns: cli.max_turns,
        max_budget_usd: cli.max_budget_usd,
        fallback_model: cli.fallback_model.clone(),
        bare_mode: cli.bare,
        worktree_name: cli.worktree.clone(),
        fork_session: cli.fork_session,
        from_pr: cli.from_pr.clone(),
        custom_session_id: cli.session_id.clone(),
        json_schema: cli.json_schema.clone(),
        plugin_dirs: cli.plugin_dir.clone(),
        disable_skills: cli.disable_slash_commands,
        beta_headers: cli.betas.clone(),
        ide_connect: cli.ide,
        coordinator_mode,
    };

    // Determine the effective prompt: positional arg, or stdin if -p with no prompt
    let effective_prompt = if let Some(ref prompt) = cli.prompt {
        Some(prompt.clone())
    } else if cli.print {
        // -p without positional prompt: read from stdin
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Some(buf)
    } else {
        None
    };

    if let Some(prompt) = effective_prompt {
        // Single-shot mode
        print_banner(
            env!("CARGO_PKG_VERSION"),
            &provider,
            &model_id,
            &permission_mode,
        );
        let resolved = resolve_slash_command(&prompt, &skill_registry);
        let mut session = AgentSession::new(session_config, backend, registry);
        session
            .executor
            .set_permission_handler(Arc::new(CliPermissionHandler));
        run_single_shot(&mut session, &resolved, cli.effective_output_format()).await
    } else {
        // Interactive mode: TUI if available, else line-based REPL
        #[cfg(feature = "tui")]
        {
            let tui_config = crab_tui::TuiConfig {
                session_config,
                backend,
                skill_dirs,
                mcp_servers: settings.mcp_servers.clone(),
            };
            let exit_info = crab_tui::run(tui_config).await?;
            print_exit_info(&exit_info);
            Ok(())
        }
        #[cfg(not(feature = "tui"))]
        {
            print_banner(
                env!("CARGO_PKG_VERSION"),
                &provider,
                &model_id,
                &permission_mode,
            );
            let mut session = AgentSession::new(session_config, backend, registry);
            session
                .executor
                .set_permission_handler(Arc::new(CliPermissionHandler));
            eprintln!("Type /exit or Ctrl+D to quit.\n");
            run_repl(&mut session, &skill_registry).await
        }
    }
}

/// Build the list of skill directories to scan.
fn build_skill_dirs(working_dir: &std::path::Path) -> Vec<PathBuf> {
    // Global skills: ~/.crab/skills/
    // Project skills: <project>/.crab/skills/
    vec![
        crab_config::settings::global_config_dir().join("skills"),
        working_dir.join(".crab").join("skills"),
    ]
}

/// If input starts with `/`, try to match a skill command and return its content
/// as the prompt. Otherwise return the original input.
fn resolve_slash_command(input: &str, skill_registry: &crab_skill::SkillRegistry) -> String {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return input.to_string();
    }

    // Extract the command name (first word after /)
    let command = trimmed
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");

    // Check built-in commands first
    if matches!(command, "exit" | "quit" | "help") {
        return input.to_string();
    }

    // Look up in skill registry
    if let Some(skill) = skill_registry.find_command(command) {
        // The rest of the input after the /command becomes arguments
        let args = trimmed
            .trim_start_matches('/')
            .trim_start_matches(command)
            .trim();

        let mut prompt = skill.content.clone();
        if !args.is_empty() {
            prompt.push_str("\n\nUser arguments: ");
            prompt.push_str(args);
        }

        eprintln!("[skill] Activated: {} — {}", skill.name, skill.description);
        return prompt;
    }

    // No matching skill — pass through as-is
    input.to_string()
}

/// CLI-based permission handler: prints prompt to stderr, reads y/n from stdin.
struct CliPermissionHandler;

impl PermissionHandler for CliPermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let prompt = prompt.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                use std::io::BufRead;
                eprint!("[permission] {prompt} ({tool_name}) [y/N] ");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                if std::io::stdin().lock().read_line(&mut line).is_ok() {
                    let answer = line.trim().to_lowercase();
                    answer == "y" || answer == "yes"
                } else {
                    false
                }
            })
            .await
            .unwrap_or(false)
        })
    }
}

/// Run a single prompt, print the result, and exit.
async fn run_single_shot(
    session: &mut AgentSession,
    prompt: &str,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let event_rx = take_event_rx(session);
    let registry = session.executor.registry_arc();
    let printer = tokio::spawn(print_events(event_rx, output_format, registry));

    let result = session.handle_user_input(prompt).await;
    // Replace the event_tx with a dummy so the printer's rx sees all senders dropped.
    let (dummy_tx, dummy_rx) = mpsc::channel::<Event>(1);
    session.event_tx = dummy_tx;
    drop(dummy_rx);
    let _ = printer.await;

    result.map_err(Into::into)
}

/// Interactive REPL: read lines, send to agent, print streaming output.
#[cfg(not(feature = "tui"))]
async fn run_repl(
    session: &mut AgentSession,
    skill_registry: &crab_skill::SkillRegistry,
) -> anyhow::Result<()> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let slash_registry = crab_agent::SlashCommandRegistry::new();

    loop {
        // Print prompt
        print!("crab> ");
        stdout.flush()?;

        // Read a line
        let mut line = String::new();
        let bytes_read = stdin.lock().read_line(&mut line)?;

        // Ctrl+D (EOF)
        if bytes_read == 0 {
            eprintln!("\nGoodbye!");
            break;
        }

        let input = line.trim();

        if input.is_empty() {
            continue;
        }

        // Intercept slash commands before they hit the LLM. Only treat input
        // starting with `/<letter>` as a slash command so real paths like
        // `/tmp/foo` still flow through as prompts.
        if let Some(cmd_rest) = input.strip_prefix('/')
            && cmd_rest
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            match dispatch_slash_command(session, skill_registry, &slash_registry, cmd_rest).await {
                SlashOutcome::Continue => continue,
                SlashOutcome::Exit => break,
                SlashOutcome::FallThrough(expanded) => {
                    run_turn(session, &expanded).await;
                    continue;
                }
            }
        }

        run_turn(session, input).await;
    }

    Ok(())
}

/// Outcome of a slash-command dispatch that controls the REPL loop.
#[cfg(not(feature = "tui"))]
enum SlashOutcome {
    /// Stay in the loop (message was printed, action handled).
    Continue,
    /// Exit the REPL.
    Exit,
    /// Expand the input (e.g. a user-defined skill) and feed it back as a turn.
    FallThrough(String),
}

/// Parse and execute a slash command. Returns the loop control outcome.
///
/// `cmd_rest` is the input with the leading `/` already stripped.
#[cfg(not(feature = "tui"))]
async fn dispatch_slash_command(
    session: &mut AgentSession,
    skill_registry: &crab_skill::SkillRegistry,
    slash_registry: &crab_agent::SlashCommandRegistry,
    cmd_rest: &str,
) -> SlashOutcome {
    let (name, args) = cmd_rest
        .split_once(char::is_whitespace)
        .map_or((cmd_rest, ""), |(n, a)| (n, a.trim()));

    // Built-in shortcut: /exit and /quit terminate the REPL even before the
    // registry is consulted (cmd_exit returns SlashAction::Exit, same effect,
    // but this keeps REPL termination independent of registry state).
    if matches!(name, "exit" | "quit") {
        eprintln!("Goodbye!");
        return SlashOutcome::Exit;
    }

    // Snapshot session fields before executing (execute borrows session
    // through the context, but we also read session state below).
    let ctx = crab_agent::SlashCommandContext {
        model: &session.config.model,
        session_id: &session.conversation.id,
        working_dir: &session.tool_ctx.working_dir,
        permission_mode: session.tool_ctx.permission_mode,
        cost: &session.cost,
        estimated_tokens: 0,
        message_count: session.conversation.len(),
        memory_dir: session
            .memory_store
            .as_ref()
            .map(|_| std::path::Path::new(".crab/memory")),
    };

    let result = slash_registry.execute(name, args, &ctx);

    match result {
        Some(crab_agent::SlashCommandResult::Message(msg)) => {
            println!("{msg}");
            SlashOutcome::Continue
        }
        Some(crab_agent::SlashCommandResult::Action(action)) => {
            handle_slash_action(session, action).await
        }
        Some(crab_agent::SlashCommandResult::Silent) => SlashOutcome::Continue,
        None => {
            // Unknown to our registry — fall back to skill expansion so
            // user-defined `/my-skill` style commands still resolve.
            let expanded = resolve_slash_command(&format!("/{cmd_rest}"), skill_registry);
            if expanded == format!("/{cmd_rest}") {
                eprintln!("Unknown command: /{name}. Try /help.");
                SlashOutcome::Continue
            } else {
                SlashOutcome::FallThrough(expanded)
            }
        }
    }
}

/// Apply a [`SlashAction`](crab_agent::SlashAction) to the running session.
#[cfg(not(feature = "tui"))]
async fn handle_slash_action(
    session: &mut AgentSession,
    action: crab_agent::SlashAction,
) -> SlashOutcome {
    use crab_agent::SlashAction;

    match action {
        SlashAction::Exit => {
            eprintln!("Goodbye!");
            SlashOutcome::Exit
        }
        SlashAction::Clear => {
            session.conversation.clear();
            println!("[info] Conversation cleared. System prompt and cost accumulator retained.");
            SlashOutcome::Continue
        }
        SlashAction::Compact => {
            let before = session.conversation.len();
            let summary = session.compact_conversation().await;
            let after = session.conversation.len();
            println!(
                "[info] Compacted {before} messages → {after} (extracted {} summary items).",
                summary.items.len()
            );
            SlashOutcome::Continue
        }
        SlashAction::Rewind(target) => {
            let what = target.as_deref().unwrap_or("(most-recent edit)");
            println!(
                "[info] /rewind {what}: the `file_history` primitive is ready but \
                 Edit/Write/Notebook tools do not yet call track_edit via ToolContextExt. \
                 Wire-up lands in a follow-up."
            );
            SlashOutcome::Continue
        }
        other => {
            // SwitchModel / Resume / Export / Init / TogglePlanMode / SetEffort /
            // ToggleFast / AddDir / CopyLast all need plumbing beyond REPL
            // state; accept the command but inform the user the runtime
            // effect is pending.
            println!("[info] Slash action {other:?} is not yet wired in the REPL.");
            SlashOutcome::Continue
        }
    }
}

/// Run one turn of the agent loop for a plain user prompt.
#[cfg(not(feature = "tui"))]
async fn run_turn(session: &mut AgentSession, input: &str) {
    let event_rx = take_event_rx(session);
    let registry = session.executor.registry_arc();
    let printer = tokio::spawn(print_events(event_rx, OutputFormat::Text, registry));

    if let Err(e) = session.handle_user_input(input).await {
        eprintln!("\n[error] {e}");
    }

    // Replace tx so the printer's rx sees all senders dropped and finishes.
    let (fresh_tx, fresh_rx) = mpsc::channel::<Event>(1);
    session.event_tx = fresh_tx;
    drop(fresh_rx);
    let _ = printer.await;
    println!();
}

/// Swap the session's `event_rx` with a fresh one, returning the old receiver.
fn take_event_rx(session: &mut AgentSession) -> mpsc::Receiver<Event> {
    // Create a fresh channel: session sends via tx, printer reads via rx.
    let (tx, rx) = mpsc::channel(256);
    session.event_tx = tx;
    rx
}

/// Drain events from the receiver and print them to stdout/stderr.
///
/// `OutputFormat::Json` and `StreamJson` emit NDJSON to stdout.
/// `OutputFormat::Text` uses colored human-readable output.
async fn print_events(
    mut rx: mpsc::Receiver<Event>,
    output_format: OutputFormat,
    registry: Arc<crab_tools::registry::ToolRegistry>,
) {
    let mut stdout = std::io::stdout();
    let mut spinner: Option<Spinner> = None;

    while let Some(event) = rx.recv().await {
        match output_format {
            OutputFormat::Json | OutputFormat::StreamJson => {
                if let Some(value) = event_to_json(&event)
                    && let Ok(line) = serde_json::to_string(&value)
                {
                    println!("{line}");
                }
                continue;
            }
            OutputFormat::Text => {}
        }

        match event {
            // Only print text content (index 0), not tool arguments (index 1000+)
            Event::ContentDelta { index: 0, delta } => {
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                print!("{delta}");
                let _ = stdout.flush();
            }
            Event::ToolUseStart { name, input, .. } => {
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                let summary = registry
                    .get(&name)
                    .and_then(|tool| tool.format_use_summary(&input))
                    .unwrap_or_else(|| name.clone());
                eprintln!("{} {}", "tool:".cyan().bold(), summary.cyan());
                spinner = Some(Spinner::start(&format!("running {name}...")));
            }
            Event::ToolOutputDelta { id: _, delta } => {
                // Stream tool output in real-time (e.g. bash stdout)
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                eprint!("{delta}");
                let _ = std::io::stderr().flush();
            }
            Event::ToolResult { id: _, output: o } => {
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                let text = o.text();
                if o.is_error {
                    eprintln!("{} {text}", "tool error:".red().bold());
                } else {
                    let display = if text.len() > 500 {
                        format!("{}...", &text[..500])
                    } else {
                        text
                    };
                    eprintln!("{} {display}", "result:".dimmed());
                }
            }
            Event::Error { message } => {
                eprintln!("{} {message}", "error:".red().bold());
            }
            Event::TokenWarning {
                usage_pct,
                used,
                limit,
            } => {
                eprintln!(
                    "{} Token usage {:.0}% ({used}/{limit})",
                    "warn:".yellow().bold(),
                    usage_pct * 100.0,
                );
            }
            Event::CompactStart { strategy, .. } => {
                eprintln!(
                    "{} Starting compaction: {strategy}",
                    "compact:".magenta().bold()
                );
            }
            Event::CompactEnd {
                after_tokens,
                removed_messages,
            } => {
                eprintln!(
                    "{} removed {removed_messages} messages, now {after_tokens} tokens",
                    "compact:".magenta().bold()
                );
            }
            _ => {}
        }
    }
}

// ─── Inlined output helpers ──────────────────────────────────────

#[cfg(feature = "tui")]
pub fn print_exit_info(info: &crab_tui::ExitInfo) {
    if info.had_conversation && !info.session_id.is_empty() {
        eprintln!(
            "\n{}\n",
            format!(
                "Resume this session with:\ncrab --resume {}",
                info.session_id
            )
            .dimmed()
        );
    }
}

fn print_banner(version: &str, provider: &str, model: &str, permission_mode: &impl fmt::Display) {
    eprintln!(
        "{} {} {} provider={} model={} permissions={}",
        "crab-code".green().bold(),
        version.dimmed(),
        "|".dimmed(),
        provider.cyan(),
        model.cyan(),
        format!("{permission_mode}").yellow(),
    );
}

struct Spinner {
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    fn start(message: &str) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let msg = message.to_string();

        let handle = std::thread::spawn(move || {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let mut i = 0;
            while running_clone.load(Ordering::Relaxed) {
                eprint!(
                    "\r{} {}",
                    frames[i % frames.len()].to_string().cyan(),
                    msg.dimmed()
                );
                let _ = std::io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_millis(80));
                i += 1;
            }
            eprint!("\r{}\r", " ".repeat(msg.len() + 4));
            let _ = std::io::stderr().flush();
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

fn event_to_json(event: &Event) -> Option<Value> {
    match event {
        Event::TurnStart { turn_index } => Some(json!({
            "type": "turn_start",
            "turn_index": turn_index,
        })),
        Event::MessageStart { id } => Some(json!({
            "type": "message_start",
            "id": id,
            "role": "assistant",
        })),
        Event::ContentDelta { index, delta } => Some(json!({
            "type": "content_delta",
            "index": index,
            "delta": delta,
        })),
        Event::ThinkingDelta { index, delta } => Some(json!({
            "type": "thinking_delta",
            "index": index,
            "delta": delta,
        })),
        Event::ContentBlockStop { index } => Some(json!({
            "type": "content_block_stop",
            "index": index,
        })),
        Event::MessageEnd { usage } => Some(json!({
            "type": "message_end",
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cache_read_tokens": usage.cache_read_tokens,
                "cache_creation_tokens": usage.cache_creation_tokens,
            },
        })),
        Event::ToolUseStart { name, id, input } => Some(json!({
            "type": "tool_use_start",
            "tool": name,
            "id": id,
            "input": input,
        })),
        Event::ToolUseInput { id, input } => Some(json!({
            "type": "tool_use_input",
            "id": id,
            "input": input,
        })),
        Event::ToolOutputDelta { id, delta } => Some(json!({
            "type": "tool_output_delta",
            "id": id,
            "delta": delta,
        })),
        Event::ToolProgress { id, progress } => Some(json!({
            "type": "tool_progress",
            "id": id,
            "elapsed_secs": progress.elapsed_secs,
            "total_lines": progress.total_lines,
            "total_bytes": progress.total_bytes,
        })),
        Event::ToolResult { id, output } => Some(json!({
            "type": "tool_result",
            "id": id,
            "is_error": output.is_error,
            "text": output.text(),
        })),
        Event::Error { message } => Some(json!({
            "type": "error",
            "message": message,
        })),
        Event::TokenWarning {
            usage_pct,
            used,
            limit,
        } => Some(json!({
            "type": "token_warning",
            "usage_pct": usage_pct,
            "used": used,
            "limit": limit,
        })),
        Event::CompactStart {
            strategy,
            before_tokens,
        } => Some(json!({
            "type": "compact_start",
            "strategy": strategy,
            "before_tokens": before_tokens,
        })),
        Event::CompactEnd {
            after_tokens,
            removed_messages,
        } => Some(json!({
            "type": "compact_end",
            "after_tokens": after_tokens,
            "removed_messages": removed_messages,
        })),
        Event::PermissionRequest {
            tool_name,
            input_summary,
            request_id,
        } => Some(json!({
            "type": "permission_request",
            "tool_name": tool_name,
            "input_summary": input_summary,
            "request_id": request_id,
        })),
        Event::PermissionResponse {
            request_id,
            allowed,
        } => Some(json!({
            "type": "permission_response",
            "request_id": request_id,
            "allowed": allowed,
        })),
        Event::MemoryLoaded { count } => Some(json!({
            "type": "memory_loaded",
            "count": count,
        })),
        Event::MemorySaved { filename } => Some(json!({
            "type": "memory_saved",
            "filename": filename,
        })),
        Event::SessionSaved { session_id } => Some(json!({
            "type": "session_saved",
            "session_id": session_id,
        })),
        Event::SessionResumed {
            session_id,
            message_count,
        } => Some(json!({
            "type": "session_resumed",
            "session_id": session_id,
            "message_count": message_count,
        })),
        Event::AgentWorkerStarted {
            worker_id,
            task_prompt,
        } => Some(json!({
            "type": "agent_worker_started",
            "worker_id": worker_id,
            "task_prompt": task_prompt,
        })),
        Event::AgentWorkerCompleted {
            worker_id,
            result,
            success,
            usage,
        } => Some(json!({
            "type": "agent_worker_completed",
            "worker_id": worker_id,
            "result": result,
            "success": success,
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cache_read_tokens": usage.cache_read_tokens,
                "cache_creation_tokens": usage.cache_creation_tokens,
            },
        })),
        // Remote/sandbox/proactive/query-phase events are not rendered
        // in -p/--print structured output; they are for TUI and SDK consumers.
        Event::RemoteStatusChanged(_)
        | Event::RemoteClientConnected { .. }
        | Event::RemoteClientDisconnected { .. }
        | Event::RemoteSessionStarted(_)
        | Event::RemoteSessionProgress { .. }
        | Event::RemoteSessionCompleted { .. }
        | Event::SandboxViolation { .. }
        | Event::ProactiveSuggestion(_)
        | Event::QueryPhaseChanged(_)
        | Event::QueryStopped { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_skill::{Skill, SkillRegistry, SkillTrigger};

    #[test]
    fn coordinator_gate_off_when_env_unset() {
        assert!(!coordinator_mode_from(|_| None));
    }

    #[test]
    fn coordinator_gate_on_with_1() {
        assert!(coordinator_mode_from(
            |k| (k == "CRAB_COORDINATOR_MODE").then(|| "1".into())
        ));
    }

    #[test]
    fn coordinator_gate_accepts_true_word_variants() {
        for v in ["true", "TRUE"] {
            assert!(coordinator_mode_from(
                |k| (k == "CRAB_COORDINATOR_MODE").then(|| v.into())
            ));
        }
    }

    #[test]
    fn coordinator_gate_rejects_unknown_values() {
        for v in ["yes", "0", "on", "false", ""] {
            assert!(
                !coordinator_mode_from(|k| (k == "CRAB_COORDINATOR_MODE").then(|| v.into())),
                "value {v:?} should not enable coordinator"
            );
        }
    }

    #[test]
    fn build_skill_dirs_includes_global_and_project() {
        let dirs = build_skill_dirs(std::path::Path::new("/tmp/project"));
        // Should contain at least the project skills dir
        assert!(dirs.iter().any(|d| d.ends_with(".crab/skills")));
    }

    #[test]
    fn resolve_slash_command_passthrough_non_slash() {
        let reg = SkillRegistry::new();
        assert_eq!(resolve_slash_command("hello world", &reg), "hello world");
    }

    #[test]
    fn resolve_slash_command_builtin_passthrough() {
        let reg = SkillRegistry::new();
        assert_eq!(resolve_slash_command("/exit", &reg), "/exit");
        assert_eq!(resolve_slash_command("/quit", &reg), "/quit");
        assert_eq!(resolve_slash_command("/help", &reg), "/help");
    }

    #[test]
    fn resolve_slash_command_no_match_passthrough() {
        let reg = SkillRegistry::new();
        assert_eq!(
            resolve_slash_command("/unknown-skill", &reg),
            "/unknown-skill"
        );
    }

    #[test]
    fn resolve_slash_command_matches_skill() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            trigger: SkillTrigger::Command {
                name: "commit".into(),
            },
            ..Skill::new("commit", "You are a commit helper.")
        });

        let result = resolve_slash_command("/commit", &reg);
        assert_eq!(result, "You are a commit helper.");
    }

    #[test]
    fn resolve_slash_command_with_args() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            trigger: SkillTrigger::Command {
                name: "review".into(),
            },
            ..Skill::new("review", "Review the code.")
        });

        let result = resolve_slash_command("/review src/main.rs", &reg);
        assert!(result.contains("Review the code."));
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn cli_parses_json_flag() {
        let cli = Cli::try_parse_from(["crab", "--json", "hello"]).unwrap();
        assert!(cli.json);
        assert_eq!(cli.prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn cli_json_defaults_to_false() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(!cli.json);
    }

    #[test]
    fn cli_parses_print_flag() {
        let cli = Cli::try_parse_from(["crab", "-p", "hello"]).unwrap();
        assert!(cli.print);
        assert_eq!(cli.prompt.as_deref(), Some("hello"));

        let cli2 = Cli::try_parse_from(["crab", "--print"]).unwrap();
        assert!(cli2.print);
        assert!(cli2.prompt.is_none());
    }

    #[test]
    fn cli_parses_continue_flag() {
        let cli = Cli::try_parse_from(["crab", "-c"]).unwrap();
        assert!(cli.continue_session);

        let cli2 = Cli::try_parse_from(["crab", "--continue"]).unwrap();
        assert!(cli2.continue_session);
    }

    #[test]
    fn cli_parses_permission_mode() {
        let cli =
            Cli::try_parse_from(["crab", "--permission-mode", "acceptEdits", "hello"]).unwrap();
        assert_eq!(cli.permission_mode.as_deref(), Some("acceptEdits"));
    }

    #[test]
    fn resolve_model_alias_known() {
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
    }

    #[test]
    fn resolve_model_alias_passthrough() {
        assert_eq!(resolve_model_alias("gpt-4o"), "gpt-4o");
        assert_eq!(
            resolve_model_alias("claude-sonnet-4-20250514"),
            "claude-sonnet-4-20250514"
        );
    }

    #[test]
    fn cli_parses_debug_flag() {
        // -d without value
        let cli = Cli::try_parse_from(["crab", "-d"]).unwrap();
        assert_eq!(cli.debug.as_deref(), Some(""));

        // -d with value
        let cli2 = Cli::try_parse_from(["crab", "-d", "api"]).unwrap();
        assert_eq!(cli2.debug.as_deref(), Some("api"));

        // --debug without value
        let cli3 = Cli::try_parse_from(["crab", "--debug"]).unwrap();
        assert_eq!(cli3.debug.as_deref(), Some(""));

        // No debug flag
        let cli4 = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(cli4.debug.is_none());
    }

    #[test]
    fn cli_parses_debug_file() {
        let cli = Cli::try_parse_from(["crab", "--debug-file", "/tmp/debug.log"]).unwrap();
        assert_eq!(
            cli.debug_file.as_deref(),
            Some(std::path::Path::new("/tmp/debug.log"))
        );
    }

    #[test]
    fn cli_parses_verbose() {
        let cli = Cli::try_parse_from(["crab", "--verbose"]).unwrap();
        assert!(cli.verbose);
    }

    // ─── OutputFormat tests ───

    #[test]
    fn cli_parses_output_format_text() {
        let cli = Cli::try_parse_from(["crab", "--output-format", "text", "hello"]).unwrap();
        assert_eq!(cli.output_format, OutputFormat::Text);
    }

    #[test]
    fn cli_parses_output_format_json() {
        let cli = Cli::try_parse_from(["crab", "--output-format", "json", "hello"]).unwrap();
        assert_eq!(cli.output_format, OutputFormat::Json);
    }

    #[test]
    fn cli_parses_output_format_stream_json() {
        let cli = Cli::try_parse_from(["crab", "--output-format", "stream-json", "hello"]).unwrap();
        assert_eq!(cli.output_format, OutputFormat::StreamJson);
    }

    #[test]
    fn cli_json_flag_overrides_output_format() {
        let cli =
            Cli::try_parse_from(["crab", "--output-format", "text", "--json", "hello"]).unwrap();
        assert_eq!(cli.effective_output_format(), OutputFormat::Json);
    }

    #[test]
    fn cli_effective_output_format_no_json_flag() {
        let cli = Cli::try_parse_from(["crab", "--output-format", "stream-json", "hello"]).unwrap();
        assert_eq!(cli.effective_output_format(), OutputFormat::StreamJson);
    }

    // ─── MCP config / settings CLI arg tests ───

    #[test]
    fn cli_parses_mcp_config() {
        let cli = Cli::try_parse_from(["crab", "--mcp-config", "a.json", "b.json", "--", "hello"])
            .unwrap();
        assert_eq!(cli.mcp_config.len(), 2);
        assert_eq!(cli.prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn cli_parses_strict_mcp_config() {
        let cli = Cli::try_parse_from(["crab", "--strict-mcp-config"]).unwrap();
        assert!(cli.strict_mcp_config);
    }

    #[test]
    fn cli_parses_settings_inline_json() {
        let cli =
            Cli::try_parse_from(["crab", "--settings", r#"{"model":"gpt-4o"}"#, "hello"]).unwrap();
        assert!(cli.settings.is_some());
    }

    #[test]
    fn cli_parses_include_flags() {
        let cli = Cli::try_parse_from([
            "crab",
            "--include-partial-messages",
            "--include-hook-events",
            "hello",
        ])
        .unwrap();
        assert!(cli.include_partial_messages);
        assert!(cli.include_hook_events);
    }

    // ─── load_settings_arg tests ───

    #[test]
    fn load_settings_arg_parses_inline_json() {
        let s = load_settings_arg(r#"{"model":"gpt-4o"}"#).unwrap();
        assert_eq!(s.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn load_settings_arg_rejects_invalid_json() {
        assert!(load_settings_arg("{invalid").is_err());
    }

    #[test]
    fn load_settings_arg_rejects_missing_file() {
        assert!(load_settings_arg("/nonexistent/settings.json").is_err());
    }

    // ─── load_mcp_configs tests ───

    #[test]
    fn load_mcp_configs_empty_paths() {
        let result = load_mcp_configs(&[]).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn load_mcp_configs_rejects_missing_file() {
        assert!(load_mcp_configs(&[PathBuf::from("/nonexistent.json")]).is_err());
    }

    // ─── event_to_json stream-json tests ───

    #[test]
    fn event_to_json_content_delta() {
        let event = Event::ContentDelta {
            index: 0,
            delta: "hello".into(),
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "content_delta");
        assert_eq!(json["delta"], "hello");
    }

    #[test]
    fn event_to_json_thinking_delta() {
        let event = Event::ThinkingDelta {
            index: 0,
            delta: "reasoning...".into(),
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "thinking_delta");
        assert_eq!(json["delta"], "reasoning...");
    }

    #[test]
    fn event_to_json_message_start() {
        let event = Event::MessageStart { id: "msg_1".into() };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "message_start");
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["id"], "msg_1");
    }

    #[test]
    fn event_to_json_message_end() {
        let event = Event::MessageEnd {
            usage: crab_core::model::TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_creation_tokens: 5,
            },
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "message_end");
        assert_eq!(json["usage"]["input_tokens"], 100);
        assert_eq!(json["usage"]["output_tokens"], 50);
    }

    #[test]
    fn event_to_json_tool_use_input() {
        let event = Event::ToolUseInput {
            id: "tu_1".into(),
            input: json!({"command": "ls"}),
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "tool_use_input");
        assert_eq!(json["input"]["command"], "ls");
    }

    #[test]
    fn event_to_json_all_variants_return_some() {
        // Ensure no variant returns None (exhaustive coverage)
        use crab_core::model::TokenUsage;
        use crab_core::tool::ToolOutput;

        let events = vec![
            Event::TurnStart { turn_index: 0 },
            Event::MessageStart { id: "m".into() },
            Event::ContentDelta {
                index: 0,
                delta: "d".into(),
            },
            Event::ThinkingDelta {
                index: 0,
                delta: "t".into(),
            },
            Event::ContentBlockStop { index: 0 },
            Event::MessageEnd {
                usage: TokenUsage::default(),
            },
            Event::ToolUseStart {
                id: "t".into(),
                name: "n".into(),
                input: Value::Null,
            },
            Event::ToolUseInput {
                id: "t".into(),
                input: json!({}),
            },
            Event::ToolOutputDelta {
                id: "t".into(),
                delta: "line".into(),
            },
            Event::ToolProgress {
                id: "t".into(),
                progress: crab_core::tool::ToolProgress {
                    elapsed_secs: 1.0,
                    total_lines: 10,
                    total_bytes: 256,
                    tail_output: String::new(),
                    timeout_secs: None,
                },
            },
            Event::ToolResult {
                id: "t".into(),
                output: ToolOutput::success("ok"),
            },
            Event::Error {
                message: "e".into(),
            },
            Event::TokenWarning {
                usage_pct: 0.5,
                used: 50,
                limit: 100,
            },
            Event::CompactStart {
                strategy: "s".into(),
                before_tokens: 0,
            },
            Event::CompactEnd {
                after_tokens: 0,
                removed_messages: 0,
            },
            Event::PermissionRequest {
                tool_name: "t".into(),
                input_summary: "s".into(),
                request_id: "r".into(),
            },
            Event::PermissionResponse {
                request_id: "r".into(),
                allowed: true,
            },
            Event::MemoryLoaded { count: 0 },
            Event::MemorySaved {
                filename: "f".into(),
            },
            Event::SessionSaved {
                session_id: "s".into(),
            },
            Event::SessionResumed {
                session_id: "s".into(),
                message_count: 0,
            },
            Event::AgentWorkerStarted {
                worker_id: "w".into(),
                task_prompt: "p".into(),
            },
            Event::AgentWorkerCompleted {
                worker_id: "w".into(),
                result: None,
                success: true,
                usage: TokenUsage::default(),
            },
        ];

        for event in &events {
            assert!(
                event_to_json(event).is_some(),
                "event_to_json returned None for {event:?}",
            );
        }
    }

    // ─── Tool filter CLI flag tests ───

    #[test]
    fn cli_parses_allowed_tools() {
        let cli =
            Cli::try_parse_from(["crab", "--allowed-tools", "Read,Write,Edit", "hello"]).unwrap();
        assert_eq!(cli.allowed_tools, vec!["Read", "Write", "Edit"]);
    }

    #[test]
    fn cli_parses_allowed_tools_camel_case_alias() {
        let cli = Cli::try_parse_from(["crab", "--allowedTools", "Bash,Read", "hello"]).unwrap();
        assert_eq!(cli.allowed_tools, vec!["Bash", "Read"]);
    }

    #[test]
    fn cli_parses_disallowed_tools() {
        let cli =
            Cli::try_parse_from(["crab", "--disallowed-tools", "Bash,mcp__*", "hello"]).unwrap();
        assert_eq!(cli.disallowed_tools, vec!["Bash", "mcp__*"]);
    }

    #[test]
    fn cli_parses_tools_flag() {
        let cli = Cli::try_parse_from(["crab", "--tools", "Read,Write", "hello"]).unwrap();
        assert_eq!(cli.tools.as_deref(), Some("Read,Write"));
    }

    #[test]
    fn cli_parses_tools_empty() {
        let cli = Cli::try_parse_from(["crab", "--tools", "", "hello"]).unwrap();
        assert_eq!(cli.tools.as_deref(), Some(""));
    }

    #[test]
    fn cli_parses_tools_default() {
        let cli = Cli::try_parse_from(["crab", "--tools", "default", "hello"]).unwrap();
        assert_eq!(cli.tools.as_deref(), Some("default"));
    }

    // ─── Effort / thinking CLI flag tests ───

    #[test]
    fn cli_parses_effort() {
        let cli = Cli::try_parse_from(["crab", "--effort", "high", "hello"]).unwrap();
        assert_eq!(cli.effort.as_deref(), Some("high"));
    }

    #[test]
    fn cli_parses_thinking() {
        let cli = Cli::try_parse_from(["crab", "--thinking", "enabled", "hello"]).unwrap();
        assert_eq!(cli.thinking.as_deref(), Some("enabled"));
    }

    // ─── resolve_tool_filters tests ───

    #[test]
    fn resolve_tool_filters_empty_inputs() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], None);
        assert!(allowed.is_empty());
        assert!(denied.is_empty());
    }

    #[test]
    fn resolve_tool_filters_passthrough_lists() {
        let (allowed, denied) =
            resolve_tool_filters(&["Read".into(), "Write".into()], &["Bash".into()], None);
        assert_eq!(allowed, vec!["Read", "Write"]);
        assert_eq!(denied, vec!["Bash"]);
    }

    #[test]
    fn resolve_tool_filters_tools_empty_disables_all() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], Some(""));
        assert!(allowed.is_empty());
        assert_eq!(denied, vec!["*"]);
    }

    #[test]
    fn resolve_tool_filters_tools_default_no_change() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], Some("default"));
        assert!(allowed.is_empty());
        assert!(denied.is_empty());
    }

    #[test]
    fn resolve_tool_filters_tools_explicit_list() {
        let (allowed, denied) = resolve_tool_filters(&[], &[], Some("Read,Write,Edit"));
        assert_eq!(allowed, vec!["Read", "Write", "Edit"]);
        assert!(denied.is_empty());
    }

    // ─── System prompt CLI flag tests ───

    #[test]
    fn cli_parses_system_prompt() {
        let cli =
            Cli::try_parse_from(["crab", "--system-prompt", "You are a pirate.", "hello"]).unwrap();
        assert_eq!(
            cli.system_prompt_override.as_deref(),
            Some("You are a pirate.")
        );
    }

    #[test]
    fn cli_parses_system_prompt_file() {
        let cli = Cli::try_parse_from(["crab", "--system-prompt-file", "/tmp/prompt.txt", "hello"])
            .unwrap();
        assert_eq!(
            cli.system_prompt_file,
            Some(PathBuf::from("/tmp/prompt.txt"))
        );
    }

    #[test]
    fn cli_parses_append_system_prompt() {
        let cli = Cli::try_parse_from([
            "crab",
            "--append-system-prompt",
            "Always be concise.",
            "hello",
        ])
        .unwrap();
        assert_eq!(
            cli.append_system_prompt.as_deref(),
            Some("Always be concise.")
        );
    }

    #[test]
    fn cli_parses_append_system_prompt_file() {
        let cli = Cli::try_parse_from([
            "crab",
            "--append-system-prompt-file",
            "/tmp/extra.txt",
            "hello",
        ])
        .unwrap();
        assert_eq!(
            cli.append_system_prompt_file,
            Some(PathBuf::from("/tmp/extra.txt"))
        );
    }

    // ─── --add-dir tests ───

    #[test]
    fn cli_parses_add_dir_single() {
        let cli = Cli::try_parse_from(["crab", "--add-dir", "/tmp/extra", "--", "hello"]).unwrap();
        assert_eq!(cli.add_dir, vec![PathBuf::from("/tmp/extra")]);
    }

    #[test]
    fn cli_parses_add_dir_multiple() {
        let cli =
            Cli::try_parse_from(["crab", "--add-dir", "/tmp/a", "/tmp/b", "--", "hello"]).unwrap();
        assert_eq!(
            cli.add_dir,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    // ─── --name tests ───

    #[test]
    fn cli_parses_name_short() {
        let cli = Cli::try_parse_from(["crab", "-n", "my-session"]).unwrap();
        assert_eq!(cli.name.as_deref(), Some("my-session"));
    }

    #[test]
    fn cli_parses_name_long() {
        let cli = Cli::try_parse_from(["crab", "--name", "feature-work"]).unwrap();
        assert_eq!(cli.name.as_deref(), Some("feature-work"));
    }

    // ─── --max-turns / --max-budget-usd / --fallback-model tests ───

    #[test]
    fn cli_parses_max_turns() {
        let cli = Cli::try_parse_from(["crab", "--max-turns", "10", "hello"]).unwrap();
        assert_eq!(cli.max_turns, Some(10));
    }

    #[test]
    fn cli_parses_max_budget_usd() {
        let cli = Cli::try_parse_from(["crab", "--max-budget-usd", "5.50", "hello"]).unwrap();
        assert!((cli.max_budget_usd.unwrap() - 5.50).abs() < f64::EPSILON);
    }

    #[test]
    fn cli_parses_fallback_model() {
        let cli = Cli::try_parse_from([
            "crab",
            "--fallback-model",
            "claude-haiku-4-5-20251001",
            "hello",
        ])
        .unwrap();
        assert_eq!(
            cli.fallback_model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
    }

    // ─── resolve_system_prompt tests ───

    #[test]
    fn resolve_system_prompt_default() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result =
            resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None).unwrap();
        assert!(result.contains("Crab Code"));
    }

    #[test]
    fn resolve_system_prompt_override() {
        let cli = Cli::try_parse_from(["crab", "--system-prompt", "Custom prompt only.", "hello"])
            .unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result =
            resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None).unwrap();
        assert_eq!(result, "Custom prompt only.");
    }

    #[test]
    fn resolve_system_prompt_append() {
        let cli = Cli::try_parse_from([
            "crab",
            "--append-system-prompt",
            "EXTRA INSTRUCTIONS",
            "hello",
        ])
        .unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result =
            resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None).unwrap();
        // Default prompt is still there
        assert!(result.contains("Crab Code"));
        // Append is at the end
        assert!(result.contains("EXTRA INSTRUCTIONS"));
    }

    #[test]
    fn resolve_system_prompt_override_plus_append() {
        let cli = Cli::try_parse_from([
            "crab",
            "--system-prompt",
            "Base.",
            "--append-system-prompt",
            "Extra.",
            "hello",
        ])
        .unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result =
            resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None).unwrap();
        assert!(result.starts_with("Base."));
        assert!(result.contains("Extra."));
        // Default prompt is NOT present since we overrode
        assert!(!result.contains("Crab Code"));
    }

    #[test]
    fn resolve_system_prompt_file_override() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("prompt.txt");
        std::fs::write(&file, "File-based prompt.").unwrap();

        let file_str = file.to_str().unwrap();
        let cli = Cli::try_parse_from(["crab", "--system-prompt-file", file_str, "hello"]).unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result =
            resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None).unwrap();
        assert_eq!(result, "File-based prompt.");
    }

    #[test]
    fn resolve_system_prompt_file_append() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("extra.txt");
        std::fs::write(&file, "APPENDED FROM FILE").unwrap();

        let file_str = file.to_str().unwrap();
        let cli = Cli::try_parse_from(["crab", "--append-system-prompt-file", file_str, "hello"])
            .unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result =
            resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None).unwrap();
        assert!(result.contains("Crab Code"));
        assert!(result.contains("APPENDED FROM FILE"));
    }

    #[test]
    fn resolve_system_prompt_missing_file_errors() {
        let cli = Cli::try_parse_from([
            "crab",
            "--system-prompt-file",
            "/nonexistent/prompt.txt",
            "hello",
        ])
        .unwrap();
        let registry = crab_tools::registry::ToolRegistry::new();
        let result = resolve_system_prompt(&cli, std::path::Path::new("."), &registry, None);
        assert!(result.is_err());
    }

    // ─── Subcommand CLI parsing tests ───

    #[test]
    fn cli_parses_update_check() {
        let cli = Cli::try_parse_from(["crab", "update", "check"]).unwrap();
        assert!(matches!(cli.command, Some(CliCommand::Update { .. })));
    }

    #[test]
    fn cli_parses_update_check_list() {
        let cli = Cli::try_parse_from(["crab", "update", "check", "--list"]).unwrap();
        match cli.command {
            Some(CliCommand::Update {
                action: Some(commands::update::UpdateAction::Check { list }),
            }) => {
                assert!(list);
            }
            _ => panic!("expected Update Check --list"),
        }
    }

    #[test]
    fn cli_parses_update_install_dry_run() {
        let cli = Cli::try_parse_from(["crab", "update", "install", "--dry-run", "1.0.0"]).unwrap();
        match cli.command {
            Some(CliCommand::Update {
                action:
                    Some(commands::update::UpdateAction::Install {
                        target,
                        dry_run,
                        force,
                    }),
            }) => {
                assert_eq!(target.as_deref(), Some("1.0.0"));
                assert!(dry_run);
                assert!(!force);
            }
            _ => panic!("expected Update Install"),
        }
    }

    #[test]
    fn cli_parses_update_rollback() {
        let cli = Cli::try_parse_from(["crab", "update", "rollback", "0.2.0"]).unwrap();
        match cli.command {
            Some(CliCommand::Update {
                action: Some(commands::update::UpdateAction::Rollback { target }),
            }) => {
                assert_eq!(target.as_deref(), Some("0.2.0"));
            }
            _ => panic!("expected Update Rollback"),
        }
    }

    #[test]
    fn cli_parses_update_default() {
        let cli = Cli::try_parse_from(["crab", "update"]).unwrap();
        match cli.command {
            Some(CliCommand::Update { action: None }) => {}
            _ => panic!("expected Update with no subcommand"),
        }
    }

    #[test]
    fn cli_parses_plugin_list() {
        let cli = Cli::try_parse_from(["crab", "plugin", "list"]).unwrap();
        assert!(matches!(cli.command, Some(CliCommand::Plugin { .. })));
    }

    #[test]
    fn cli_parses_plugin_install() {
        let cli = Cli::try_parse_from(["crab", "plugin", "install", "./my-plugin"]).unwrap();
        match cli.command {
            Some(CliCommand::Plugin {
                action: commands::plugin::PluginAction::Install { source },
            }) => {
                assert_eq!(source, "./my-plugin");
            }
            _ => panic!("expected Plugin Install"),
        }
    }

    #[test]
    fn cli_parses_plugin_remove() {
        let cli = Cli::try_parse_from(["crab", "plugin", "remove", "my-plugin"]).unwrap();
        match cli.command {
            Some(CliCommand::Plugin {
                action: commands::plugin::PluginAction::Remove { name },
            }) => {
                assert_eq!(name, "my-plugin");
            }
            _ => panic!("expected Plugin Remove"),
        }
    }

    #[test]
    fn cli_parses_plugin_enable() {
        let cli = Cli::try_parse_from(["crab", "plugin", "enable", "my-plugin"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(CliCommand::Plugin {
                action: commands::plugin::PluginAction::Enable { .. }
            })
        ));
    }

    #[test]
    fn cli_parses_plugin_disable() {
        let cli = Cli::try_parse_from(["crab", "plugin", "disable", "my-plugin"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(CliCommand::Plugin {
                action: commands::plugin::PluginAction::Disable { .. }
            })
        ));
    }

    #[test]
    fn cli_parses_plugin_validate() {
        let cli = Cli::try_parse_from(["crab", "plugin", "validate", "./path"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(CliCommand::Plugin {
                action: commands::plugin::PluginAction::Validate { .. }
            })
        ));
    }

    #[test]
    fn cli_parses_agents() {
        let cli = Cli::try_parse_from(["crab", "agents"]).unwrap();
        assert!(matches!(cli.command, Some(CliCommand::Agents)));
    }

    #[test]
    fn cli_parses_completion_bash() {
        let cli = Cli::try_parse_from(["crab", "completion", "bash"]).unwrap();
        match cli.command {
            Some(CliCommand::Completion { shell }) => {
                assert_eq!(shell, clap_complete::Shell::Bash);
            }
            _ => panic!("expected Completion"),
        }
    }

    #[test]
    fn cli_parses_completion_powershell() {
        let cli = Cli::try_parse_from(["crab", "completion", "powershell"]).unwrap();
        match cli.command {
            Some(CliCommand::Completion { shell }) => {
                assert_eq!(shell, clap_complete::Shell::PowerShell);
            }
            _ => panic!("expected Completion PowerShell"),
        }
    }

    // ─── Completion generation tests ───

    #[test]
    fn completion_generate_bash_does_not_panic() {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::Bash, &mut cmd, "crab", &mut buf);
        assert!(!buf.is_empty());
    }

    #[test]
    fn completion_generate_zsh_does_not_panic() {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::Zsh, &mut cmd, "crab", &mut buf);
        assert!(!buf.is_empty());
    }

    #[test]
    fn completion_generate_fish_does_not_panic() {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::Fish, &mut cmd, "crab", &mut buf);
        assert!(!buf.is_empty());
    }

    #[test]
    fn completion_generate_powershell_does_not_panic() {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::PowerShell, &mut cmd, "crab", &mut buf);
        assert!(!buf.is_empty());
    }

    // ─── B-level CLI flag tests (Steps 10–13) ───

    #[test]
    fn cli_parses_bare() {
        let cli = Cli::try_parse_from(["crab", "--bare", "hello"]).unwrap();
        assert!(cli.bare);
    }

    #[test]
    fn cli_bare_defaults_to_false() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(!cli.bare);
    }

    #[test]
    fn cli_parses_no_session_persistence() {
        let cli = Cli::try_parse_from(["crab", "--no-session-persistence", "hello"]).unwrap();
        assert!(cli.no_session_persistence);
    }

    #[test]
    fn cli_parses_worktree_without_value() {
        let cli = Cli::try_parse_from(["crab", "-w"]).unwrap();
        assert_eq!(cli.worktree.as_deref(), Some(""));
    }

    #[test]
    fn cli_parses_worktree_with_value() {
        let cli = Cli::try_parse_from(["crab", "--worktree", "feature-x"]).unwrap();
        assert_eq!(cli.worktree.as_deref(), Some("feature-x"));
    }

    #[test]
    fn cli_parses_tmux() {
        let cli = Cli::try_parse_from(["crab", "--tmux"]).unwrap();
        assert!(cli.tmux);
    }

    #[test]
    fn cli_parses_fork_session() {
        let cli = Cli::try_parse_from(["crab", "--fork-session"]).unwrap();
        assert!(cli.fork_session);
    }

    #[test]
    fn cli_parses_from_pr_without_value() {
        let cli = Cli::try_parse_from(["crab", "--from-pr"]).unwrap();
        assert_eq!(cli.from_pr.as_deref(), Some(""));
    }

    #[test]
    fn cli_parses_from_pr_with_value() {
        let cli = Cli::try_parse_from(["crab", "--from-pr", "123"]).unwrap();
        assert_eq!(cli.from_pr.as_deref(), Some("123"));
    }

    #[test]
    fn cli_parses_session_id() {
        let cli = Cli::try_parse_from(["crab", "--session-id", "abc-123"]).unwrap();
        assert_eq!(cli.session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn cli_parses_json_schema() {
        let cli = Cli::try_parse_from(["crab", "--json-schema", "schema.json", "hello"]).unwrap();
        assert_eq!(cli.json_schema.as_deref(), Some("schema.json"));
    }

    #[test]
    fn cli_parses_plugin_dir_single() {
        let cli =
            Cli::try_parse_from(["crab", "--plugin-dir", "/tmp/plugins", "--", "hello"]).unwrap();
        assert_eq!(cli.plugin_dir, vec![PathBuf::from("/tmp/plugins")]);
    }

    #[test]
    fn cli_parses_plugin_dir_multiple() {
        let cli = Cli::try_parse_from([
            "crab",
            "--plugin-dir",
            "/tmp/a",
            "--plugin-dir",
            "/tmp/b",
            "--",
            "hello",
        ])
        .unwrap();
        assert_eq!(
            cli.plugin_dir,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    #[test]
    fn cli_parses_disable_slash_commands() {
        let cli = Cli::try_parse_from(["crab", "--disable-slash-commands"]).unwrap();
        assert!(cli.disable_slash_commands);
    }

    #[test]
    fn cli_parses_betas() {
        let cli =
            Cli::try_parse_from(["crab", "--betas", "prompt-caching", "--", "hello"]).unwrap();
        assert_eq!(cli.betas, vec!["prompt-caching"]);
    }

    #[test]
    fn cli_parses_betas_multiple() {
        let cli = Cli::try_parse_from([
            "crab",
            "--betas",
            "prompt-caching",
            "computer-use",
            "--",
            "hello",
        ])
        .unwrap();
        assert_eq!(cli.betas, vec!["prompt-caching", "computer-use"]);
    }

    #[test]
    fn cli_parses_ide() {
        let cli = Cli::try_parse_from(["crab", "--ide"]).unwrap();
        assert!(cli.ide);
    }

    #[test]
    fn cli_ide_defaults_to_false() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(!cli.ide);
    }

    // ─── --setting-sources CLI flag tests ───

    #[test]
    fn cli_parses_setting_sources() {
        let cli =
            Cli::try_parse_from(["crab", "--setting-sources", "user,project", "hello"]).unwrap();
        assert_eq!(cli.setting_sources.as_deref(), Some("user,project"));
    }

    #[test]
    fn cli_setting_sources_default_is_none() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(cli.setting_sources.is_none());
    }
}
