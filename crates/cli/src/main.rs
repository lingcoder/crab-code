mod commands;
mod setup;

use std::borrow::Cow;
use std::io::Write;

use clap::Parser;
use futures::stream::StreamExt;

use crab_api::types::{MessageRequest, StreamEvent};
use crab_core::message::Message;
use crab_core::model::ModelId;

/// Crab Code -- Rust-native Agentic Coding CLI
#[derive(Parser)]
#[command(name = "crab", version, about)]
struct Cli {
    /// User prompt (if provided, runs in non-interactive demo mode)
    prompt: Option<String>,

    /// LLM provider: "anthropic" (default) or "openai"
    #[arg(long, default_value = "anthropic")]
    provider: String,

    /// Model ID override (e.g. "claude-sonnet-4-20250514", "gpt-4o")
    #[arg(long, short)]
    model: Option<String>,

    /// Maximum output tokens
    #[arg(long, default_value = "4096")]
    max_tokens: u32,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(prompt) = cli.prompt {
        // Demo mode: single-shot streaming query
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_demo(cli.provider, cli.model, cli.max_tokens, prompt))
    } else {
        // Interactive mode placeholder
        println!("crab-code v{}", env!("CARGO_PKG_VERSION"));
        println!("Interactive mode not yet implemented. Pass a prompt as argument:");
        println!("  cargo run -p crab-cli -- \"hello\"");
        Ok(())
    }
}

async fn run_demo(
    provider: String,
    model_override: Option<String>,
    max_tokens: u32,
    prompt: String,
) -> anyhow::Result<()> {
    // Build settings from the CLI args
    let mut settings = crab_config::Settings {
        api_provider: Some(provider.clone()),
        ..Default::default()
    };
    if let Some(ref m) = model_override {
        settings.model = Some(m.clone());
    }

    // Resolve model ID
    let model_id = model_override.unwrap_or_else(|| {
        if provider == "openai" {
            "gpt-4o".to_string()
        } else {
            "claude-sonnet-4-20250514".to_string()
        }
    });

    eprintln!("[crab] provider={provider}, model={model_id}, max_tokens={max_tokens}");

    // Create backend
    let backend = crab_api::create_backend(&settings);
    eprintln!("[crab] backend={}", backend.name());

    // Build request
    let messages = vec![Message::user(&prompt)];
    let req = MessageRequest {
        model: ModelId::from(model_id.as_str()),
        messages: Cow::Borrowed(&messages),
        system: None,
        max_tokens,
        tools: vec![],
        temperature: None,
        cache_breakpoints: vec![],
    };

    // Stream response
    let mut stream = std::pin::pin!(backend.stream_message(req));
    let mut stdout = std::io::stdout();
    let mut total_input = 0u64;
    let mut total_output = 0u64;

    while let Some(event) = stream.next().await {
        match event {
            Ok(ev) => match ev {
                StreamEvent::MessageStart { id, usage } => {
                    eprintln!("[crab] message_start id={id}");
                    total_input += usage.input_tokens;
                    total_output += usage.output_tokens;
                }
                StreamEvent::ContentDelta { delta, .. } => {
                    print!("{delta}");
                    stdout.flush()?;
                }
                StreamEvent::MessageDelta {
                    usage, stop_reason, ..
                } => {
                    total_output += usage.output_tokens;
                    if let Some(reason) = stop_reason {
                        eprintln!("\n[crab] stop_reason={reason}");
                    }
                }
                StreamEvent::MessageStop => {
                    // end
                }
                StreamEvent::ContentBlockStart { index, content_type } => {
                    eprintln!("[crab] content_block_start index={index} type={content_type}");
                }
                StreamEvent::ContentBlockStop { index } => {
                    eprintln!("[crab] content_block_stop index={index}");
                }
                StreamEvent::Error { message } => {
                    eprintln!("\n[crab] stream error: {message}");
                }
            },
            Err(e) => {
                eprintln!("\n[crab] error: {e}");
                return Err(e.into());
            }
        }
    }

    println!();
    eprintln!(
        "[crab] tokens: input={}, output={}, total={}",
        total_input,
        total_output,
        total_input + total_output,
    );

    Ok(())
}
