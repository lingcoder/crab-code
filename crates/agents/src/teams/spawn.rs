//! Turning `Agent` tool markers into teammate spawns.
//!
//! The Agent tool emits a `spawn_agent` marker into its tool-result text.
//! A marker carrying a non-empty `name` spawns a [`Lifetime::Resident`]
//! teammate (addressable, keeps its conversation); one without a name spawns
//! a [`Lifetime::Ephemeral`] teammate whose result folds back into the parent
//! conversation as an `<agent-result>` message.
//!
//! Both go through the same [`TeammateConfig`] and the same runner, so an
//! agent definition's registry filter, model override, permission restriction,
//! and turn limits apply identically regardless of lifetime.

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message};
use crab_core::model::ModelId;
use crab_core::tool::ToolContext;
use crab_engine::QueryConfig;
use crab_session::Conversation;
use crab_team::backend::{TeammateConfig, TeammateRunCtx, TeammateRunner};
use crab_team::bus::{AgentMessage, Envelope};
use crab_team::roster::Lifetime;
use crab_tools::executor::{PermissionHandler, ToolExecutor};
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;

use super::permission::TeamPermissionHandler;
use super::worker::{AgentWorker, WorkerConfig, WorkerResult};
use crate::builtin::builtin_agents;
use crate::coordinator::PermissionSyncManager;
use crate::definition::{AgentDefinition, ToolSet};

/// The marker `action` value the Agent tool emits for a spawn request.
pub const SPAWN_AGENT_ACTION: &str = "spawn_agent";

/// The marker `action` value the `SendMessage` tool emits.
pub const MESSAGE_SENT_ACTION: &str = "message_sent";

/// Scan `conversation[starting_len..]` tool-result text for markers the team
/// runner cares about (`spawn_agent` and `message_sent`).
#[must_use]
pub fn scan_team_markers(
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
        .filter(|v| {
            matches!(
                v.get("action").and_then(serde_json::Value::as_str),
                Some(SPAWN_AGENT_ACTION | MESSAGE_SENT_ACTION)
            )
        })
        .collect()
}

/// The addressable name a spawn marker requests, if any.
///
/// An absent, null, or whitespace-only `name` means an ephemeral spawn.
#[must_use]
pub fn marker_name(marker: &serde_json::Value) -> Option<&str> {
    marker
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
}

/// Build a [`TeammateConfig`] from a parsed `spawn_agent` marker.
///
/// `base_prompt` is the session's system prompt, inherited beneath a short
/// preamble when the marker names no known agent definition. Returns `None`
/// when the marker is not a spawn request or carries no task.
#[must_use]
pub fn teammate_config_from_marker(
    marker: &serde_json::Value,
    base_prompt: &str,
) -> Option<TeammateConfig> {
    if marker.get("action").and_then(serde_json::Value::as_str)? != SPAWN_AGENT_ACTION {
        return None;
    }
    let task = marker.get("task").and_then(serde_json::Value::as_str)?;

    let name = marker_name(marker).unwrap_or_default().to_owned();
    let resident = !name.is_empty();

    let lifetime = if resident {
        Lifetime::Resident
    } else {
        Lifetime::Ephemeral {
            max_turns: marker
                .get("max_turns")
                .and_then(serde_json::Value::as_u64)
                .map(|v| usize::try_from(v).unwrap_or(usize::MAX)),
            max_duration: None,
        }
    };

    let subagent_type = marker
        .get("subagent_type")
        .and_then(serde_json::Value::as_str);
    let def = subagent_type.and_then(agent_definition);

    // A named agent definition supplies the system prompt; otherwise inherit
    // the parent's prompt beneath a short role preamble.
    let system_prompt = match &def {
        Some(d) => d.system_prompt.clone(),
        None if resident => format!(
            "You are teammate \"{name}\" in this session's agent team. Complete each \
             task you receive and report concise results.\n\n{base_prompt}"
        ),
        None => format!("You are a sub-agent worker. Complete the assigned task.\n\n{base_prompt}"),
    };

    let role = subagent_type
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(if resident { "teammate" } else { "worker" });

    let mut config = TeammateConfig::new(name, role, lifetime)
        .with_system_prompt(system_prompt)
        .with_seed_task(task);

    if let Some(model) = def.as_ref().and_then(|d| d.model.clone()) {
        config = config.with_model(model);
    }
    if let Some(mode) = marker
        .get("parent_permission_mode")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<crab_core::permission::PermissionMode>().ok())
    {
        config = config.with_parent_permission_mode(mode);
    }
    if let Some(wd) = marker
        .get("working_dir")
        .and_then(serde_json::Value::as_str)
    {
        config = config.with_working_dir(std::path::PathBuf::from(wd));
    }
    Some(config)
}

