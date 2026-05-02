use std::fmt;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use owo_colors::OwoColorize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crab_core::event::Event;

use crate::args::OutputFormat;

/// Drain events from the receiver and print them to stdout/stderr.
///
/// `OutputFormat::Json` and `StreamJson` emit NDJSON to stdout.
/// `OutputFormat::Text` uses colored human-readable output.
pub async fn print_events(
    mut rx: mpsc::Receiver<Event>,
    output_format: OutputFormat,
    registry: Arc<crab_tools::registry::ToolRegistry>,
) {
    let mut stdout = std::io::stdout();
    let mut spinner: Option<Spinner> = None;

    while let Some(event) = rx.recv().await {
        match output_format {
            OutputFormat::Json | OutputFormat::StreamJson => {
                if let Some(value) = event_to_json(&event)
                    && let Ok(line) = serde_json::to_string(&value)
                {
                    println!("{line}");
                }
                continue;
            }
            OutputFormat::Text => {}
        }

        match event {
            // Only print text content (index 0), not tool arguments (index 1000+)
            Event::ContentDelta { index: 0, delta } => {
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                print!("{delta}");
                let _ = stdout.flush();
            }
            Event::ToolUseStart { name, input, .. } => {
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                let summary = registry
                    .get(&name)
                    .and_then(|tool| tool.format_use_summary(&input))
                    .unwrap_or_else(|| name.clone());
                eprintln!("{} {}", "tool:".cyan().bold(), summary.cyan());
                spinner = Some(Spinner::start(&format!("running {name}...")));
            }
            Event::ToolOutputDelta { id: _, delta } => {
                // Stream tool output in real-time (e.g. bash stdout)
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                eprint!("{delta}");
                let _ = std::io::stderr().flush();
            }
            Event::ToolResult { id: _, output: o } => {
                if let Some(mut s) = spinner.take() {
                    s.stop();
                }
                let text = o.text();
                if o.is_error {
                    eprintln!("{} {text}", "tool error:".red().bold());
                } else {
                    let display = if text.len() > 500 {
                        format!("{}...", &text[..500])
                    } else {
                        text
                    };
                    eprintln!("{} {display}", "result:".dimmed());
                }
            }
            Event::Error { message } => {
                eprintln!("{} {message}", "error:".red().bold());
            }
            Event::TokenWarning {
                usage_pct,
                used,
                limit,
            } => {
                eprintln!(
                    "{} Token usage {:.0}% ({used}/{limit})",
                    "warn:".yellow().bold(),
                    usage_pct * 100.0,
                );
            }
            Event::CompactStart { strategy, .. } => {
                eprintln!(
                    "{} Starting compaction: {strategy}",
                    "compact:".magenta().bold()
                );
            }
            Event::CompactEnd {
                after_tokens,
                removed_messages,
            } => {
                eprintln!(
                    "{} removed {removed_messages} messages, now {after_tokens} tokens",
                    "compact:".magenta().bold()
                );
            }
            _ => {}
        }
    }
}

#[cfg(feature = "tui")]
pub fn print_exit_info(info: &crab_tui::ExitInfo) {
    if info.had_conversation && !info.session_id.is_empty() {
        eprintln!(
            "\n{}\n",
            format!(
                "Resume this session with:\ncrab --resume {}",
                info.session_id
            )
            .dimmed()
        );
    }
}

pub fn print_banner(
    version: &str,
    provider: &str,
    model: &str,
    permission_mode: &impl fmt::Display,
) {
    eprintln!(
        "{} {} {} provider={} model={} permissions={}",
        "crab-code".green().bold(),
        version.dimmed(),
        "|".dimmed(),
        provider.cyan(),
        model.cyan(),
        format!("{permission_mode}").yellow(),
    );
}

pub struct Spinner {
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(message: &str) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let msg = message.to_string();

