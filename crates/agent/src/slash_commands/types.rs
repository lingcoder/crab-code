//! Slash command framework and built-in commands.
//!
//! Provides a registry of `/command` handlers that can be executed
//! from the REPL or TUI. Commands receive a context struct with
//! references to session state and return a result indicating
//! what action (if any) the caller should take.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crab_core::model::ModelId;
use crab_core::permission::PermissionMode;
use crab_session::CostAccumulator;

/// Context passed to slash commands, providing read access to session state.
pub struct SlashCommandContext<'a> {
    /// Current model ID.
    pub model: &'a ModelId,
    /// Current session ID.
    pub session_id: &'a str,
    /// Working directory.
    pub working_dir: &'a Path,
    /// Permission mode.
    pub permission_mode: PermissionMode,
    /// Cost accumulator (read-only snapshot).
    pub cost: &'a CostAccumulator,
    /// Estimated token count in conversation.
    pub estimated_tokens: u64,
    /// Number of messages in conversation.
    pub message_count: usize,
    /// Memory directory (if configured).
    pub memory_dir: Option<&'a Path>,
}

/// The result of executing a slash command.
#[derive(Debug, Clone)]
pub enum SlashCommandResult {
    /// Display a message to the user.
    Message(String),
    /// Trigger an action in the session/REPL.
    Action(SlashAction),
    /// Command executed silently (no output, no action).
    Silent,
}

/// Actions that a slash command can request the caller to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashAction {
    /// Clear conversation history.
    Clear,
    /// Trigger context compaction.
    Compact,
    /// Exit the session.
    Exit,
    /// Switch to a different model.
    SwitchModel(String),
    /// Toggle plan mode.
    TogglePlanMode,
    /// Generate a AGENTS.md template in the working directory.
    Init,
    /// Export conversation to a file.
    Export(String),
    /// Set effort level (low/medium/high/max).
    SetEffort(String),
    /// Toggle fast mode.
    ToggleFast,
    /// Add an additional working directory.
    AddDir(PathBuf),
    /// Resume a previous session by ID.
    Resume(String),
    /// Copy last assistant message to clipboard.
    CopyLast,
    /// Rewind the most recent file edit, or all edits if `None`.
    Rewind(Option<String>),
}

/// A registered slash command.
struct CommandEntry {
    name: &'static str,
    description: &'static str,
    handler: fn(args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult,
}

/// Registry of all available slash commands.
pub struct SlashCommandRegistry {
    commands: HashMap<&'static str, CommandEntry>,
    /// Insertion-ordered list of command names for /help display.
    order: Vec<&'static str>,
}

impl SlashCommandRegistry {
    /// Create a new registry with all built-in commands pre-registered.
    #[must_use]
    pub fn new() -> Self {
        use super::handlers::{
            cmd_add_dir, cmd_branch, cmd_clear, cmd_commit, cmd_compact, cmd_config, cmd_copy,
            cmd_cost, cmd_diff, cmd_doctor, cmd_effort, cmd_exit, cmd_export, cmd_fast, cmd_files,
            cmd_help, cmd_history, cmd_init, cmd_keybindings, cmd_mcp, cmd_memory, cmd_model,
            cmd_permissions, cmd_plan, cmd_plugin, cmd_rename, cmd_resume, cmd_review, cmd_rewind,
            cmd_skills, cmd_status, cmd_theme, cmd_thinking,
        };

        let mut reg = Self {
            commands: HashMap::new(),
            order: Vec::new(),
        };
        reg.register("help", "List all available commands", cmd_help);
        reg.register("clear", "Clear conversation history", cmd_clear);
        reg.register("compact", "Trigger context compaction", cmd_compact);
        reg.register("cost", "Show token usage and cost", cmd_cost);
        reg.register(
            "status",
            "Show session status (model, tokens, dir)",
            cmd_status,
        );
        reg.register("memory", "List memory files", cmd_memory);
        reg.register(
            "init",
            "Generate a AGENTS.md template in current directory",
            cmd_init,
        );
        reg.register("model", "Switch model (/model <name-or-alias>)", cmd_model);
        reg.register("config", "Show current configuration values", cmd_config);
        reg.register(
            "permissions",
            "Show current permission mode",
            cmd_permissions,
        );
        reg.register("exit", "Exit the session", cmd_exit);
        reg.register("plan", "Toggle plan mode", cmd_plan);
        // ─── Batch 2 ───
        reg.register(
            "resume",
            "Resume a previous session (/resume [id])",
            cmd_resume,
        );
        reg.register("history", "List recent sessions", cmd_history);
        reg.register("export", "Export conversation (/export [path])", cmd_export);
        reg.register("doctor", "Run health diagnostics", cmd_doctor);
        reg.register("diff", "Show git diff summary", cmd_diff);
        reg.register("review", "Show pending review items", cmd_review);
        reg.register(
            "effort",
            "Set effort level (/effort low|medium|high|max)",
            cmd_effort,
        );
        reg.register("fast", "Toggle fast mode", cmd_fast);
        reg.register(
            "thinking",
            "Show current thinking/effort settings",
            cmd_thinking,
        );
        reg.register("skills", "List available skills", cmd_skills);
        // ─── Batch 3 ───
        reg.register(
            "add-dir",
            "Add working directory (/add-dir <path>)",
            cmd_add_dir,
        );
        reg.register(
            "files",
            "List tracked files in working directory",
            cmd_files,
        );
        reg.register("plugin", "List loaded plugins", cmd_plugin);
        reg.register("mcp", "List MCP server connections", cmd_mcp);
        reg.register("branch", "Show current git branch", cmd_branch);
        reg.register("commit", "Show recent git commits", cmd_commit);
        reg.register("theme", "Show current theme", cmd_theme);
        reg.register("keybindings", "Show key bindings", cmd_keybindings);
        reg.register(
            "rename",
            "Rename current session (/rename <name>)",
            cmd_rename,
        );
        reg.register("copy", "Copy last assistant message", cmd_copy);
        reg.register(
            "rewind",
            "Rewind the latest file edit (/rewind [path])",
            cmd_rewind,
        );
        reg
    }

    fn register(
        &mut self,
        name: &'static str,
        description: &'static str,
        handler: fn(&str, &SlashCommandContext<'_>) -> SlashCommandResult,
    ) {
        self.commands.insert(
            name,
            CommandEntry {
                name,
                description,
                handler,
            },
        );
        self.order.push(name);
    }

    /// Execute a slash command by name.
    ///
    /// Returns `None` if the command is not found.
    pub fn execute(
        &self,
        name: &str,
        args: &str,
        ctx: &SlashCommandContext<'_>,
    ) -> Option<SlashCommandResult> {
        self.commands
            .get(name)
            .map(|entry| (entry.handler)(args, ctx))
    }

    /// Look up a command by name.
    pub fn find(&self, name: &str) -> Option<(&str, &str)> {
        self.commands.get(name).map(|e| (e.name, e.description))
    }

    /// List all commands in registration order as `(name, description)` pairs.
    pub fn list(&self) -> Vec<(&str, &str)> {
        self.order
            .iter()
            .filter_map(|name| self.commands.get(name).map(|e| (e.name, e.description)))
            .collect()
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Default for SlashCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}
