//! Slash command infrastructure for the TUI runner.
//!
//! Owns the slash dispatch flow: parses user input, consults the
//! [`CommandRegistry`] (built-in commands), falls through to the
//! [`SkillRegistry`] (user skills), and translates [`CommandEffect`]
//! variants into concrete state mutations on the [`App`].

use crab_agents::runtime::AgentRuntime;
use crab_agents::{DefaultShell, ToolRegistry};
use crab_commands::{CommandContext, CommandEffect, CommandRegistry, CommandResult, CostSnapshot};

use crate::app::{App, ChatMessage};
use crate::components::autocomplete::CommandInfo;

/// Static list of built-in slash commands for Tab completion.
pub(super) fn builtin_slash_commands() -> Vec<CommandInfo> {
    vec![
        CommandInfo {
            name: "/help".into(),
            description: "Show available commands".into(),
        },
        CommandInfo {
            name: "/clear".into(),
            description: "Clear conversation history".into(),
        },
        CommandInfo {
            name: "/compact".into(),
            description: "Compact conversation (free context)".into(),
        },
        CommandInfo {
            name: "/exit".into(),
            description: "Exit crab-code".into(),
        },
        CommandInfo {
            name: "/model".into(),
            description: "Show or switch the current model".into(),
        },
        CommandInfo {
            name: "/cost".into(),
            description: "Show token usage and cost".into(),
        },
        CommandInfo {
            name: "/status".into(),
            description: "Show session status".into(),
        },
        CommandInfo {
            name: "/memory".into(),
            description: "Show or manage memory files".into(),
        },
        CommandInfo {
            name: "/config".into(),
            description: "Open settings configuration".into(),
        },
        CommandInfo {
            name: "/permissions".into(),
            description: "Show current permission mode".into(),
        },
        CommandInfo {
            name: "/resume".into(),
            description: "Resume a previous session".into(),
        },
        CommandInfo {
            name: "/diff".into(),
            description: "Show recent file changes".into(),
        },
        CommandInfo {
            name: "/review".into(),
            description: "Review recent code changes".into(),
        },
        CommandInfo {
            name: "/commit".into(),
            description: "Create a git commit".into(),
        },
        CommandInfo {
            name: "/plan".into(),
            description: "Enter plan mode".into(),
        },
        CommandInfo {
            name: "/fast".into(),
            description: "Toggle fast mode".into(),
        },
        CommandInfo {
            name: "/thinking".into(),
            description: "Toggle extended thinking".into(),
        },
        CommandInfo {
            name: "/effort".into(),
            description: "Set effort level".into(),
        },
    ]
}

/// What should happen after processing a user-submitted line.
///
/// Any builtin slash command handled entirely in the TUI returns
/// [`SubmitOutcome::Handled`] (no LLM call). Skill expansions and
/// unrecognized text return [`SubmitOutcome::SpawnQuery`] with the
/// payload. `/exit` and `/quit` return [`SubmitOutcome::Quit`].
pub(super) enum SubmitOutcome {
    /// Send this (possibly skill-expanded) text to the LLM.
    SpawnQuery(String),
    /// Command was fully handled locally; no LLM call.
    Handled,
    /// Tear down the session.
    Quit,
}

/// Route a user-submitted line through the command registry and apply any
/// local-only effects (push system message, open overlay, compact, switch
/// model, ...). Returns what the caller should do next.
#[allow(clippy::too_many_lines)]
pub(super) async fn handle_submit(
    rt: &mut AgentRuntime,
    app: &mut App,
    command_registry: &CommandRegistry,
    text: &str,
    session_id: &str,
) -> SubmitOutcome {
    let trimmed = text.trim();

    // `!command` runs the configured shell tool directly and surfaces
    // the result as a system message — the LLM is not consulted.
    // Routing respects `default_shell` config but falls back to Bash
    // whenever PowerShell isn't actually registered (e.g. on Linux/macOS,
    // or Windows without `CRAB_USE_POWERSHELL_TOOL`).
    if let Some(command) = trimmed.strip_prefix('!') {
        let command = command.trim();
        if !command.is_empty() {
            run_shell_command(rt, app, command).await;
        }
        return SubmitOutcome::Handled;
    }

    if !trimmed.starts_with('/') {
        return SubmitOutcome::SpawnQuery(text.to_string());
    }

    let without_slash = trimmed.trim_start_matches('/');
    let command = without_slash.split_whitespace().next().unwrap_or("");
    let args = without_slash.trim_start_matches(command).trim();

    let ctx = build_command_ctx(rt, session_id);
    if let Some(result) = command_registry.execute(command, args, &ctx) {
        return match result {
            CommandResult::Message(msg) => {
                app.push_system_message(msg);
                SubmitOutcome::Handled
            }
            CommandResult::Silent => SubmitOutcome::Handled,
            CommandResult::Effect(effect) => {
                apply_command_effect(rt, app, effect, session_id).await
            }
        };
    }

    if let Some(skill) = rt.skill_registry().find_command(command) {
        let mut prompt = skill.content.clone();
        if !args.is_empty() {
            prompt.push_str("\n\nUser arguments: ");
            prompt.push_str(args);
        }
        return SubmitOutcome::SpawnQuery(prompt);
    }

    SubmitOutcome::SpawnQuery(text.to_string())
}

