use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crab_agents::{AgentSession, SessionConfig, build_system_prompt};
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::{PermissionHandler, PermissionResult};

use crate::args::{Cli, OutputFormat};
#[cfg(feature = "tui")]
use crate::commands;
#[cfg(feature = "tui")]
use crate::output::print_exit_info;
use crate::output::{print_banner, print_events};

/// Read Coordinator Mode gate from env (no CLI flag by design -- insiders opt
/// in via `CRAB_COORDINATOR_MODE=1`). Agent Teams base infrastructure is
/// unconditional; only Coordinator Mode (tool ACL + prompt overlay) is gated.
pub fn coordinator_mode_enabled() -> bool {
    coordinator_mode_from(|k| std::env::var(k).ok())
}

fn coordinator_mode_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    lookup("CRAB_COORDINATOR_MODE").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
}

/// How to launch when no positional prompt and no `-p` were given.
///
/// The interactive TUI may only run when both stdout and stdin are real
/// terminals. A piped stdout/stdin forces the headless path, matching how
/// other agentic CLIs behave when their output is captured or their input
/// is fed from a pipe.
#[derive(Debug, PartialEq, Eq)]
enum LaunchMode {
    /// Both stdout and stdin are terminals -- start the interactive TUI.
    Interactive,
    /// stdin is piped -- read it as the headless prompt.
    HeadlessFromStdin,
    /// stdout is not a terminal but stdin is -- there is no prompt source.
    NoPromptSource,
}

fn launch_mode(stdout_is_tty: bool, stdin_is_tty: bool) -> LaunchMode {
    if stdout_is_tty && stdin_is_tty {
        LaunchMode::Interactive
    } else if !stdin_is_tty {
        LaunchMode::HeadlessFromStdin
    } else {
        LaunchMode::NoPromptSource
    }
}

