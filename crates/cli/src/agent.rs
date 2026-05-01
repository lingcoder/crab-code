use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crab_agent::{AgentSession, SessionConfig, build_system_prompt};
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::PermissionHandler;

use crate::args::{Cli, OutputFormat};
#[cfg(feature = "tui")]
use crate::commands;
#[cfg(feature = "tui")]
use crate::output::print_exit_info;
use crate::output::{print_banner, print_events};

/// Read Coordinator Mode gate from env (no CLI flag by design — insiders opt
/// in via `CRAB_COORDINATOR_MODE=1`). Agent Teams base infrastructure is
/// unconditional; only Coordinator Mode (tool ACL + prompt overlay) is gated.
pub fn coordinator_mode_enabled() -> bool {
    coordinator_mode_from(|k| std::env::var(k).ok())
}

fn coordinator_mode_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    lookup("CRAB_COORDINATOR_MODE").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
}

/// Resolve well-known model aliases to their full model IDs.
/// Unknown strings are returned unchanged.
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
pub async fn run(cli: &Cli, resume_session_id: Option<String>) -> anyhow::Result<()> {
    // Initialise debug/tracing if requested
    let debug_filter = crab_common::utils::debug::resolve_debug_filter(cli.debug.as_deref());
    let debug_config = crab_common::utils::debug::DebugConfig {
        enabled: debug_filter.is_some() || cli.verbose,
        filter: debug_filter,
        file: cli.debug_file.clone(),
    };
    crab_common::utils::debug::init_debug(&debug_config);

    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Load merged settings with optional source control + validation
    let sources = cli
        .setting_sources
        .as_ref()
        .map(|s| crab_config::config::ConfigLayer::parse_list(s));
    let resolve_ctx = crab_config::ResolveContext::new()
        .with_process_env()
        .resolve_config_dir(cli.config_dir.as_deref())
        .with_project_dir(Some(working_dir.clone()))
        .with_cli_config_file(cli.config.clone())
        .with_cli_overrides(cli.config_override.clone())
        .with_sources_filter(sources);
    let mut settings = crab_config::resolve(&resolve_ctx)?;
    let validation_warnings = crab_config::validate_all_config_files(Some(&working_dir));
    for w in &validation_warnings {
        tracing::warn!("settings validation: {w}");
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
    let effective_settings = crab_config::Config {
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
    let skill_registry = crab_skills::SkillRegistry::discover(&skill_dirs).unwrap_or_default();
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

    let global_dir = crab_config::config::global_config_dir();
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
            let mut settings_warnings: Vec<String> = validation_warnings
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            if let Some(latest) = commands::update::startup_version_check() {
                settings_warnings.push(format!(
                    "Update available: v{latest} — run `crab update` to install"
                ));
            }
            let tui_config = crab_tui::TuiConfig {
                session_config,
                backend,
                skill_dirs,
                mcp_servers: settings.mcp_servers.clone(),
                settings_warnings,
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
        crab_config::config::global_config_dir().join("skills"),
        working_dir.join(".crab").join("skills"),
    ]
}

/// If input starts with `/`, try to match a skill command and return its content
/// as the prompt. Otherwise return the original input.
fn resolve_slash_command(input: &str, skill_registry: &crab_skills::SkillRegistry) -> String {
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
                use std::io::{BufRead, Write};
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
    skill_registry: &crab_skills::SkillRegistry,
) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let slash_registry = crab_commands::CommandRegistry::new();

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
    skill_registry: &crab_skills::SkillRegistry,
    command_registry: &crab_commands::CommandRegistry,
    cmd_rest: &str,
) -> SlashOutcome {
    let (name, args) = cmd_rest
        .split_once(char::is_whitespace)
        .map_or((cmd_rest, ""), |(n, a)| (n, a.trim()));

    if matches!(name, "exit" | "quit") {
        eprintln!("Goodbye!");
        return SlashOutcome::Exit;
    }

    let summary = session.cost.summary();
    let ctx = crab_commands::CommandContext {
        model: &session.config.model,
        session_id: &session.conversation.id,
        working_dir: &session.tool_ctx.working_dir,
        permission_mode: session.tool_ctx.permission_mode,
        cost: crab_commands::CostSnapshot {
            input_tokens: summary.input_tokens,
            output_tokens: summary.output_tokens,
            cache_read_tokens: summary.cache_read_tokens,
            cache_creation_tokens: summary.cache_creation_tokens,
            total_cost_usd: summary.total_cost_usd,
            api_calls: summary.api_calls,
        },
        estimated_tokens: 0,
        message_count: session.conversation.len(),
        memory_dir: session
            .memory_store
            .as_ref()
            .map(|_| std::path::Path::new(".crab/memory")),
    };

    let result = command_registry.execute(name, args, &ctx);

    match result {
        Some(crab_commands::CommandResult::Message(msg)) => {
            println!("{msg}");
            SlashOutcome::Continue
        }
        Some(crab_commands::CommandResult::Effect(effect)) => {
            handle_command_effect(session, effect).await
        }
        Some(crab_commands::CommandResult::Silent) => SlashOutcome::Continue,
        None => {
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

/// Apply a [`CommandEffect`] to the running session.
#[cfg(not(feature = "tui"))]
async fn handle_command_effect(
    session: &mut AgentSession,
    effect: crab_commands::CommandEffect,
) -> SlashOutcome {
    use crab_commands::CommandEffect;

    match effect {
        CommandEffect::Exit => {
            eprintln!("Goodbye!");
            SlashOutcome::Exit
        }
        CommandEffect::Clear => {
            session.conversation.clear();
            println!("[info] Conversation cleared. System prompt and cost accumulator retained.");
            SlashOutcome::Continue
        }
        CommandEffect::Compact => {
            let before = session.conversation.len();
            let summary = session.compact_conversation().await;
            let after = session.conversation.len();
            println!(
                "[info] Compacted {before} messages → {after} (extracted {} summary items).",
                summary.items.len()
            );
            SlashOutcome::Continue
        }
        CommandEffect::Rewind(target) => {
            let what = target.as_deref().unwrap_or("(most-recent edit)");
            println!(
                "[info] /rewind {what}: the `file_history` primitive is ready but \
                 Edit/Write/Notebook tools do not yet call track_edit via ToolContextExt. \
                 Wire-up lands in a follow-up."
            );
            SlashOutcome::Continue
        }
        other => {
            println!("[info] Command effect {other:?} is not yet wired in the REPL.");
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use crab_skills::{Skill, SkillRegistry, SkillTrigger};

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
}
