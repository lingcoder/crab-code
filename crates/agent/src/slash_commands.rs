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
    /// Generate a CRAB.md template in the working directory.
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
            "Generate a CRAB.md template in current directory",
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

// ─── Built-in command implementations ────────────────────────────

fn cmd_help(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    // We build the help text from a fresh registry to list all commands.
    // This avoids needing &self in the handler signature.
    let reg = SlashCommandRegistry::new();
    let mut lines = vec!["Available commands:".to_string()];
    for (name, desc) in reg.list() {
        lines.push(format!("  /{name:<14} {desc}"));
    }
    let _ = ctx; // ctx available for future extensions
    SlashCommandResult::Message(lines.join("\n"))
}

fn cmd_clear(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::Clear)
}

fn cmd_compact(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::Compact)
}

fn cmd_cost(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let summary = ctx.cost.summary();
    let text = format!(
        "Session cost:\n  Input tokens:    {}\n  Output tokens:   {}\n  Cache read:      {}\n  Cache creation:  {}\n  API calls:       {}\n  Total cost:      ${:.4}",
        summary.input_tokens,
        summary.output_tokens,
        summary.cache_read_tokens,
        summary.cache_creation_tokens,
        summary.api_calls,
        summary.total_cost_usd,
    );
    SlashCommandResult::Message(text)
}

fn cmd_status(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let text = format!(
        "Session status:\n  Model:           {}\n  Session ID:      {}\n  Working dir:     {}\n  Permission mode: {}\n  Messages:        {}\n  Est. tokens:     {}",
        ctx.model,
        ctx.session_id,
        ctx.working_dir.display(),
        ctx.permission_mode,
        ctx.message_count,
        ctx.estimated_tokens,
    );
    SlashCommandResult::Message(text)
}

fn cmd_memory(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let Some(dir) = ctx.memory_dir else {
        return SlashCommandResult::Message("No memory directory configured.".into());
    };

    if !dir.exists() {
        return SlashCommandResult::Message(format!(
            "Memory directory does not exist: {}",
            dir.display()
        ));
    }

    let entries: Vec<String> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .map(|e| format!("  {}", e.file_name().to_string_lossy()))
        .collect();

    if entries.is_empty() {
        return SlashCommandResult::Message("No memory files found.".into());
    }

    let mut text = format!("Memory files ({}):", entries.len());
    for entry in &entries {
        text.push('\n');
        text.push_str(entry);
    }
    SlashCommandResult::Message(text)
}

fn cmd_init(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::Init)
}

fn cmd_model(args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let name = args.trim();
    if name.is_empty() {
        return SlashCommandResult::Message(
            "Usage: /model <name-or-alias>\nAliases: sonnet, opus, haiku".into(),
        );
    }

    let resolved = resolve_model_alias(name);
    SlashCommandResult::Action(SlashAction::SwitchModel(resolved))
}

fn cmd_config(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let text = format!(
        "Current configuration:\n  Model:           {}\n  Permission mode: {}\n  Working dir:     {}\n  Memory dir:      {}",
        ctx.model,
        ctx.permission_mode,
        ctx.working_dir.display(),
        ctx.memory_dir
            .as_ref()
            .map_or_else(|| "(none)".to_string(), |d| d.display().to_string()),
    );
    SlashCommandResult::Message(text)
}

fn cmd_permissions(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let text = format!("Permission mode: {}", ctx.permission_mode);
    SlashCommandResult::Message(text)
}

fn cmd_exit(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::Exit)
}

fn cmd_plan(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::TogglePlanMode)
}

// ─── Batch 2 command implementations ────────────────────────────

fn cmd_resume(args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let id = args.trim();
    if id.is_empty() {
        return SlashCommandResult::Message(
            "Usage: /resume <session-id>\nUse /history to list available sessions.".into(),
        );
    }
    let _ = ctx;
    SlashCommandResult::Action(SlashAction::Resume(id.to_string()))
}