/// Resolve the effective system prompt from CLI flags.
///
/// Priority:
/// 1. `--system-prompt` / `--system-prompt-file` -- replaces the default entirely.
/// 2. `--append-system-prompt` / `--append-system-prompt-file` -- appends to the default.
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
    let debug_filter = crab_utils::debug::resolve_debug_filter(cli.debug.as_deref());
    let debug_config = crab_utils::debug::DebugConfig {
        enabled: debug_filter.is_some() || cli.verbose,
        filter: debug_filter,
        file: cli.debug_file.clone(),
    };
    crab_utils::debug::init_debug(&debug_config);

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
        let mcp = crab_mcp::config::load_mcp_configs(&cli.mcp_config)?;
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

    // settings already has config.toml -> user -> project -> local -> env merged.
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
        .map(crab_config::model_alias::resolve_model_alias)
        .or_else(|| settings.model.clone())
        .unwrap_or_else(|| {
            if provider == "openai" || provider == "deepseek" || provider == "ollama" {
                "deepseek-chat".to_string()
            } else {
                "claude-sonnet-4-6".to_string()
            }
        });

    // Build effective settings -- just override provider and model from CLI
    let effective_settings = crab_config::Config {
        api_provider: Some(provider.clone()),
        model: Some(model_id.clone()),
        ..settings.clone()
    };

    let backend = Arc::new(crab_api::create_backend(&effective_settings));
    let mut registry = create_default_registry();

    // Only the headless/single-shot path connects MCP synchronously -- it needs
    // tools before the one turn runs. The interactive TUI connects MCP in the
    // background via AgentRuntime::init, so pre-connecting here would just block
    // startup and throw the result away.
    let want_headless = cli.prompt.is_some()
        || cli.print
        || !matches!(
            launch_mode(
                std::io::stdout().is_terminal(),
                std::io::stdin().is_terminal(),
            ),
            LaunchMode::Interactive,
        );

    // Connect to MCP servers and register their tools (headless only).
    let mcp_manager = if want_headless && let Some(ref mcp_value) = settings.mcp_servers {
        let mut mgr = crab_mcp::McpManager::new();
        let failed = mgr.start_all(mcp_value).await.unwrap_or_else(|e| {
            eprintln!("Warning: failed to parse MCP config: {e}");
            Vec::new()
        });
        for name in &failed {
            eprintln!("Warning: MCP server '{name}' failed to connect");
        }
        let mgr_handle = Arc::new(tokio::sync::Mutex::new(mgr));
        let mgr_ref = mgr_handle.lock().await;
        let count = crab_tools::builtin::mcp_tool::register_mcp_tools(
            &mgr_ref,
            &mut registry,
            Some(Arc::clone(&mgr_handle)),
        )
        .await;
        drop(mgr_ref);
        if count > 0 {
            eprintln!("Registered {count} MCP tool(s).");
        }
        Some(mgr_handle)
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
    let mut skill_registry = crab_skills::SkillRegistry::new();
    if !cli.bare && !cli.disable_slash_commands {
        skill_registry.register_all(crab_skills::builtin::builtin_skills());
    }
    let disk = crab_skills::SkillRegistry::discover(&skill_dirs).unwrap_or_default();
    for skill in disk.list() {
        skill_registry.register(skill.clone());
    }
    if let Some(ref mgr_handle) = mcp_manager {
        let mgr = mgr_handle.lock().await;
        let count =
            crab_agents::mcp_skills::register_mcp_prompt_skills(&mgr, &mut skill_registry).await;
        drop(mgr);
        if count > 0 {
            eprintln!("Registered {count} MCP prompt skill(s).");
        }
    }
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
    let permission_mode = crab_config::permission_mode::resolve_permission_mode(
        cli.permission_mode.as_deref(),
        cli.dangerously_skip_permissions,
        cli.auto,
        cli.trust_project,
        settings.permission_mode.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!(e))?;

    let global_dir = crab_config::config::global_config_dir();
    let sessions_dir = global_dir.join("sessions");

    // --no-session-persistence disables session saving
    let effective_sessions_dir = if cli.no_session_persistence || cli.bare {
        None
    } else {
        Some(sessions_dir.clone())
    };

    // Best-effort retention: prune session files older than the configured
    // window so the sessions directory does not grow unbounded. Skipped when
    // persistence is off or retention is unset/zero.
    if let Some(ref dir) = effective_sessions_dir
        && let Some(days) = settings.cleanup_period_days
        && days > 0
    {
        match crab_session::SessionHistory::new(dir.clone()).cleanup_older_than(days) {
            Ok(n) if n > 0 => {
                tracing::info!("pruned {n} session file(s) older than {days} days");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "session cleanup failed"),
        }
    }

    // A bare `--resume` (no id) opens the startup resume picker instead of
    // resuming a specific session.
    let open_resume_picker = resume_session_id.as_deref() == Some("");
    let resume_session_id = if open_resume_picker {
        None
    } else {
        resume_session_id
    };

    // Resolve resume ID: explicit --resume > -c (continue latest) > None
    let effective_resume_id = if resume_session_id.is_some() {
        resume_session_id
    } else {
        crab_session::resume::find_resume_session(&sessions_dir, cli.continue_session, &working_dir)
    };

    // --fork-session: when resuming, generate a new session ID (fork) instead of reusing
    // --session-id: override auto-generated session ID
    let session_id = if let Some(ref id) = cli.session_id {
        id.clone()
    } else if cli.fork_session && effective_resume_id.is_some() {
        crab_utils::id::new_ulid()
    } else {
        crab_utils::id::new_ulid()
    };

    // Build allowed/denied tool lists from CLI flags
    let (effective_allowed, effective_denied) =
        crab_core::permission::tool_filter::resolve_tool_filters(
            &cli.allowed_tools,
            &cli.disallowed_tools,
            cli.tools.as_deref(),
        );

    // Resolve effort level and thinking mode
    let effort = cli.effort.as_deref().map(str::to_lowercase);
    let thinking_mode = cli.thinking.as_deref().map(str::to_lowercase);

    // Validate effort if provided
    if let Some(ref e) = effort {
        crab_config::effort::validate_effort(e).map_err(|msg| anyhow::anyhow!(msg))?;
    }
    // Validate thinking if provided
    if let Some(ref t) = thinking_mode {
        crab_config::effort::validate_thinking(t).map_err(|msg| anyhow::anyhow!(msg))?;
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
        default_shell: settings.default_shell_kind().as_str().to_string(),
    };

    // A positional arg or `-p` keeps the existing behavior; otherwise TTY
    // status decides interactive-vs-headless (see `launch_mode`).
    let effective_prompt = if let Some(ref prompt) = cli.prompt {
        Some(prompt.clone())
    } else if cli.print {
        // -p without positional prompt: read from stdin
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Some(buf)
    } else {
        match launch_mode(
            std::io::stdout().is_terminal(),
            std::io::stdin().is_terminal(),
        ) {
            LaunchMode::Interactive => None,
            LaunchMode::HeadlessFromStdin => {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                Some(buf)
            }
            LaunchMode::NoPromptSource => {
                anyhow::bail!(
                    "No prompt provided and stdout is not a terminal. \
                     Pass a prompt or pipe input, e.g. `crab -p \"...\"` or `echo ... | crab`."
                );
            }
        }
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
                    "Update available: v{latest} -- run `crab update` to install"
                ));
            }
            let tui_config = crab_tui::TuiConfig {
                session_config,
                backend,
                skill_dirs,
                mcp_servers: settings.mcp_servers.clone(),
                settings_warnings,
                prefill: cli.prefill.clone(),
                open_resume_picker,
                prefers_reduced_motion: settings.prefers_reduced_motion.unwrap_or(false),
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
            crate::repl::run_repl(&mut session, &skill_registry).await
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
pub fn resolve_slash_command(input: &str, skill_registry: &crab_skills::SkillRegistry) -> String {
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

        eprintln!("[skill] Activated: {} -- {}", skill.name, skill.description);
        return prompt;
    }

    // No matching skill -- pass through as-is
    input.to_string()
}

