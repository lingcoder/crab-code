//! Shared sub-agent spawn helpers used by both runtime facades.
//!
//! The Agent tool emits a `spawn_agent` marker into its tool-result text; these
//! helpers scan a turn's tool results for that marker, spawn each requested
//! worker into a [`WorkerPool`], and fold the worker's output back into the
//! conversation as an `<agent-result>` user message.

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message};
use crab_core::model::ModelId;
use crab_core::tool::ToolContext;
use crab_engine::QueryConfig;
use crab_session::{Conversation, CostAccumulator};
use crab_swarm::backend::{TeammateRunCtx, TeammateRunner};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;

use super::worker::WorkerResult;
use super::worker_pool::WorkerPool;
use crate::builtin::builtin_agents;
use crate::definition::{AgentDefinition, ToolSet};

/// The marker `action` value the Agent tool emits for a spawn request.
pub const SPAWN_AGENT_ACTION: &str = "spawn_agent";

/// Scan `conversation[starting_len..]` tool-result text for `spawn_agent`
/// markers emitted by the Agent tool.
#[must_use]
pub fn scan_spawn_markers(
    conversation: &Conversation,
    starting_len: usize,
) -> Vec<serde_json::Value> {
    conversation
        .messages()
        .iter()
        .skip(starting_len)
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => {
                serde_json::from_str::<serde_json::Value>(content).ok()
            }
            _ => None,
        })
        .filter(|v| v.get("action").and_then(serde_json::Value::as_str) == Some(SPAWN_AGENT_ACTION))
        .collect()
}

/// Spawn one worker from a parsed `spawn_agent` marker into `pool`, using the
/// caller-supplied worker executor (its tool registry) and base system prompt.
///
/// Returns the worker id, or `None` when the marker is not a spawn request. The
/// worker never persists into the parent session's crash log (its
/// `session_persister` is cleared) since it runs an independent conversation.
#[allow(clippy::too_many_arguments)]
pub fn spawn_worker_from_marker(
    pool: &mut WorkerPool,
    marker: &serde_json::Value,
    backend: &Arc<LlmBackend>,
    worker_registry: Arc<ToolRegistry>,
    parent_prompt: &str,
    tool_ctx: &ToolContext,
    loop_config: &QueryConfig,
    event_tx: &mpsc::Sender<Event>,
) -> Option<String> {
    if marker.get("action")?.as_str()? != SPAWN_AGENT_ACTION {
        return None;
    }
    let task = marker.get("task")?.as_str()?.to_string();
    let max_turns = marker
        .get("max_turns")
        .and_then(serde_json::Value::as_u64)
        .map(|v| usize::try_from(v).unwrap_or(usize::MAX));

    // An optional named agent definition (`subagent_type`) supplies the
    // worker's system prompt, restricts its tool registry (read-only /
    // disallowed / explicit allow-list), and may override the model. When
    // absent or unknown, fall back to the generic worker prompt + parent
    // registry so nothing regresses.
    let def = marker
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .and_then(|t| builtin_agents().into_iter().find(|d| d.agent_type == t));

    let (system_prompt, registry) = if let Some(d) = &def {
        (
            d.system_prompt.clone(),
            Arc::new(build_def_registry(&worker_registry, d)),
        )
    } else {
        (
            format!("You are a sub-agent worker. Complete the assigned task.\n\n{parent_prompt}"),
            worker_registry,
        )
    };

    // A spawned worker has no interactive permission handler, so without this
    // it would fail closed (tp-7) and could not run any write/exec tool. The
    // spawn itself was user-confirmed (the Agent tool requires confirmation)
    // and the worker runs under the parent's restricted permission mode
    // (`restrict_to` below), so unattended auto-approval is the right posture.
    let mut exec = ToolExecutor::new(registry);
    exec.set_allow_unattended(true);
    let executor = Arc::new(exec);

    let mut worker_ctx = tool_ctx.clone();
    if let Some(parent_mode) = marker
        .get("parent_permission_mode")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<crab_core::permission::PermissionMode>().ok())
    {
        worker_ctx.permission_mode = worker_ctx.permission_mode.restrict_to(parent_mode);
    }

    // The worker runs an independent conversation, so it must not append to the
    // parent session's JSONL crash log.
    let mut worker_config = loop_config.clone();
    worker_config.session_persister = None;
    if let Some(d) = &def
        && let Some(model) = &d.model
    {
        worker_config.model = ModelId::from(model.clone());
    }

    Some(pool.spawn_worker(
        task,
        system_prompt,
        backend.clone(),
        executor,
        worker_ctx,
        worker_config,
        event_tx.clone(),
        max_turns,
    ))
}

/// Build a worker tool registry from `parent`, applying an agent definition's
/// restrictions: read-only agents keep only read-only tools, an explicit
/// `ToolSet::Specific` allow-list is retained, and `disallowed_tools` are
/// removed.
fn build_def_registry(parent: &ToolRegistry, def: &AgentDefinition) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for name in parent.tool_names() {
        if let Some(tool) = parent.get(name) {
            if def.read_only && !tool.is_read_only() {
                continue;
            }
            reg.register(Arc::clone(tool));
        }
    }
    if let ToolSet::Specific(allowed) = &def.tools {
        let allow: Vec<&str> = allowed.iter().map(String::as_str).collect();
        reg.retain_names(&allow);
    }
    let deny: Vec<&str> = def.disallowed_tools.iter().map(String::as_str).collect();
    reg.remove_names(&deny);
    reg
}

