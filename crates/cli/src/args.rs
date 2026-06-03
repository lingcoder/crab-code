use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::commands;

/// Output format for CLI results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
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
pub struct Cli {
    /// User prompt (if provided, runs single-shot mode then exits)
    pub prompt: Option<String>,

    /// LLM provider: "anthropic" (default) or "openai"
    #[arg(long, default_value = "anthropic")]
    pub provider: String,

    /// Model ID override (e.g. "claude-sonnet-4-6", "gpt-4o").
    /// Supports aliases: "sonnet", "opus", "haiku".
    #[arg(long, short)]
    pub model: Option<String>,

    /// Maximum output tokens
    #[arg(long, default_value = "4096")]
    pub max_tokens: u32,

    /// Trust in-project file operations (skip confirmation for project writes)
    #[arg(long, short = 't')]
    pub trust_project: bool,

    /// Skip ALL permission checks (dangerous!)
    #[arg(long)]
    pub dangerously_skip_permissions: bool,

    /// Auto mode: heuristic classifier auto-allows safe tools, prompts on
    /// risky ones, denies dangerous patterns.
    #[arg(long)]
    pub auto: bool,

    /// Run as an ACP (Agent Client Protocol) agent over stdio. The
    /// editor (Zed, Neovim, …) spawns crab as a child process and
    /// drives it via JSON-RPC; all other flags are ignored in this
    /// mode. See <https://agentclientprotocol.com>.
    #[arg(long)]
    pub acp: bool,

    /// Output format: text (human-readable), json (single JSON result),
    /// stream-json (NDJSON real-time stream).
    #[arg(long, value_enum, default_value = "text")]
    pub output_format: OutputFormat,

    /// Alias for --output-format json (backward compatible).
    #[arg(long)]
    pub json: bool,

    /// Include partial message chunks in stream-json output.
    #[arg(long)]
    pub include_partial_messages: bool,

    /// Include hook lifecycle events in stream-json output.
    #[arg(long)]
    pub include_hook_events: bool,

    /// Load MCP server configuration from JSON file(s).
    #[arg(long = "mcp-config", num_args = 1..)]
    pub mcp_config: Vec<PathBuf>,

    /// Ignore MCP servers from settings files, use only --mcp-config.
    #[arg(long)]
    pub strict_mcp_config: bool,

    /// Load a whole-file config overlay (TOML) from the given path.
    /// The file sits at the top of the file layer (above local).
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Override a single config field via dotted path. Repeatable.
    /// Example: `-c model=opus`, `-c permissions.allow='["Bash(git:*)"]'`.
    /// Values are parsed as TOML, falling back to a string.
    #[arg(short = 'c', long = "config-override", value_name = "KEY.PATH=VALUE")]
    pub config_override: Vec<String>,

    /// Override the user-level config directory (otherwise `CRAB_CONFIG_DIR`
    /// or `~/.crab/`). Useful for containers, integration tests, and
    /// multi-identity setups.
    #[arg(long = "config-dir", value_name = "DIR")]
    pub config_dir: Option<PathBuf>,

    /// Resume a previous session by ID
    #[arg(long)]
    pub resume: Option<String>,

    /// Pre-fill the interactive prompt input with text without submitting it.
    /// Ignored in print / single-shot mode.
    #[arg(long)]
    pub prefill: Option<String>,

    /// Print mode: run a single prompt and print the result (non-interactive).
    /// If no prompt is given, reads from stdin.
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Continue the most recent session for the current directory.
    #[arg(long = "continue")]
    pub continue_session: bool,

    /// Permission mode: "default", "acceptEdits", "dontAsk", "bypassPermissions", "plan",
    /// "trust-project", "dangerously".
    #[arg(long)]
    pub permission_mode: Option<String>,

    /// Enable debug logging. Optionally specify a filter (e.g. -d api).
    /// Use without a value for global debug output.
    #[arg(short = 'd', long, num_args = 0..=1, default_missing_value = "")]
    pub debug: Option<String>,

    /// Write debug logs to a file (in addition to stderr).
    #[arg(long)]
    pub debug_file: Option<PathBuf>,