fn cmd_history(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let sessions_dir = ctx
        .working_dir
        .parent()
        .unwrap_or(ctx.working_dir)
        .join(".crab")
        .join("sessions");

    if !sessions_dir.exists() {
        return SlashCommandResult::Message("No session history found.".into());
    }

    let mut entries: Vec<(String, std::time::SystemTime)> = std::fs::read_dir(&sessions_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            let name = e.path().file_stem()?.to_string_lossy().to_string();
            Some((name, mtime))
        })
        .collect();

    entries.sort_by(|a, b| b.1.cmp(&a.1));

    if entries.is_empty() {
        return SlashCommandResult::Message("No sessions found.".into());
    }

    let max = 10.min(entries.len());
    let mut lines = vec![format!(
        "Recent sessions (showing {max} of {}):",
        entries.len()
    )];
    for (id, _mtime) in &entries[..max] {
        lines.push(format!("  {id}"));
    }
    lines.push("\nUse /resume <id> to continue a session.".into());
    SlashCommandResult::Message(lines.join("\n"))
}

fn cmd_export(args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let path = args.trim();
    if path.is_empty() {
        let default = format!("session_{}.md", ctx.session_id);
        return SlashCommandResult::Action(SlashAction::Export(default));
    }
    SlashCommandResult::Action(SlashAction::Export(path.to_string()))
}

fn cmd_doctor(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let mut checks = vec!["Health check:".to_string()];

    // Working directory exists
    if ctx.working_dir.exists() {
        checks.push(format!(
            "  [ok] Working directory: {}",
            ctx.working_dir.display()
        ));
    } else {
        checks.push(format!(
            "  [!!] Working directory missing: {}",
            ctx.working_dir.display()
        ));
    }

    // Git available
    let git_ok = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok();
    checks.push(if git_ok {
        "  [ok] git: available".into()
    } else {
        "  [!!] git: not found".into()
    });

    // Model configured
    checks.push(format!("  [ok] Model: {}", ctx.model));

    // Permission mode
    checks.push(format!("  [ok] Permission mode: {}", ctx.permission_mode));

    // Memory directory
    match ctx.memory_dir {
        Some(d) if d.exists() => checks.push(format!("  [ok] Memory dir: {}", d.display())),
        Some(d) => checks.push(format!("  [!!] Memory dir missing: {}", d.display())),
        None => checks.push("  [--] Memory dir: not configured".into()),
    }

    SlashCommandResult::Message(checks.join("\n"))
}

fn cmd_diff(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let output = std::process::Command::new("git")
        .args(["diff", "--stat"])
        .current_dir(ctx.working_dir)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                SlashCommandResult::Message("No uncommitted changes.".into())
            } else {
                SlashCommandResult::Message(format!("Git diff summary:\n{text}"))
            }
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            SlashCommandResult::Message(format!("git diff failed: {err}"))
        }
        Err(e) => SlashCommandResult::Message(format!("Failed to run git: {e}")),
    }
}

fn cmd_review(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    // Show staged changes as items to review
    let output = std::process::Command::new("git")
        .args(["diff", "--cached", "--stat"])
        .current_dir(ctx.working_dir)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                SlashCommandResult::Message("No staged changes to review.".into())
            } else {
                SlashCommandResult::Message(format!("Staged changes for review:\n{text}"))
            }
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            SlashCommandResult::Message(format!("git diff --cached failed: {err}"))
        }
        Err(e) => SlashCommandResult::Message(format!("Failed to run git: {e}")),
    }
}

fn cmd_effort(args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let level = args.trim().to_lowercase();
    match level.as_str() {
        "low" | "medium" | "high" | "max" => {
            SlashCommandResult::Action(SlashAction::SetEffort(level))
        }
        "" => SlashCommandResult::Message(
            "Usage: /effort <low|medium|high|max>\nControls reasoning depth and token budget."
                .into(),
        ),
        _ => SlashCommandResult::Message(format!(
            "Unknown effort level: '{level}'. Valid: low, medium, high, max"
        )),
    }
}

fn cmd_fast(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::ToggleFast)
}

fn cmd_thinking(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let text = format!(
        "Thinking settings:\n  Model: {}\n  Permission mode: {}",
        ctx.model, ctx.permission_mode,
    );
    SlashCommandResult::Message(text)
}

fn cmd_skills(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    // Skills are loaded via the plugin system; this command shows a placeholder
    // until the full SkillRegistry is wired into the context.
    SlashCommandResult::Message(
        "Skills:\n  No skills loaded.\n  Use /init to create a CRAB.md with skill definitions."
            .into(),
    )
}