/// Build a teammate runner for the in-process swarm backend.
///
/// Each spawned teammate waits for its first inbound message, then runs an
/// agent loop seeded with its configured system prompt against that message
/// (looping for any subsequent messages until cancelled). Like sub-agent
/// workers, teammates run unattended under the inherited permission mode since
/// they have no interactive handler.
pub fn teammate_runner(
    backend: Arc<LlmBackend>,
    registry: Arc<ToolRegistry>,
    tool_ctx: ToolContext,
    loop_config: QueryConfig,
    event_tx: mpsc::Sender<Event>,
) -> TeammateRunner {
    Arc::new(move |ctx: TeammateRunCtx| {
        let backend = Arc::clone(&backend);
        let registry = Arc::clone(&registry);
        let base_ctx = tool_ctx.clone();
        let mut loop_config = loop_config.clone();
        loop_config.session_persister = None;
        let event_tx = event_tx.clone();
        Box::pin(async move {
            let TeammateRunCtx {
                id,
                config,
                mut rx,
                cancel,
            } = ctx;
            let mut exec = ToolExecutor::new(registry);
            exec.set_allow_unattended(true);
            let mut tool_ctx = base_ctx;
            if let Some(wd) = &config.working_dir {
                tool_ctx.working_dir = wd.clone();
            }
            loop {
                let seed = tokio::select! {
                    () = cancel.cancelled() => break,
                    msg = rx.recv() => match msg {
                        Some(s) => s,
                        None => break,
                    },
                };
                let mut convo =
                    Conversation::new(id.clone(), config.system_prompt.clone(), 200_000);
                convo.push(Message::user(seed));
                let mut cost = CostAccumulator::default();
                let _ = crab_engine::query_loop(
                    &mut convo,
                    &backend,
                    &exec,
                    &tool_ctx,
                    &loop_config,
                    &mut cost,
                    event_tx.clone(),
                    cancel.clone(),
                )
                .await;
            }
        })
    })
}

/// Format a finished worker's output as an `<agent-result>` user message the
/// parent model sees on its next turn.
#[must_use]
pub fn agent_result_message(result: &WorkerResult) -> Message {
    let status = if result.success {
        "completed"
    } else {
        "failed"
    };
    let output = result.output.as_deref().unwrap_or("(no output)");
    Message::user(format!(
        "<agent-result worker-id=\"{}\" status=\"{status}\">\n{output}\n</agent-result>",
        result.worker_id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convo_with_marker(marker: &str) -> Conversation {
        let mut c = Conversation::new("t".into(), String::new(), 1000);
        c.push(Message::tool_result("tu_1", marker, false));
        c
    }

    #[test]
    fn scan_finds_spawn_marker() {
        let marker = r#"{"action":"spawn_agent","task":"do x"}"#;
        let c = convo_with_marker(marker);
        let found = scan_spawn_markers(&c, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["task"], "do x");
    }

    #[test]
    fn scan_ignores_other_markers_and_respects_start() {
        let c = convo_with_marker(r#"{"action":"team_created","team_name":"x"}"#);
        assert!(scan_spawn_markers(&c, 0).is_empty());
        // starting_len past the message skips it.
        let c2 = convo_with_marker(r#"{"action":"spawn_agent","task":"y"}"#);
        assert!(scan_spawn_markers(&c2, 1).is_empty());
    }

    #[test]
    fn explore_agent_def_filters_to_read_only_tools() {
        let parent = crab_tools::builtin::create_default_registry();
        let explore = builtin_agents()
            .into_iter()
            .find(|d| d.agent_type == "Explore")
            .expect("Explore agent");
        let filtered = build_def_registry(&parent, &explore);
        let names = filtered.tool_names();
        // A read-only agent keeps read tools but loses write/exec tools.
        assert!(names.contains(&"Read"), "Read should survive: {names:?}");
        assert!(!names.contains(&"Write"), "Write must be dropped");
        assert!(!names.contains(&"Edit"), "Edit must be dropped");
        assert!(!names.contains(&"Bash"), "Bash must be dropped");
    }

    #[test]
    fn agent_result_message_formats_status_and_output() {
        let ok = WorkerResult {
            worker_id: "worker_1".into(),
            output: Some("the answer".into()),
            success: true,
            usage: crab_core::model::TokenUsage::default(),
            conversation: Conversation::new("w".into(), String::new(), 0),
        };
        let msg = ok.success.then(|| agent_result_message(&ok)).unwrap();
        let text = msg.text();
        assert!(text.contains("worker-id=\"worker_1\""));
        assert!(text.contains("status=\"completed\""));
        assert!(text.contains("the answer"));

        let failed = WorkerResult {
            worker_id: "worker_2".into(),
            output: None,
            success: false,
            usage: crab_core::model::TokenUsage::default(),
            conversation: Conversation::new("w2".into(), String::new(), 0),
        };
        let text = agent_result_message(&failed).text();
        assert!(text.contains("status=\"failed\""));
        assert!(text.contains("(no output)"));
    }
}