/// Everything the team runner shares with each teammate it spawns.
#[derive(Clone)]
pub struct TeamHandles {
    /// Where a teammate publishes the result of each completed task.
    pub results_tx: mpsc::Sender<WorkerResult>,
    /// The session's permission handler, or `None` for a non-interactive
    /// session with no user to ask.
    pub permission: Option<Arc<dyn PermissionHandler>>,
    /// Coalesces concurrent permission requests across the team.
    pub permission_sync: Arc<PermissionSyncManager>,
}

/// The addressable name for a teammate: its own name, or its id when the
/// spawn was unnamed.
fn display_name(id: &str, config: &TeammateConfig) -> String {
    if config.name.trim().is_empty() {
        id.to_owned()
    } else {
        config.name.clone()
    }
}

/// Look up a built-in agent definition by `subagent_type`.
fn agent_definition(agent_type: &str) -> Option<AgentDefinition> {
    builtin_agents()
        .into_iter()
        .find(|d| d.agent_type == agent_type)
}

/// Build a worker tool registry from `parent`, applying an agent definition's
/// restrictions: read-only agents keep only read-only tools, an explicit
/// `ToolSet::Specific` allow-list is retained, and `disallowed_tools` are
/// removed.
pub(crate) fn build_def_registry(parent: &ToolRegistry, def: &AgentDefinition) -> ToolRegistry {
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

/// Build the runner that drives every spawned teammate, regardless of
/// lifetime.
///
/// Ephemeral teammates take their seed task, run once, publish a
/// [`WorkerResult`] on `results_tx`, and exit. Resident teammates keep their
/// conversation and loop, running one turn per inbound message until
/// cancelled — so `SendMessage` genuinely continues an agent rather than
/// restarting it.
pub fn teammate_runner(
    backend: Arc<LlmBackend>,
    registry: Arc<ToolRegistry>,
    tool_ctx: ToolContext,
    loop_config: QueryConfig,
    event_tx: mpsc::Sender<Event>,
    handles: TeamHandles,
) -> TeammateRunner {
    Arc::new(move |ctx: TeammateRunCtx| {
        let backend = Arc::clone(&backend);
        let base_registry = Arc::clone(&registry);
        let base_ctx = tool_ctx.clone();
        let base_config = loop_config.clone();
        let event_tx = event_tx.clone();
        let results_tx = handles.results_tx.clone();
        let permission = handles.permission.clone();
        let permission_sync = Arc::clone(&handles.permission_sync);

        Box::pin(async move {
            let TeammateRunCtx {
                id,
                config,
                mut rx,
                cancel,
            } = ctx;

            let worker = build_worker(
                &id,
                &config,
                &backend,
                &base_registry,
                &base_ctx,
                &base_config,
                &event_tx,
                &cancel,
                permission.map(|session| {
                    Arc::new(TeamPermissionHandler::new(
                        display_name(&id, &config),
                        session,
                        permission_sync,
                    )) as Arc<dyn PermissionHandler>
                }),
            );

            if config.lifetime.is_ephemeral() {
                // The seed task was delivered at spawn time; take it and run.
                let Some(task) = recv_or_cancel(&mut rx, &cancel).await else {
                    return;
                };
                let result = worker.run_once(task).await;
                let _ = results_tx.send(result).await;
                return;
            }

            // Resident: one conversation, one turn per inbound message.
            let mut conversation = worker.new_conversation();
            while let Some(task) = recv_or_cancel(&mut rx, &cancel).await {
                let result = worker.run_turn(&mut conversation, task).await;
                let _ = results_tx.send(result).await;
            }
        })
    })
}

/// Await the next task from the teammate's mailbox, returning `None` when
/// cancelled, closed, or asked to shut down.
///
/// Envelopes that carry no work for the agent loop (status updates, responses)
/// are skipped rather than treated as prompts.
async fn recv_or_cancel(
    rx: &mut mpsc::Receiver<Envelope>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<String> {
    loop {
        let envelope = tokio::select! {
            () = cancel.cancelled() => return None,
            msg = rx.recv() => msg?,
        };
        match envelope.payload {
            AgentMessage::AssignTask { prompt, .. } => return Some(prompt),
            AgentMessage::Request { body, .. } => return Some(body),
            AgentMessage::Shutdown => return None,
            other => {
                tracing::debug!(payload = ?other, "teammate ignored a non-task message");
            }
        }
    }
}

/// Assemble the per-teammate agent loop from its spawn config.
///
/// This is where an agent definition's registry filter, the model override,
/// and the parent permission restriction are applied — uniformly for both
/// lifetimes.
#[allow(clippy::too_many_arguments)]
fn build_worker(
    id: &str,
    config: &TeammateConfig,
    backend: &Arc<LlmBackend>,
    base_registry: &Arc<ToolRegistry>,
    base_ctx: &ToolContext,
    base_config: &QueryConfig,
    event_tx: &mpsc::Sender<Event>,
    cancel: &tokio_util::sync::CancellationToken,
    permission: Option<Arc<dyn PermissionHandler>>,
) -> AgentWorker {
    let registry = match agent_definition(&config.role) {
        Some(def) => Arc::new(build_def_registry(base_registry, &def)),
        None => Arc::clone(base_registry),
    };

    // A teammate has no terminal of its own, so it asks through the session's
    // handler and the user answers once for the whole team. Only when the
    // session itself is non-interactive does it fall back to unattended
    // approval — the spawn was user-confirmed and the permission mode below is
    // still capped at the parent's.
    let mut exec = ToolExecutor::new(registry);
    match permission {
        Some(handler) => exec.set_permission_handler(handler),
        None => exec.set_allow_unattended(true),
    }

    let mut tool_ctx = base_ctx.clone();
    if let Some(mode) = config.parent_permission_mode {
        tool_ctx.permission_mode = tool_ctx.permission_mode.restrict_to(mode);
    }
    if let Some(wd) = &config.working_dir {
        tool_ctx.working_dir.clone_from(wd);
    }

    // A teammate runs an independent conversation, so it must not append to
    // the parent session's JSONL crash log.
    let mut loop_config = base_config.clone();
    loop_config.session_persister = None;
    if let Some(model) = &config.model {
        loop_config.model = ModelId::from(model.clone());
    }

    AgentWorker::new(
        WorkerConfig {
            worker_id: id.to_owned(),
            system_prompt: config.system_prompt.clone(),
            max_turns: config.lifetime.max_turns(),
            max_duration: config.lifetime.max_duration(),
            context_window: config.context_window,
        },
        display_name(id, config),
        Arc::clone(backend),
        Arc::new(exec),
        tool_ctx,
        loop_config,
        event_tx.clone(),
        cancel.clone(),
    )
}

/// Format a finished teammate's output as an `<agent-result>` user message the
/// parent model sees on its next turn.
#[must_use]
pub fn agent_result_message(result: &WorkerResult) -> Message {
    let status = if result.success {
        "completed"
    } else {
        "failed"
    };
    // Prefer real output; on failure with no output, surface the error so the
    // parent can tell a crashed teammate from one that produced nothing.
    let output = result
        .output
        .as_deref()
        .or(result.error.as_deref())
        .unwrap_or("(no output)");
    Message::user(format!(
        "<agent-result worker-id=\"{}\" name=\"{}\" status=\"{status}\">\n{output}\n</agent-result>",
        result.worker_id, result.name,
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
    fn scan_finds_spawn_and_message_markers() {
        let c = convo_with_marker(r#"{"action":"spawn_agent","task":"do x"}"#);
        let found = scan_team_markers(&c, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["task"], "do x");

        let c = convo_with_marker(r#"{"action":"message_sent","to":"alice","message":"hi"}"#);
        assert_eq!(scan_team_markers(&c, 0).len(), 1);
    }

    #[test]
    fn scan_ignores_unrelated_markers_and_respects_start() {
        let c = convo_with_marker(r#"{"action":"something_else"}"#);
        assert!(scan_team_markers(&c, 0).is_empty());

        let c = convo_with_marker(r#"{"action":"spawn_agent","task":"y"}"#);
        assert!(scan_team_markers(&c, 1).is_empty());
    }

    #[test]
    fn scan_keeps_named_spawns() {
        // Named spawns are no longer filtered out: one path handles both.
        let named = convo_with_marker(r#"{"action":"spawn_agent","task":"y","name":"alice"}"#);
        assert_eq!(scan_team_markers(&named, 0).len(), 1);
    }

    #[test]
    fn marker_name_treats_blank_as_absent() {
        let named = serde_json::json!({"name": "alice"});
        assert_eq!(marker_name(&named), Some("alice"));

        for blank in [
            serde_json::json!({}),
            serde_json::json!({ "name": null }),
            serde_json::json!({"name": "  "}),
        ] {
            assert_eq!(marker_name(&blank), None, "{blank} should be unnamed");
        }
    }

    #[test]
    fn unnamed_marker_builds_ephemeral_config() {
        let marker = serde_json::json!({
            "action": "spawn_agent",
            "task": "do x",
            "max_turns": 5,
        });
        let config = teammate_config_from_marker(&marker, "BASE").unwrap();
        assert!(config.name.is_empty());
        assert_eq!(config.role, "worker");
        assert_eq!(config.seed_task, "do x");
        assert_eq!(config.lifetime.max_turns(), Some(5));
        assert!(config.lifetime.is_ephemeral());
        assert!(config.system_prompt.contains("sub-agent worker"));
        assert!(config.system_prompt.contains("BASE"));
    }

    #[test]
    fn named_marker_builds_resident_config() {
        let marker = serde_json::json!({
            "action": "spawn_agent",
            "task": "review it",
            "name": "alice",
        });
        let config = teammate_config_from_marker(&marker, "BASE").unwrap();
        assert_eq!(config.name, "alice");
        assert_eq!(config.role, "teammate");
        assert_eq!(config.lifetime, Lifetime::Resident);
        assert!(config.system_prompt.contains("teammate \"alice\""));
        assert!(config.system_prompt.contains("BASE"));
    }

    #[test]
    fn max_turns_is_ignored_for_resident_spawns() {
        let marker = serde_json::json!({
            "action": "spawn_agent",
            "task": "t",
            "name": "alice",
            "max_turns": 3,
        });
        let config = teammate_config_from_marker(&marker, "BASE").unwrap();
        assert_eq!(config.lifetime, Lifetime::Resident);
        assert_eq!(config.lifetime.max_turns(), None);
    }

    #[test]
    fn subagent_type_supplies_prompt_and_role_for_both_lifetimes() {
        for name in [serde_json::Value::Null, serde_json::json!("alice")] {
            let marker = serde_json::json!({
                "action": "spawn_agent",
                "task": "search",
                "subagent_type": "Explore",
                "name": name,
            });
            let config = teammate_config_from_marker(&marker, "BASE").unwrap();
            assert_eq!(config.role, "Explore");
            // The definition's prompt replaces the inherited one.
            assert!(!config.system_prompt.contains("BASE"));
        }
    }

    #[test]
    fn parent_permission_mode_is_carried_into_config() {
        let marker = serde_json::json!({
            "action": "spawn_agent",
            "task": "t",
            "parent_permission_mode": "plan",
        });
        let config = teammate_config_from_marker(&marker, "BASE").unwrap();
        assert_eq!(
            config.parent_permission_mode,
            Some(crab_core::permission::PermissionMode::Plan)
        );
    }

    #[test]
    fn non_spawn_or_taskless_markers_build_nothing() {
        let wrong_action = serde_json::json!({"action": "message_sent", "task": "x"});
        assert!(teammate_config_from_marker(&wrong_action, "BASE").is_none());

        let no_task = serde_json::json!({"action": "spawn_agent"});
        assert!(teammate_config_from_marker(&no_task, "BASE").is_none());
    }

    #[test]
    fn explore_agent_def_filters_to_read_only_tools() {
        let parent = crab_tools::builtin::create_default_registry();
        let explore = agent_definition("Explore").expect("Explore agent");
        let filtered = build_def_registry(&parent, &explore);
        let names = filtered.tool_names();
        assert!(names.contains(&"Read"), "Read should survive: {names:?}");
        assert!(!names.contains(&"Write"), "Write must be dropped");
        assert!(!names.contains(&"Edit"), "Edit must be dropped");
        assert!(!names.contains(&"Bash"), "Bash must be dropped");
    }

    #[test]
    fn agent_result_message_formats_status_and_output() {
        let ok = WorkerResult {
            worker_id: "ip-0".into(),
            name: "alice".into(),
            output: Some("the answer".into()),
            success: true,
            error: None,
            usage: crab_core::model::TokenUsage::default(),
        };
        let text = agent_result_message(&ok).text();
        assert!(text.contains("worker-id=\"ip-0\""));
        assert!(text.contains("name=\"alice\""));
        assert!(text.contains("status=\"completed\""));
        assert!(text.contains("the answer"));

        let failed = WorkerResult {
            worker_id: "ip-1".into(),
            name: "ip-1".into(),
            output: None,
            success: false,
            error: Some("backend exploded".into()),
            usage: crab_core::model::TokenUsage::default(),
        };
        let text = agent_result_message(&failed).text();
        assert!(text.contains("status=\"failed\""));
        assert!(text.contains("backend exploded"));
    }
}
