use tokio::sync::mpsc;

use crab_agents::AgentSession;
use crab_core::event::Event;

/// Interactive REPL: read lines, send to agent, print streaming output.
#[cfg(not(feature = "tui"))]
pub async fn run_repl(
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
pub enum SlashOutcome {
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
pub async fn dispatch_slash_command(
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
        context_window: session.conversation.context_window,
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
            let expanded =
                super::agent::resolve_slash_command(&format!("/{cmd_rest}"), skill_registry);
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
                "[info] Compacted {before} messages -> {after} (extracted {} summary items).",
                summary.items.len()
            );
            SlashOutcome::Continue
        }
        CommandEffect::Rewind(target) => {
            match session.rewind(target.as_deref()) {
                Ok(restored) if restored.is_empty() => {
                    println!("[info] No file edits to rewind.");
                }
                Ok(restored) => {
                    println!("[info] Rewound: {}", restored.join(", "));
                }
                Err(e) => {
                    println!("[info] Rewind failed: {e}");
                }
            }
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
pub async fn run_turn(session: &mut AgentSession, input: &str) {
    use crate::args::OutputFormat;
    use crate::output::print_events;

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
pub fn take_event_rx(session: &mut AgentSession) -> mpsc::Receiver<Event> {
    // Create a fresh channel: session sends via tx, printer reads via rx.
    let (tx, rx) = mpsc::channel(256);
    session.event_tx = tx;
    rx
}