fn build_command_ctx<'a>(rt: &'a AgentRuntime, session_id: &'a str) -> CommandContext<'a> {
    let summary = rt.cost().summary();
    let estimated_tokens = summary.input_tokens + summary.output_tokens;
    CommandContext {
        model: &rt.loop_config().model,
        session_id,
        working_dir: &rt.tool_ctx().working_dir,
        permission_mode: rt.tool_ctx().permission_mode,
        cost: CostSnapshot {
            input_tokens: summary.input_tokens,
            output_tokens: summary.output_tokens,
            cache_read_tokens: summary.cache_read_tokens,
            cache_creation_tokens: summary.cache_creation_tokens,
            total_cost_usd: summary.total_cost_usd,
            api_calls: summary.api_calls,
        },
        estimated_tokens,
        message_count: rt.conversation().len(),
        memory_dir: rt.memory_dir(),
    }
}

/// Translate a [`CommandEffect`] into concrete state mutations.
pub(super) async fn apply_command_effect(
    rt: &mut AgentRuntime,
    app: &mut App,
    effect: CommandEffect,
    session_id: &str,
) -> SubmitOutcome {
    use crab_core::permission::PermissionMode;

    match effect {
        CommandEffect::Exit => SubmitOutcome::Quit,

        CommandEffect::Clear => {
            rt.save_session(session_id);
            rt.new_session(session_id);
            app.reset_for_new_session();
            app.push_system_message("Conversation cleared.");
            SubmitOutcome::Handled
        }

        CommandEffect::Compact => {
            let result = rt.compact_now().await;
            app.messages.push(crate::app::ChatMessage::CompactBoundary {
                strategy: result.strategy,
                after_tokens: result.after_tokens,
                removed_messages: result.removed_messages,
            });
            app.total_input_tokens = result.before_tokens;
            app.total_output_tokens = 0;
            SubmitOutcome::Handled
        }

        CommandEffect::SwitchModel(name) => {
            rt.loop_config_mut().model = crab_core::model::ModelId::from(name.as_str());
            app.model_name.clone_from(&name);
            app.push_system_message(format!("Switched model to {name}"));
            SubmitOutcome::Handled
        }

        CommandEffect::TogglePlanMode => {
            let cur = rt.tool_ctx().permission_mode;
            let next = if cur == PermissionMode::Plan {
                PermissionMode::Default
            } else {
                PermissionMode::Plan
            };
            rt.tool_ctx_mut().permission_mode = next;
            app.permission_mode = next;
            app.push_system_message(format!("Permission mode: {next}"));
            SubmitOutcome::Handled
        }

        CommandEffect::OpenOverlay(kind) => {
            app.open_overlay_by_kind(kind);
            SubmitOutcome::Handled
        }

        CommandEffect::Init => {
            let path = rt.tool_ctx().working_dir.join("AGENTS.md");
            if path.exists() {
                app.push_system_message(format!("AGENTS.md already exists at {}", path.display()));
            } else {
                let template = "# Project Instructions\n\n\
                    Use this file to tell Crab Code how to work in this project:\n\
                    conventions, required commands, test targets, review rules, etc.\n";
                match std::fs::write(&path, template) {
                    Ok(()) => app.push_system_message(format!(
                        "Wrote AGENTS.md template at {}",
                        path.display()
                    )),
                    Err(e) => {
                        app.push_system_message(format!("Failed to write AGENTS.md: {e}"));
                    }
                }
            }
            SubmitOutcome::Handled
        }

        CommandEffect::Resume(id) => {
            app.push_system_message(format!("Resuming session {id}\u{2026}"));
            app.apply_event(crate::app_event::AppEvent::SwitchSession(id));
            SubmitOutcome::Handled
        }

        CommandEffect::Export(path) => {
            use std::fmt::Write as _;
            let mut out = String::new();
            for msg in rt.conversation().messages() {
                let _ = writeln!(out, "{msg:?}\n");
            }
            match std::fs::write(&path, out) {
                Ok(()) => app.push_system_message(format!("Exported conversation to {path}")),
                Err(e) => app.push_system_message(format!("Export failed: {e}")),
            }
            SubmitOutcome::Handled
        }

        CommandEffect::SetEffort(level) => {
            if let Ok(effort) = level.parse::<crab_agents::EffortLevel>() {
                rt.loop_config_mut().effort = Some(effort);
                app.push_system_message(format!("Effort level set to {level}"));
            } else {
                app.push_system_message(format!(
                    "Unknown effort level '{level}'. Use low|medium|high|max."
                ));
            }
            SubmitOutcome::Handled
        }

        CommandEffect::ToggleFast => {
            app.push_system_message(
                "Fast mode toggle is a no-op in this build — set `fast_mode` in settings.json.",
            );
            SubmitOutcome::Handled
        }

        CommandEffect::AddDir(dir) => {
            if dir.exists() && dir.is_dir() {
                app.push_system_message(format!(
                    "Additional working dir registered: {}",
                    dir.display()
                ));
            } else {
                app.push_system_message(format!("Not a directory: {}", dir.display()));
            }
            SubmitOutcome::Handled
        }

        CommandEffect::CopyLast => {
            let last = app.messages.iter().rev().find_map(|m| match m {
                crate::app::ChatMessage::Assistant { text } => Some(text.clone()),
                _ => None,
            });
            match last {
                Some(t) => {
                    app.push_system_message(format!(
                        "Copied last assistant message ({} chars)",
                        t.len()
                    ));
                }
                None => app.push_system_message("No assistant message to copy."),
            }
            SubmitOutcome::Handled
        }

        CommandEffect::Rewind(target) => {
            match rt.rewind(target.as_deref()) {
                Ok(restored) if restored.is_empty() => {
                    app.push_system_message("No file edits to rewind.");
                }
                Ok(restored) => {
                    let list = restored.join(", ");
                    app.push_system_message(format!("Rewound: {list}"));
                }
                Err(e) => {
                    app.push_system_message(format!("Rewind failed: {e}"));
                }
            }
            SubmitOutcome::Handled
        }
    }
}