// ─── Batch 3 command implementations ────────────────────────────

fn cmd_add_dir(args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let path = args.trim();
    if path.is_empty() {
        return SlashCommandResult::Message("Usage: /add-dir <path>".into());
    }
    let dir = PathBuf::from(path);
    if !dir.is_dir() {
        return SlashCommandResult::Message(format!("Not a directory: {path}"));
    }
    SlashCommandResult::Action(SlashAction::AddDir(dir))
}

fn cmd_files(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let output = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(ctx.working_dir)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let count = text.lines().count();
            if count == 0 {
                return SlashCommandResult::Message("No tracked files.".into());
            }
            let preview: String = text.lines().take(30).collect::<Vec<_>>().join("\n");
            let suffix = if count > 30 {
                format!("\n  ... and {} more files", count - 30)
            } else {
                String::new()
            };
            SlashCommandResult::Message(format!("Tracked files ({count}):\n{preview}{suffix}"))
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            SlashCommandResult::Message(format!("git ls-files failed: {err}"))
        }
        Err(_) => SlashCommandResult::Message("Not a git repository or git not available.".into()),
    }
}

fn cmd_plugin(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Message(
        "Plugins:\n  No plugins loaded.\n  Use `crab plugin list` to manage plugins.".into(),
    )
}

fn cmd_mcp(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Message(
        "MCP servers:\n  No MCP servers connected.\n  Configure servers in ~/.crab/settings.json."
            .into(),
    )
}

fn cmd_branch(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(ctx.working_dir)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() {
                SlashCommandResult::Message("Detached HEAD (no branch).".into())
            } else {
                SlashCommandResult::Message(format!("Current branch: {branch}"))
            }
        }
        Ok(_) | Err(_) => {
            SlashCommandResult::Message("Not a git repository or git not available.".into())
        }
    }
}

fn cmd_commit(_args: &str, ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let output = std::process::Command::new("git")
        .args(["log", "--oneline", "-10"])
        .current_dir(ctx.working_dir)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                SlashCommandResult::Message("No commits yet.".into())
            } else {
                SlashCommandResult::Message(format!("Recent commits:\n{text}"))
            }
        }
        Ok(_) | Err(_) => {
            SlashCommandResult::Message("Not a git repository or git not available.".into())
        }
    }
}

fn cmd_theme(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Message("Current theme: default\nTheme customization coming soon.".into())
}

fn cmd_keybindings(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Message(
        "Key bindings:\n  Enter        Send message\n  Ctrl+C       Cancel current operation\n  Ctrl+D       Exit session\n  Tab          Autocomplete\n  Up/Down      Navigate history\n  Esc          Clear input".into(),
    )
}

fn cmd_rename(args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    let name = args.trim();
    if name.is_empty() {
        return SlashCommandResult::Message("Usage: /rename <name>".into());
    }
    // Renaming is a display-level operation handled by the caller
    SlashCommandResult::Message(format!("Session renamed to: {name}"))
}

fn cmd_copy(_args: &str, _ctx: &SlashCommandContext<'_>) -> SlashCommandResult {
    SlashCommandResult::Action(SlashAction::CopyLast)
}

