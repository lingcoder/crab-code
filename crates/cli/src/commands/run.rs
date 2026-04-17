//! Non-interactive single-shot execution mode.
//!
//! Takes a prompt from CLI args or stdin, runs one query, outputs the result.
//! Used for scripting and piping: `crab -p "explain this code" < file.rs`
//!
//! This module handles the "print mode" flow:
//! 1. Build config → create API backend → build tool registry
//! 2. Construct a single prompt (from args or stdin)
//! 3. Execute the agent loop (with optional max-turns limit)
//! 4. Format and output the result (text, JSON, or streaming)
//! 5. Exit with appropriate status code

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use crab_agent::{AgentSession, SessionConfig};
use crab_config::settings;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_tools::builtin::create_default_registry;

/// Output format for non-interactive execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Plain text — assistant response only (default).
    #[default]
    Text,
    /// JSON — structured output with metadata.
    Json,
    /// Streaming — emit tokens as they arrive (NDJSON).
    Streaming,
}

/// Configuration for non-interactive execution.
pub struct RunConfig {
    /// The prompt to send to the agent.
    pub prompt: String,
    /// Project root directory.
    pub project_dir: PathBuf,
    /// Provider name (e.g. "anthropic", "openai").
    pub provider: String,
    /// Model ID override.
    pub model: Option<String>,
    /// Maximum output tokens per response.
    pub max_tokens: u32,
    /// Permission mode override.
    pub permission_mode: Option<String>,
    /// Output format: text, JSON, or streaming.
    pub output_format: OutputFormat,
    /// Maximum number of agent turns (None = unlimited).
    pub max_turns: Option<u32>,
    /// Enable verbose/debug logging.
    pub verbose: bool,
    /// Custom system prompt from settings.
    pub custom_instructions: Option<String>,
    /// Bare mode: skip hooks, plugins, memory, CRAB.md.
    pub bare_mode: bool,
    /// Allowed tools (filters).
    pub allowed_tools: Vec<String>,
    /// Denied tools (filters).
    pub denied_tools: Vec<String>,
    /// Fallback model when primary is overloaded.
    pub fallback_model: Option<String>,
    /// Effort level.
    pub effort: Option<String>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            project_dir: PathBuf::from("."),
            provider: "anthropic".to_string(),
            model: None,
            max_tokens: 4096,
            permission_mode: None,
            output_format: OutputFormat::Text,
            max_turns: None,
            verbose: false,
            custom_instructions: None,
            bare_mode: false,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            fallback_model: None,
            effort: None,
        }
    }
}

/// Run a single non-interactive query.
///
/// This is the entry point for `crab -p "prompt"` and `echo "prompt" | crab -p`.
///
/// # Flow
///
/// 1. Load and merge settings (global + project + env)
/// 2. Create LLM backend and tool registry
/// 3. Build system prompt and session config
/// 4. Execute the agent loop for one user message
/// 5. Drain events and format output to stdout
///
/// # Errors
///
/// Returns an error if settings loading, backend creation, or the query fails.
pub async fn run_once(config: RunConfig) -> anyhow::Result<()> {
    // 1. Load settings
    let merged_settings = settings::load_merged_settings(Some(&config.project_dir))
        .context("failed to load settings")?;

    // 2. Resolve provider and model
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

    // 3. Build backend
    let effective_settings = crab_config::Settings {
        api_provider: Some(provider.clone()),
        api_base_url: merged_settings.api_base_url.clone(),
        api_key: merged_settings.api_key.clone(),
        model: Some(model_id.clone()),
        ..merged_settings.clone()
    };

    let backend = Arc::new(crab_api::create_backend(&effective_settings));
    let registry = create_default_registry();

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

    // 5. Build system prompt
    let system_prompt = crab_agent::build_system_prompt(
        &config.project_dir,
        &registry,
        effective_settings.system_prompt.as_deref(),
    );

    let global_dir = settings::global_config_dir();
    let session_id = crab_common::utils::id::new_ulid();

    // 6. Build session config
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
            allowed_tools: config.allowed_tools,
            denied_tools: config.denied_tools,
        },
        memory_dir: if config.bare_mode {
            None
        } else {
            Some(global_dir.join("memory"))
        },
        sessions_dir: None, // No session persistence in print mode
        resume_session_id: None,
        effort: config.effort,
        thinking_mode: None,
        additional_dirs: Vec::new(),
        session_name: None,
        max_turns: config.max_turns,
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

    // 7. Create agent session and run
    let mut session = AgentSession::new(session_config, backend, registry);

    // Set up a simple CLI permission handler for non-interactive mode.
    // In print mode, we auto-allow by default (the user explicitly ran
    // a non-interactive command). If the permission mode is restrictive,
    // the permission system will still gate dangerous operations.
    session
        .executor
        .set_permission_handler(Arc::new(PrintModePermissionHandler));

    // Take the event receiver for output formatting
    let event_rx = take_event_rx(&mut session);

    // Spawn output printer in background
    let format = config.output_format;
    let printer = tokio::spawn(drain_events(event_rx, format));

    // Execute the query
    let result = session.handle_user_input(&config.prompt).await;

    // Signal printer to stop by dropping the sender
    let (dummy_tx, dummy_rx) = tokio::sync::mpsc::channel::<Event>(1);
    session.event_tx = dummy_tx;
    drop(dummy_rx);
    let _ = printer.await;

    result.map_err(Into::into)
}