/// Pick the tool that should service a `!` prefix invocation.
///
/// `default_shell` carries the user's preference (`bash` or `powershell`),
/// but a preference for PowerShell only takes effect when the tool is
/// actually registered. On Linux/macOS or Windows without
/// `CRAB_USE_POWERSHELL_TOOL` the routing falls back to `Bash` so the
/// `!` prefix never breaks for users who flipped the config without
/// flipping the env var.
fn resolve_shell_tool_name(default_shell: DefaultShell, registry: &ToolRegistry) -> &'static str {
    match default_shell {
        DefaultShell::PowerShell
            if registry.get(DefaultShell::PowerShell.tool_name()).is_some() =>
        {
            DefaultShell::PowerShell.tool_name()
        }
        _ => DefaultShell::Bash.tool_name(),
    }
}

/// Execute `command` via the configured shell tool and push the result
/// into the transcript as a system message. Permissions are honored —
/// the tool's standard ASK / DENY paths still apply.
async fn run_shell_command(rt: &AgentRuntime, app: &mut App, command: &str) {
    let tool_name = resolve_shell_tool_name(rt.default_shell(), rt.executor().registry());

    // Echo the user's command into the transcript so the output has
    // context — the renderer otherwise loses it (we never round-trip
    // through the LLM).
    app.messages.push(ChatMessage::User {
        text: format!("!{command}"),
    });

    let input = serde_json::json!({ "command": command });
    let ctx = rt.tool_ctx().clone();
    let result = rt.executor().execute(tool_name, input, &ctx).await;

    let (output, is_error) = match result {
        Ok(out) => (out.text(), out.is_error),
        Err(e) => (format!("[shell error] {e}"), true),
    };

    app.messages.push(ChatMessage::ToolResult {
        tool_name: tool_name.to_string(),
        output,
        is_error,
        display: None,
        collapsed: false,
        is_read_only: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_tools::builtin::create_default_registry;

    #[test]
    fn resolve_defaults_to_bash() {
        let reg = create_default_registry();
        assert_eq!(resolve_shell_tool_name(DefaultShell::Bash, &reg), "Bash");
    }

    #[test]
    fn resolve_powershell_falls_back_when_unregistered() {
        // An empty registry obviously doesn't carry PowerShell — the
        // resolver must drop back to Bash so users can never end up
        // routing to a tool that won't run.
        let reg = ToolRegistry::new();
        assert_eq!(
            resolve_shell_tool_name(DefaultShell::PowerShell, &reg),
            "Bash",
        );
    }

    #[test]
    fn resolve_powershell_routes_when_registered() {
        let mut reg = ToolRegistry::new();
        reg.register(std::sync::Arc::new(StubPowerShell));
        assert_eq!(
            resolve_shell_tool_name(DefaultShell::PowerShell, &reg),
            "PowerShell",
        );
    }

    /// Minimal Tool stub registered under the canonical `PowerShell` name —
    /// keeps the resolver test self-contained instead of depending on the
    /// (Windows-only, env-gated) real PowerShell tool registration path.
    struct StubPowerShell;

    impl crab_core::tool::Tool for StubPowerShell {
        fn name(&self) -> &str {
            "PowerShell"
        }
        fn description(&self) -> &str {
            ""
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &crab_core::tool::ToolContext,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = crab_core::Result<crab_core::tool::ToolOutput>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(crab_core::tool::ToolOutput::success("")) })
        }
    }
}