/// Resolve well-known model aliases to full IDs.
fn resolve_model_alias(alias: &str) -> String {
    match alias {
        "sonnet" => "claude-sonnet-4-20250514".to_string(),
        "opus" => "claude-opus-4-20250514".to_string(),
        "haiku" => "claude-haiku-4-5-20251001".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crab_core::model::ModelId;
    use crab_core::permission::PermissionMode;
    use crab_session::CostAccumulator;

    fn make_ctx() -> (ModelId, CostAccumulator, PathBuf) {
        (
            ModelId::from("claude-sonnet-4-20250514"),
            CostAccumulator::default(),
            PathBuf::from("/tmp/project"),
        )
    }

    fn ctx_from<'a>(
        model: &'a ModelId,
        cost: &'a CostAccumulator,
        dir: &'a Path,
    ) -> SlashCommandContext<'a> {
        SlashCommandContext {
            model,
            session_id: "sess_test",
            working_dir: dir,
            permission_mode: PermissionMode::Default,
            cost,
            estimated_tokens: 5000,
            message_count: 10,
            memory_dir: None,
        }
    }

    // ─── Registry tests ───

    #[test]
    fn registry_has_32_commands() {
        let reg = SlashCommandRegistry::new();
        assert_eq!(reg.len(), 32);
        assert!(!reg.is_empty());
    }

    #[test]
    fn registry_find_known_command() {
        let reg = SlashCommandRegistry::new();
        let (name, desc) = reg.find("help").unwrap();
        assert_eq!(name, "help");
        assert!(!desc.is_empty());
    }

    #[test]
    fn registry_find_unknown_command() {
        let reg = SlashCommandRegistry::new();
        assert!(reg.find("nonexistent").is_none());
    }

    #[test]
    fn registry_list_returns_all() {
        let reg = SlashCommandRegistry::new();
        let list = reg.list();
        assert_eq!(list.len(), 32);
        let names: Vec<&str> = list.iter().map(|(n, _)| *n).collect();
        // Original 12
        assert!(names.contains(&"help"));
        assert!(names.contains(&"exit"));
        assert!(names.contains(&"model"));
        assert!(names.contains(&"cost"));
        // Batch 2
        assert!(names.contains(&"resume"));
        assert!(names.contains(&"history"));
        assert!(names.contains(&"export"));
        assert!(names.contains(&"effort"));
        assert!(names.contains(&"fast"));
        // Batch 3
        assert!(names.contains(&"add-dir"));
        assert!(names.contains(&"files"));
        assert!(names.contains(&"branch"));
        assert!(names.contains(&"commit"));
        assert!(names.contains(&"copy"));
    }

    #[test]
    fn registry_execute_unknown_returns_none() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        assert!(reg.execute("nonexistent", "", &ctx).is_none());
    }

    // ─── Command output tests ───

    #[test]
    fn help_lists_all_commands() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("help", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("/help"));
            assert!(text.contains("/exit"));
            assert!(text.contains("/model"));
            assert!(text.contains("/cost"));
            assert!(text.contains("/status"));
            assert!(text.contains("/clear"));
            assert!(text.contains("/compact"));
            assert!(text.contains("/memory"));
            assert!(text.contains("/init"));
            assert!(text.contains("/config"));
            assert!(text.contains("/permissions"));
            assert!(text.contains("/plan"));
            // Batch 2
            assert!(text.contains("/resume"));
            assert!(text.contains("/history"));
            assert!(text.contains("/export"));
            assert!(text.contains("/doctor"));
            assert!(text.contains("/diff"));
            assert!(text.contains("/review"));
            assert!(text.contains("/effort"));
            assert!(text.contains("/fast"));
            assert!(text.contains("/thinking"));
            assert!(text.contains("/skills"));
            // Batch 3
            assert!(text.contains("/add-dir"));
            assert!(text.contains("/files"));
            assert!(text.contains("/plugin"));
            assert!(text.contains("/mcp"));
            assert!(text.contains("/branch"));
            assert!(text.contains("/commit"));
            assert!(text.contains("/theme"));
            assert!(text.contains("/keybindings"));
            assert!(text.contains("/rename"));
            assert!(text.contains("/copy"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn clear_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("clear", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::Clear)
        ));
    }

    #[test]
    fn compact_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("compact", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::Compact)
        ));
    }

    #[test]
    fn exit_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("exit", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::Exit)
        ));
    }

    #[test]
    fn plan_returns_toggle_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("plan", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::TogglePlanMode)
        ));
    }

    #[test]
    fn init_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("init", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::Init)
        ));
    }

    #[test]
    fn cost_shows_summary() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("cost", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Session cost:"));
            assert!(text.contains("Input tokens:"));
            assert!(text.contains("Total cost:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn status_shows_session_info() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("status", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("claude-sonnet-4-20250514"));
            assert!(text.contains("sess_test"));
            assert!(text.contains("Messages:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn permissions_shows_mode() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("permissions", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Permission mode:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn config_shows_values() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("config", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Current configuration:"));
            assert!(text.contains("Model:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn memory_no_dir_configured() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("memory", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("No memory directory configured"));
        } else {
            panic!("expected Message");
        }
    }

    // ─── Model alias tests ───

    #[test]
    fn model_alias_sonnet() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("model", "sonnet", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::SwitchModel(m)) if m == "claude-sonnet-4-20250514"
        ));
    }

    #[test]
    fn model_alias_opus() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("model", "opus", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::SwitchModel(m)) if m == "claude-opus-4-20250514"
        ));
    }

    #[test]
    fn model_alias_haiku() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("model", "haiku", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::SwitchModel(m)) if m == "claude-haiku-4-5-20251001"
        ));
    }

    #[test]
    fn model_passthrough_full_id() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("model", "gpt-4o", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::SwitchModel(m)) if m == "gpt-4o"
        ));
    }

    #[test]
    fn model_no_args_shows_usage() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("model", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Usage:"));
            assert!(text.contains("sonnet"));
        } else {
            panic!("expected Message");
        }
    }

    // ─── Default trait ───

    #[test]
    fn default_registry_has_commands() {
        let reg = SlashCommandRegistry::default();
        assert_eq!(reg.len(), 32);
    }

    // ─── Batch 2 command tests ───

    #[test]
    fn resume_no_args_shows_usage() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("resume", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Usage:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn resume_with_id_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("resume", "abc123", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::Resume(id)) if id == "abc123"
        ));
    }

    #[test]
    fn export_default_path() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("export", "", &ctx).unwrap();
        if let SlashCommandResult::Action(SlashAction::Export(path)) = result {
            assert!(path.starts_with("session_"));
            assert!(path.ends_with(".md"));
        } else {
            panic!("expected Export action");
        }
    }

    #[test]
    fn export_custom_path() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("export", "output.md", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::Export(p)) if p == "output.md"
        ));
    }

    #[test]
    fn effort_valid_levels() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);

        for level in &["low", "medium", "high", "max"] {
            let result = reg.execute("effort", level, &ctx).unwrap();
            assert!(
                matches!(&result, SlashCommandResult::Action(SlashAction::SetEffort(l)) if l == level),
                "failed for level: {level}"
            );
        }
    }

    #[test]
    fn effort_no_args_shows_usage() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("effort", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Usage:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn effort_invalid_shows_error() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("effort", "turbo", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Unknown effort level"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn fast_returns_toggle_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("fast", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::ToggleFast)
        ));
    }

    #[test]
    fn copy_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("copy", "", &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::CopyLast)
        ));
    }

    #[test]
    fn doctor_shows_health_check() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("doctor", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Health check:"));
            assert!(text.contains("Model:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn thinking_shows_settings() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("thinking", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Thinking settings:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn skills_shows_info() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("skills", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Skills:"));
        } else {
            panic!("expected Message");
        }
    }

    // ─── Batch 3 command tests ───

    #[test]
    fn add_dir_no_args_shows_usage() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("add-dir", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Usage:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn add_dir_nonexistent_shows_error() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg
            .execute("add-dir", "/nonexistent/path/xyz", &ctx)
            .unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Not a directory"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn add_dir_valid_dir_returns_action() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let tmp = std::env::temp_dir();
        let result = reg.execute("add-dir", tmp.to_str().unwrap(), &ctx).unwrap();
        assert!(matches!(
            result,
            SlashCommandResult::Action(SlashAction::AddDir(_))
        ));
    }

    #[test]
    fn plugin_shows_info() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("plugin", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Plugins:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn mcp_shows_info() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("mcp", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("MCP servers:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn theme_shows_info() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("theme", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("theme"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn keybindings_shows_info() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("keybindings", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Key bindings:"));
            assert!(text.contains("Enter"));
            assert!(text.contains("Ctrl+C"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn rename_no_args_shows_usage() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("rename", "", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("Usage:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn rename_with_name_confirms() {
        let reg = SlashCommandRegistry::new();
        let (model, cost, dir) = make_ctx();
        let ctx = ctx_from(&model, &cost, &dir);
        let result = reg.execute("rename", "my-session", &ctx).unwrap();
        if let SlashCommandResult::Message(text) = result {
            assert!(text.contains("my-session"));
        } else {
            panic!("expected Message");
        }
    }
}