        let handle = std::thread::spawn(move || {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let mut i = 0;
            while running_clone.load(Ordering::Relaxed) {
                eprint!(
                    "\r{} {}",
                    frames[i % frames.len()].to_string().cyan(),
                    msg.dimmed()
                );
                let _ = std::io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_millis(80));
                i += 1;
            }
            eprint!("\r{}\r", " ".repeat(msg.len() + 4));
            let _ = std::io::stderr().flush();
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn event_to_json(event: &Event) -> Option<Value> {
    match event {
        Event::TurnStart { turn_index } => Some(json!({
            "type": "turn_start",
            "turn_index": turn_index,
        })),
        Event::MessageStart { id } => Some(json!({
            "type": "message_start",
            "id": id,
            "role": "assistant",
        })),
        Event::ContentDelta { index, delta } => Some(json!({
            "type": "content_delta",
            "index": index,
            "delta": delta,
        })),
        Event::ThinkingDelta { index, delta } => Some(json!({
            "type": "thinking_delta",
            "index": index,
            "delta": delta,
        })),
        Event::ContentBlockStop { index } => Some(json!({
            "type": "content_block_stop",
            "index": index,
        })),
        Event::MessageEnd { usage } => Some(json!({
            "type": "message_end",
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cache_read_tokens": usage.cache_read_tokens,
                "cache_creation_tokens": usage.cache_creation_tokens,
            },
        })),
        Event::ToolUseStart { name, id, input } => Some(json!({
            "type": "tool_use_start",
            "tool": name,
            "id": id,
            "input": input,
        })),
        Event::ToolUseInput { id, input } => Some(json!({
            "type": "tool_use_input",
            "id": id,
            "input": input,
        })),
        Event::ToolOutputDelta { id, delta } => Some(json!({
            "type": "tool_output_delta",
            "id": id,
            "delta": delta,
        })),
        Event::ToolProgress { id, progress } => Some(json!({
            "type": "tool_progress",
            "id": id,
            "elapsed_secs": progress.elapsed_secs,
            "total_lines": progress.total_lines,
            "total_bytes": progress.total_bytes,
        })),
        Event::ToolResult { id, output } => Some(json!({
            "type": "tool_result",
            "id": id,
            "is_error": output.is_error,
            "text": output.text(),
        })),
        Event::Error { message } => Some(json!({
            "type": "error",
            "message": message,
        })),
        Event::TokenWarning {
            usage_pct,
            used,
            limit,
        } => Some(json!({
            "type": "token_warning",
            "usage_pct": usage_pct,
            "used": used,
            "limit": limit,
        })),
        Event::CompactStart {
            strategy,
            before_tokens,
        } => Some(json!({
            "type": "compact_start",
            "strategy": strategy,
            "before_tokens": before_tokens,
        })),
        Event::CompactEnd {
            after_tokens,
            removed_messages,
        } => Some(json!({
            "type": "compact_end",
            "after_tokens": after_tokens,
            "removed_messages": removed_messages,
        })),
        Event::PermissionRequest {
            tool_name,
            input_summary,
            request_id,
            tool_input,
        } => Some(json!({
            "type": "permission_request",
            "tool_name": tool_name,
            "input_summary": input_summary,
            "request_id": request_id,
            "tool_input": tool_input,
        })),
        Event::PermissionResponse {
            request_id,
            allowed,
            feedback,
        } => {
            let mut obj = json!({
                "type": "permission_response",
                "request_id": request_id,
                "allowed": allowed,
            });
            if let Some(text) = feedback {
                obj.as_object_mut()
                    .unwrap()
                    .insert("feedback".into(), Value::String(text.clone()));
            }
            Some(obj)
        }
        Event::MemoryLoaded { count } => Some(json!({
            "type": "memory_loaded",
            "count": count,
        })),
        Event::MemorySaved { filename } => Some(json!({
            "type": "memory_saved",
            "filename": filename,
        })),
        Event::SessionSaved { session_id } => Some(json!({
            "type": "session_saved",
            "session_id": session_id,
        })),
        Event::SessionResumed {
            session_id,
            message_count,
        } => Some(json!({
            "type": "session_resumed",
            "session_id": session_id,
            "message_count": message_count,
        })),
        Event::AgentWorkerStarted {
            worker_id,
            task_prompt,
        } => Some(json!({
            "type": "agent_worker_started",
            "worker_id": worker_id,
            "task_prompt": task_prompt,
        })),
        Event::AgentWorkerCompleted {
            worker_id,
            result,
            success,
            usage,
        } => Some(json!({
            "type": "agent_worker_completed",
            "worker_id": worker_id,
            "result": result,
            "success": success,
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cache_read_tokens": usage.cache_read_tokens,
                "cache_creation_tokens": usage.cache_creation_tokens,
            },
        })),
        Event::ContextUpgraded { .. } | Event::StreamAborted { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_to_json_content_delta() {
        let event = Event::ContentDelta {
            index: 0,
            delta: "hello".into(),
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "content_delta");
        assert_eq!(json["delta"], "hello");
    }

    #[test]
    fn event_to_json_thinking_delta() {
        let event = Event::ThinkingDelta {
            index: 0,
            delta: "reasoning...".into(),
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "thinking_delta");
        assert_eq!(json["delta"], "reasoning...");
    }

    #[test]
    fn event_to_json_message_start() {
        let event = Event::MessageStart { id: "msg_1".into() };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "message_start");
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["id"], "msg_1");
    }