/// CLI-based permission handler: prints prompt to stderr, reads y/n from stdin.
struct CliPermissionHandler;

/// Format a CLI permission prompt line, picking the best representation
/// for well-known tools so the user sees the actual command/path instead
/// of a generic summary.
fn format_cli_permission(tool_name: &str, prompt: &str, tool_input: &Value) -> String {
    let lower = tool_name.to_lowercase();
    match lower.as_str() {
        "bash" => {
            let command = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or(prompt);
            let description = tool_input
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            match description {
                Some(desc) => format!("[permission] Bash: {command}\n  ({desc})"),
                None => format!("[permission] Bash: {command}"),
            }
        }
        "edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or(prompt);
            format!("[permission] Edit: {path}")
        }
        "write" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or(prompt);
            format!("[permission] Write: {path}")
        }
        _ => format!("[permission] {prompt} ({tool_name})"),
    }
}

impl PermissionHandler for CliPermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
        tool_input: &Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionResult> + Send + '_>> {
        let line = format_cli_permission(tool_name, prompt, tool_input);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                use std::io::{BufRead, Write};
                eprint!("{line} [y/N] ");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                if std::io::stdin().lock().read_line(&mut line).is_ok() {
                    let answer = line.trim().to_lowercase();
                    let allowed = answer == "y" || answer == "yes";
                    PermissionResult {
                        allowed,
                        feedback: None,
                    }
                } else {
                    PermissionResult::deny()
                }
            })
            .await
            .unwrap_or_else(|_| PermissionResult::deny())
        })
    }
}

/// Run a single prompt, print the result, and exit.
async fn run_single_shot(
    session: &mut AgentSession,
    prompt: &str,
    output_format: OutputFormat,
) -> anyhow::Result<()> {
    let event_rx = crate::repl::take_event_rx(session);
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
    fn launch_mode_interactive_only_when_both_tty() {
        assert_eq!(launch_mode(true, true), LaunchMode::Interactive);
    }

    #[test]
    fn launch_mode_piped_stdout_has_no_prompt_source() {
        // `crab | cat`: stdout captured, keyboard still attached to stdin.
        assert_eq!(launch_mode(false, true), LaunchMode::NoPromptSource);
    }

    #[test]
    fn launch_mode_piped_stdin_reads_stdin() {
        // `echo hi | crab`: stdin is the prompt source.
        assert_eq!(launch_mode(true, false), LaunchMode::HeadlessFromStdin);
        // Both piped (e.g. in a script) also reads stdin.
        assert_eq!(launch_mode(false, false), LaunchMode::HeadlessFromStdin);
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