    /// Enable verbose output.
    #[arg(long)]
    pub verbose: bool,

    /// Allowed tools (comma-separated). Supports glob patterns like `Bash(git:*)`.
    #[arg(long = "allowed-tools", alias = "allowedTools", value_delimiter = ',')]
    pub allowed_tools: Vec<String>,

    /// Disallowed tools (comma-separated). Supports glob patterns like `mcp__*`.
    #[arg(
        long = "disallowed-tools",
        alias = "disallowedTools",
        value_delimiter = ','
    )]
    pub disallowed_tools: Vec<String>,

    /// Available tool set: "" (disable all), "default" (all), or comma-separated names.
    #[arg(long)]
    pub tools: Option<String>,

    /// Effort level for reasoning: low, medium, high, max.
    #[arg(long)]
    pub effort: Option<String>,

    /// Extended thinking mode: enabled, adaptive, disabled.
    #[arg(long)]
    pub thinking: Option<String>,

    /// Override the default system prompt entirely.
    #[arg(long = "system-prompt")]
    pub system_prompt_override: Option<String>,

    /// Override the default system prompt from a file.
    #[arg(long = "system-prompt-file")]
    pub system_prompt_file: Option<PathBuf>,

    /// Append text to the default system prompt.
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Option<String>,

    /// Append text from a file to the default system prompt.
    #[arg(long = "append-system-prompt-file")]
    pub append_system_prompt_file: Option<PathBuf>,

    /// Additional directories the agent may access (repeatable).
    #[arg(long = "add-dir", num_args = 1..)]
    pub add_dir: Vec<PathBuf>,

    /// Session display name (shown in /resume list and terminal title).
    #[arg(short = 'n', long)]
    pub name: Option<String>,

    /// Maximum agent turns in print mode.
    #[arg(long = "max-turns")]
    pub max_turns: Option<u32>,

    /// Maximum spend in USD in print mode.
    #[arg(long = "max-budget-usd")]
    pub max_budget_usd: Option<f64>,

    /// Fallback model to use when the primary model is overloaded.
    #[arg(long = "fallback-model")]
    pub fallback_model: Option<String>,

    // ─── Step 10: bare + no-session-persistence ───
    /// Minimal mode — skip hooks, LSP, plugins, auto-memory, AGENTS.md discovery.
    #[arg(long)]
    pub bare: bool,

    /// Disable session persistence (useful in print mode).
    #[arg(long)]
    pub no_session_persistence: bool,

    // ─── Step 11: worktree + tmux ───
    /// Create a git worktree. Optionally provide a branch name.
    #[arg(short = 'w', long, num_args = 0..=1, default_missing_value = "")]
    pub worktree: Option<String>,

    /// Open the worktree in a tmux session (requires --worktree).
    #[arg(long)]
    pub tmux: bool,

    // ─── Step 12: fork-session + from-pr + session-id + json-schema ───
    /// When resuming, fork into a new session instead of continuing the old one.
    #[arg(long)]
    pub fork_session: bool,

    /// Load context from a GitHub PR (number or URL). Optionally provide the value.
    #[arg(long = "from-pr", num_args = 0..=1, default_missing_value = "")]
    pub from_pr: Option<String>,

    /// Use a custom session UUID instead of auto-generating one.
    #[arg(long = "session-id")]
    pub session_id: Option<String>,

    /// Validate the final output against a JSON Schema (path or inline JSON).
    #[arg(long = "json-schema")]
    pub json_schema: Option<String>,

    // ─── Step 13: plugin-dir + disable-slash-commands + betas + ide ───
    /// Additional plugin directories to load at runtime (repeatable).
    #[arg(long = "plugin-dir")]
    pub plugin_dir: Vec<PathBuf>,

    /// Disable all slash commands / skills.
    #[arg(long)]
    pub disable_slash_commands: bool,

    /// API beta headers to send (repeatable).
    #[arg(long, num_args = 1..)]
    pub betas: Vec<String>,

    /// Connect to IDE extension automatically.
    #[arg(long)]
    pub ide: bool,

    /// Control which settings sources to load (comma-separated: user,project,local).
    /// Default: all sources. Example: --setting-sources user,project
    #[arg(long = "setting-sources")]
    pub setting_sources: Option<String>,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// Subcommands for `crab`.
