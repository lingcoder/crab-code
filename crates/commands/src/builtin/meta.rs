use crate::context::CommandContext;
use crate::types::{CommandEffect, CommandResult, OverlayKind, SlashCommand};

pub struct ConfigCommand;

impl SlashCommand for ConfigCommand {
    fn name(&self) -> &'static str {
        "config"
    }
    fn description(&self) -> &'static str {
        "Show current configuration values"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Config))
    }
}

pub struct PermissionsCommand;

impl SlashCommand for PermissionsCommand {
    fn name(&self) -> &'static str {
        "permissions"
    }
    fn description(&self) -> &'static str {
        "Show current permission mode"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Permissions))
    }
}

pub struct KeybindingsCommand;

impl SlashCommand for KeybindingsCommand {
    fn name(&self) -> &'static str {
        "keybindings"
    }
    fn description(&self) -> &'static str {
        "Show key bindings"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Message(
            "Key bindings:\n  Enter        Send message\n  Ctrl+C       Cancel current operation\n  Ctrl+D       Exit session\n  Tab          Autocomplete\n  Up/Down      Navigate history\n  Esc          Clear input".into(),
        )
    }
}

pub struct ThemeCommand;

impl SlashCommand for ThemeCommand {
    fn name(&self) -> &'static str {
        "theme"
    }
    fn description(&self) -> &'static str {
        "Show current theme"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Message("Current theme: default\nTheme customization coming soon.".into())
    }
}

pub struct PluginCommand;

impl SlashCommand for PluginCommand {
    fn name(&self) -> &'static str {
        "plugin"
    }
    fn description(&self) -> &'static str {
        "List loaded plugins"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Message(
            "Plugins:\n  No plugins loaded.\n  Use `crab plugin list` to manage plugins.".into(),
        )
    }
}

pub struct SkillsCommand;

impl SlashCommand for SkillsCommand {
    fn name(&self) -> &'static str {
        "skills"
    }
    fn description(&self) -> &'static str {
        "List available skills"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Message(
            "Skills:\n  No skills loaded.\n  Use /init to create an AGENTS.md with skill definitions."
                .into(),
        )
    }
}

pub struct McpCommand;

impl SlashCommand for McpCommand {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn description(&self) -> &'static str {
        "Open MCP server browser"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Mcp))
    }
}

pub struct TeamCommand;

impl SlashCommand for TeamCommand {
    fn name(&self) -> &'static str {
        "team"
    }
    fn description(&self) -> &'static str {
        "Open agent team browser"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Team))
    }
}

pub struct MemoryCommand;

impl SlashCommand for MemoryCommand {
    fn name(&self) -> &'static str {
        "memory"
    }
    fn description(&self) -> &'static str {
        "List memory files"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let Some(dir) = ctx.memory_dir else {
            return CommandResult::Message(
                "No memory directory configured. Set `memory_dir` in ~/.crab/settings.json.".into(),
            );
        };
        if !dir.exists() {
            return CommandResult::Message(format!(
                "Memory directory does not exist: {}",
                dir.display()
            ));
        }
        CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Memory))
    }
}

pub struct AgentsCommand;

impl SlashCommand for AgentsCommand {
    fn name(&self) -> &'static str {
        "agents"
    }
    fn description(&self) -> &'static str {
        "List agent configurations"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let mut entries = Vec::new();
        let agents_md = ctx.working_dir.join("AGENTS.md");
        if agents_md.exists() {
            entries.push(format!("  AGENTS.md ({})", agents_md.display()));
        }
        let agents_dir = ctx.working_dir.join(".claude").join("agents");
        if agents_dir.is_dir()
            && let Ok(rd) = std::fs::read_dir(&agents_dir)
        {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "md") {
                    entries.push(format!(
                        "  {}",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
        if entries.is_empty() {
            CommandResult::Message(
                "No agent configurations found.\nRun /init to create AGENTS.md.".into(),
            )
        } else {
            CommandResult::Message(format!("Agent configurations:\n{}", entries.join("\n")))
        }
    }
}

pub struct HooksCommand;

impl SlashCommand for HooksCommand {
    fn name(&self) -> &'static str {
        "hooks"
    }
    fn description(&self) -> &'static str {
        "Show hook configuration"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let settings_path = ctx.working_dir.join(".claude").join("settings.json");
        if !settings_path.exists() {
            return CommandResult::Message("No hooks configured.".into());
        }
        match std::fs::read_to_string(&settings_path) {
            Ok(content) => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                    && let Some(hooks) = json.get("hooks")
                {
                    return CommandResult::Message(format!(
                        "Hooks:\n{}",
                        serde_json::to_string_pretty(hooks).unwrap_or_default()
                    ));
                }
                CommandResult::Message("No hooks configured.".into())
            }
            Err(e) => CommandResult::Message(format!("Failed to read settings: {e}")),
        }
    }
}