    #[test]
    fn event_to_json_message_end() {
        let event = Event::MessageEnd {
            usage: crab_core::model::TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 10,
                cache_creation_tokens: 5,
            },
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "message_end");
        assert_eq!(json["usage"]["input_tokens"], 100);
        assert_eq!(json["usage"]["output_tokens"], 50);
    }

    #[test]
    fn event_to_json_tool_use_input() {
        let event = Event::ToolUseInput {
            id: "tu_1".into(),
            input: json!({"command": "ls"}),
        };
        let json = event_to_json(&event).unwrap();
        assert_eq!(json["type"], "tool_use_input");
        assert_eq!(json["input"]["command"], "ls");
    }

    #[test]
    fn event_to_json_all_variants_return_some() {
        // Ensure no variant returns None (exhaustive coverage)
        use crab_core::model::TokenUsage;
        use crab_core::tool::ToolOutput;

        let events = vec![
            Event::TurnStart { turn_index: 0 },
            Event::MessageStart { id: "m".into() },
            Event::ContentDelta {
                index: 0,
                delta: "d".into(),
            },
            Event::ThinkingDelta {
                index: 0,
                delta: "t".into(),
            },
            Event::ContentBlockStop { index: 0 },
            Event::MessageEnd {
                usage: TokenUsage::default(),
            },
            Event::ToolUseStart {
                id: "t".into(),
                name: "n".into(),
                input: Value::Null,
            },
            Event::ToolUseInput {
                id: "t".into(),
                input: json!({}),
            },
            Event::ToolOutputDelta {
                id: "t".into(),
                delta: "line".into(),
            },
            Event::ToolProgress {
                id: "t".into(),
                progress: crab_core::tool::ToolProgress {
                    elapsed_secs: 1.0,
                    total_lines: 10,
                    total_bytes: 256,
                    tail_output: String::new(),
                    timeout_secs: None,
                },
            },
            Event::ToolResult {
                id: "t".into(),
                output: ToolOutput::success("ok"),
            },
            Event::Error {
                message: "e".into(),
            },
            Event::TokenWarning {
                usage_pct: 0.5,
                used: 50,
                limit: 100,
            },
            Event::CompactStart {
                strategy: "s".into(),
                before_tokens: 0,
            },
            Event::CompactEnd {
                after_tokens: 0,
                removed_messages: 0,
            },
            Event::PermissionRequest {
                tool_name: "t".into(),
                input_summary: "s".into(),
                request_id: "r".into(),
                tool_input: serde_json::Value::Null,
            },
            Event::PermissionResponse {
                request_id: "r".into(),
                allowed: true,
                feedback: None,
            },
            Event::PermissionResponse {
                request_id: "r2".into(),
                allowed: false,
                feedback: Some("use Read instead".into()),
            },
            Event::MemoryLoaded { count: 0 },
            Event::MemorySaved {
                filename: "f".into(),
            },
            Event::SessionSaved {
                session_id: "s".into(),
            },
            Event::SessionResumed {
                session_id: "s".into(),
                message_count: 0,
            },
            Event::AgentWorkerStarted {
                worker_id: "w".into(),
                task_prompt: "p".into(),
            },
            Event::AgentWorkerCompleted {
                worker_id: "w".into(),
                result: None,
                success: true,
                usage: TokenUsage::default(),
            },
        ];

        for event in &events {
            assert!(
                event_to_json(event).is_some(),
                "event_to_json returned None for {event:?}",
            );
        }
    }
}