#[derive(Subcommand)]
pub enum CliCommand {
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
    /// Inspect the permission rule store and audit log
    Permissions {
        #[command(subcommand)]
        action: commands::permissions::PermissionsAction,
    },
    /// Generate shell completion scripts
    Completion {
        /// Shell to generate completions for
        shell: crate::completions::Shell,
    },
}

/// Session management actions.
#[derive(Subcommand)]
pub enum SessionAction {
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
    pub fn effective_output_format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output_format
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
        let cli = Cli::try_parse_from(["crab", "--continue"]).unwrap();
        assert!(cli.continue_session);
    }

    #[test]
    fn cli_parses_permission_mode() {
        let cli =
            Cli::try_parse_from(["crab", "--permission-mode", "acceptEdits", "hello"]).unwrap();
        assert_eq!(cli.permission_mode.as_deref(), Some("acceptEdits"));
    }

    #[test]
    fn cli_parses_auto_flag() {
        let cli = Cli::try_parse_from(["crab", "--auto", "hello"]).unwrap();
        assert!(cli.auto);
    }

    #[test]
    fn cli_auto_defaults_to_false() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(!cli.auto);
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

    // ─── MCP config / config CLI arg tests ───

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
    fn cli_parses_config_file() {
        let cli = Cli::try_parse_from(["crab", "--config", "/tmp/my.toml", "hello"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/my.toml"))
        );
    }

    #[test]
    fn cli_parses_config_dir_flag() {
        let cli = Cli::try_parse_from(["crab", "--config-dir", "/tmp/cfg-dir", "hello"]).unwrap();
        assert_eq!(
            cli.config_dir.as_deref(),
            Some(std::path::Path::new("/tmp/cfg-dir"))
        );
    }

    #[test]
    fn cli_rejects_legacy_settings_flag() {
        // --settings was removed in favor of --config (file overlay) and the
        // -c override flag. clap surfaces the unknown argument instead of
        // silently dropping it.
        let result = Cli::try_parse_from(["crab", "--settings", "{}", "hello"]);
        let Err(err) = result else {
            panic!("expected legacy --settings flag to be rejected");
        };
        assert!(
            err.kind() == clap::error::ErrorKind::UnknownArgument,
            "expected UnknownArgument, got {:?}",
            err.kind(),
        );
    }

    #[test]
    fn cli_parses_config_override_short() {
        let cli = Cli::try_parse_from(["crab", "-c", "model=opus", "hello"]).unwrap();
        assert_eq!(cli.config_override, vec!["model=opus".to_string()]);
    }

    #[test]
    fn cli_parses_config_override_long_repeatable() {
        let cli = Cli::try_parse_from([
            "crab",
            "--config-override",
            "model=opus",
            "-c",
            "permissions.defaultMode=plan",
            "hello",
        ])
        .unwrap();
        assert_eq!(
            cli.config_override,
            vec![
                "model=opus".to_string(),
                "permissions.defaultMode=plan".to_string(),
            ],
        );
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
                assert_eq!(shell, crate::completions::Shell::Bash);
            }
            _ => panic!("expected Completion"),
        }
    }

    #[test]
    fn cli_parses_completion_powershell() {
        let cli = Cli::try_parse_from(["crab", "completion", "powershell"]).unwrap();
        match cli.command {
            Some(CliCommand::Completion { shell }) => {
                assert_eq!(shell, crate::completions::Shell::PowerShell);
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
        crate::completions::generate_completions(
            crate::completions::Shell::PowerShell,
            &mut cmd,
            &mut buf,
        )
        .unwrap();
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

    #[test]
    fn cli_parses_prefill() {
        let cli = Cli::try_parse_from(["crab", "--prefill", "draft text"]).unwrap();
        assert_eq!(cli.prefill.as_deref(), Some("draft text"));
    }

    #[test]
    fn cli_prefill_default_is_none() {
        let cli = Cli::try_parse_from(["crab", "hello"]).unwrap();
        assert!(cli.prefill.is_none());
    }
}