pub struct TasksCommand;

impl SlashCommand for TasksCommand {
    fn name(&self) -> &'static str {
        "tasks"
    }
    fn description(&self) -> &'static str {
        "List active tasks"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Message("Task information is available in the team browser (/team).".into())
    }
}

pub struct ColorCommand;

impl SlashCommand for ColorCommand {
    fn name(&self) -> &'static str {
        "color"
    }
    fn description(&self) -> &'static str {
        "Set prompt color (/color <name>)"
    }
    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let color = args.trim();
        if color.is_empty() {
            return CommandResult::Message(
                "Usage: /color <name>\nAvailable: red, green, blue, yellow, cyan, magenta, white"
                    .into(),
            );
        }
        match color {
            "red" | "green" | "blue" | "yellow" | "cyan" | "magenta" | "white" => {
                CommandResult::Effect(CommandEffect::SetColor(color.to_string()))
            }
            _ => CommandResult::Message(format!(
                "Unknown color '{color}'. Available: red, green, blue, yellow, cyan, magenta, white"
            )),
        }
    }
}

pub struct IdeCommand;

impl SlashCommand for IdeCommand {
    fn name(&self) -> &'static str {
        "ide"
    }
    fn description(&self) -> &'static str {
        "Show IDE connection status"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        let port = std::env::var("CRAB_ACP_PORT").unwrap_or_else(|_| "not set".into());
        let ide = std::env::var("CRAB_IDE").unwrap_or_else(|_| "none".into());
        CommandResult::Message(format!("IDE status:\n  IDE: {ide}\n  ACP port: {port}"))
    }
}

pub struct ReloadPluginsCommand;

impl SlashCommand for ReloadPluginsCommand {
    fn name(&self) -> &'static str {
        "reload-plugins"
    }
    fn description(&self) -> &'static str {
        "Reload plugins and skills"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Effect(CommandEffect::ReloadPlugins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_test_ctx, test_model_and_dir};

    #[test]
    fn config_opens_overlay() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            ConfigCommand.execute("", &ctx),
            CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Config))
        ));
    }

    #[test]
    fn permissions_opens_overlay() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            PermissionsCommand.execute("", &ctx),
            CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Permissions))
        ));
    }

    #[test]
    fn keybindings_shows_info() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = KeybindingsCommand.execute("", &ctx) {
            assert!(text.contains("Key bindings:"));
            assert!(text.contains("Enter"));
            assert!(text.contains("Ctrl+C"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn theme_shows_info() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = ThemeCommand.execute("", &ctx) {
            assert!(text.contains("theme"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn plugin_shows_info() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = PluginCommand.execute("", &ctx) {
            assert!(text.contains("Plugins:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn skills_shows_info() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = SkillsCommand.execute("", &ctx) {
            assert!(text.contains("Skills:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn mcp_opens_overlay() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            McpCommand.execute("", &ctx),
            CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Mcp))
        ));
    }

    #[test]
    fn team_opens_overlay() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            TeamCommand.execute("", &ctx),
            CommandResult::Effect(CommandEffect::OpenOverlay(OverlayKind::Team))
        ));
    }

    #[test]
    fn memory_no_dir_configured() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = MemoryCommand.execute("", &ctx) {
            assert!(text.contains("No memory directory configured"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn agents_no_config_shows_hint() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = AgentsCommand.execute("", &ctx) {
            assert!(text.contains("No agent configurations"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn hooks_no_settings_shows_message() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = HooksCommand.execute("", &ctx) {
            assert!(text.contains("No hooks configured"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn tasks_shows_message() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = TasksCommand.execute("", &ctx) {
            assert!(text.contains("team browser"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn color_no_args_shows_usage() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = ColorCommand.execute("", &ctx) {
            assert!(text.contains("Usage:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn color_valid_returns_effect() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            ColorCommand.execute("red", &ctx),
            CommandResult::Effect(CommandEffect::SetColor(ref c)) if c == "red"
        ));
    }

    #[test]
    fn color_invalid_shows_error() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = ColorCommand.execute("puce", &ctx) {
            assert!(text.contains("Unknown color"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn ide_shows_status() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = IdeCommand.execute("", &ctx) {
            assert!(text.contains("IDE status:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn reload_plugins_returns_effect() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            ReloadPluginsCommand.execute("", &ctx),
            CommandResult::Effect(CommandEffect::ReloadPlugins)
        ));
    }
}