/// Permission handler for print mode.
///
/// In non-interactive single-shot mode, permission prompts cannot be shown
/// to the user. The handler denies by default — the user should use
/// `--dangerously-skip-permissions` or `--trust-project` if they need
/// write operations in print mode.
struct PrintModePermissionHandler;

impl crab_tools::executor::PermissionHandler for PrintModePermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let prompt = prompt.to_string();
        Box::pin(async move {
            eprintln!(
                "[permission] Cannot prompt in non-interactive mode. \
                 Denying: {tool_name} ({prompt})"
            );
            eprintln!(
                "[permission] Use --dangerously-skip-permissions or \
                 --trust-project for automated workflows."
            );
            false
        })
    }
}

/// Swap the session's `event_rx` with a fresh one, returning the old receiver.
fn take_event_rx(session: &mut AgentSession) -> tokio::sync::mpsc::Receiver<Event> {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    session.event_tx = tx;
    rx
}

/// Drain events from the agent and write output to stdout/stderr.
async fn drain_events(mut rx: tokio::sync::mpsc::Receiver<Event>, format: OutputFormat) {
    use std::io::Write;
    let mut stdout = std::io::stdout();

    while let Some(event) = rx.recv().await {
        match format {
            OutputFormat::Text => match &event {
                Event::ContentDelta { delta, .. } => {
                    print!("{delta}");
                    let _ = stdout.flush();
                }
                Event::ToolUseStart { name, .. } => {
                    eprintln!("[tool] {name}");
                }
                Event::ToolOutputDelta { delta, .. } => {
                    eprint!("{delta}");
                    let _ = std::io::stderr().flush();
                }
                Event::ToolResult { output, .. } if output.is_error => {
                    eprintln!("[tool error] {}", output.text());
                }
                Event::Error { message } => {
                    eprintln!("[error] {message}");
                }
                _ => {}
            },
            OutputFormat::Json | OutputFormat::Streaming => {
                // Emit events as NDJSON
                if let Ok(json) = serde_json::to_string(&event_to_value(&event)) {
                    println!("{json}");
                }
            }
        }
    }

    // Ensure final newline for text mode
    if format == OutputFormat::Text {
        println!();
    }
}

/// Convert an Event to a JSON Value for structured output.
fn event_to_value(event: &Event) -> serde_json::Value {
    match event {
        Event::ContentDelta { index, delta } => {
            serde_json::json!({
                "type": "content_delta",
                "index": index,
                "delta": delta,
            })
        }
        Event::ToolUseStart { id, name, input } => {
            serde_json::json!({
                "type": "tool_use_start",
                "id": id,
                "name": name,
                "input": input,
            })
        }
        Event::ToolResult { id, output } => {
            serde_json::json!({
                "type": "tool_result",
                "id": id,
                "content": output.text(),
                "is_error": output.is_error,
            })
        }
        Event::Error { message } => {
            serde_json::json!({
                "type": "error",
                "message": message,
            })
        }
        Event::MessageEnd { usage } => {
            serde_json::json!({
                "type": "message_end",
                "usage": {
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                },
            })
        }
        _ => {
            serde_json::json!({
                "type": "other",
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_config_default() {
        let config = RunConfig::default();
        assert!(config.prompt.is_empty());
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.max_tokens, 4096);
        assert_eq!(config.output_format, OutputFormat::Text);
        assert!(config.max_turns.is_none());
        assert!(!config.verbose);
        assert!(!config.bare_mode);
    }

    #[test]
    fn run_config_with_overrides() {
        let config = RunConfig {
            prompt: "explain this code".into(),
            model: Some("gpt-4o".into()),
            provider: "openai".into(),
            output_format: OutputFormat::Json,
            max_turns: Some(5),
            ..Default::default()
        };
        assert_eq!(config.prompt, "explain this code");
        assert_eq!(config.model.as_deref(), Some("gpt-4o"));
        assert_eq!(config.output_format, OutputFormat::Json);
        assert_eq!(config.max_turns, Some(5));
    }

    #[test]
    fn output_format_default_is_text() {
        assert_eq!(OutputFormat::default(), OutputFormat::Text);
    }

    #[test]
    fn event_to_value_content_delta() {
        let event = Event::ContentDelta {
            index: 0,
            delta: "hello".into(),
        };
        let value = event_to_value(&event);
        assert_eq!(value["type"], "content_delta");
        assert_eq!(value["delta"], "hello");
    }

    #[test]
    fn event_to_value_error() {
        let event = Event::Error {
            message: "something broke".into(),
        };
        let value = event_to_value(&event);
        assert_eq!(value["type"], "error");
        assert_eq!(value["message"], "something broke");
    }

    #[test]
    fn event_to_value_tool_start() {
        let event = Event::ToolUseStart {
            id: "tu_1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
        };
        let value = event_to_value(&event);
        assert_eq!(value["type"], "tool_use_start");
        assert_eq!(value["name"], "bash");
    }
}
