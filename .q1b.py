def patch(path, edits):
    s = open(path, encoding='utf-8').read()
    for old, new in edits:
        assert old in s, f'{path}: {old[:100]!r}'
        s = s.replace(old, new, 1)
    open(path, 'w', encoding='utf-8').write(s)


patch('crates/agents/src/teams/spawn.rs', [
    ("""/// Everything the team runner shares with each teammate it spawns.
#[derive(Clone)]
pub struct TeamHandles {
    /// Where a teammate publishes the result of each completed task.
    pub results_tx: mpsc::Sender<WorkerResult>,
    /// The session's permission handler, or `None` for a non-interactive
    /// session with no user to ask.
    pub permission: Option<Arc<dyn PermissionHandler>>,
    /// Coalesces concurrent permission requests across the team.
    pub permission_sync: Arc<PermissionSyncManager>,
}""",
     """/// A team marker a teammate emitted during its own turn.
///
/// Teammates run in spawned tasks and cannot reach the team runner, so the
/// markers their tools produce travel back over a channel the same way results
/// do. Without this a teammate's `SendMessage` would land in its own
/// conversation and be read by nobody.
#[derive(Debug, Clone)]
pub struct TeammateMarker {
    /// The id of the teammate that emitted it — the `from` the routing rules
    /// reason about.
    pub from: String,
    /// The parsed marker payload.
    pub marker: serde_json::Value,
}

/// Everything the team runner shares with each teammate it spawns.
#[derive(Clone)]
pub struct TeamHandles {
    /// Where a teammate publishes the result of each completed task.
    pub results_tx: mpsc::Sender<WorkerResult>,
    /// Where a teammate publishes the team markers its own tools emitted.
    pub markers_tx: mpsc::Sender<TeammateMarker>,
    /// The session's permission handler, or `None` for a non-interactive
    /// session with no user to ask.
    pub permission: Option<Arc<dyn PermissionHandler>>,
    /// Coalesces concurrent permission requests across the team.
    pub permission_sync: Arc<PermissionSyncManager>,
}"""),

    ("use crab_team::roster::Lifetime;",
     "use crab_team::roster::{Capability, Lifetime};"),

    ("""/// Ephemeral teammates take their seed task, run once, publish a
/// [`WorkerResult`] on `results_tx`, and exit. Resident teammates keep their
/// conversation and loop, running one turn per inbound message until
/// cancelled — so `SendMessage` genuinely continues an agent rather than
/// restarting it.""",
     """/// Ephemeral teammates take their seed task, run once, publish a
/// [`WorkerResult`], and exit. Resident teammates keep their conversation and
/// loop, running one turn per inbound message until cancelled — so
/// `SendMessage` genuinely continues an agent rather than restarting it.
///
/// Either way the teammate's own conversation is scanned after each turn and
/// any team markers it produced are sent back, so a teammate's `SendMessage`
/// reaches the router instead of dying in a conversation nobody reads."""),

    ("""        let results_tx = handles.results_tx.clone();
        let permission = handles.permission.clone();""",
     """        let results_tx = handles.results_tx.clone();
        let markers_tx = handles.markers_tx.clone();
        let permission = handles.permission.clone();"""),

    ("""            if config.lifetime.is_ephemeral() {
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
            }""",
     """            // Both lifetimes run turns against a conversation the runner
            // owns, so the post-turn marker scan is identical either way.
            let mut conversation = worker.new_conversation();
            let ephemeral = config.lifetime.is_ephemeral();

            while let Some(task) = recv_or_cancel(&mut rx, &cancel).await {
                let turn_start = conversation.messages().len();
                let result = worker.run_turn(&mut conversation, task).await;

                for marker in scan_team_markers(&conversation, turn_start) {
                    let sent = markers_tx
                        .send(TeammateMarker {
                            from: id.clone(),
                            marker,
                        })
                        .await;
                    if sent.is_err() {
                        break;
                    }
                }
                let _ = results_tx.send(result).await;

                if ephemeral {
                    // One task, and the teammate is done.
                    return;
                }
            }"""),

    ("""    let registry = match agent_definition(&config.role) {
        Some(def) => Arc::new(build_def_registry(base_registry, &def)),
        None => Arc::clone(base_registry),
    };""",
     """    let mut registry = match agent_definition(&config.role) {
        Some(def) => build_def_registry(base_registry, &def),
        None => clone_registry(base_registry),
    };
    // A teammate that could spawn teammates would recurse with nothing
    // bounding the depth — neither the roster nor the job registry caps it.
    // Delegation stays with the main agent.
    registry.remove_names(TEAMMATE_DENIED_TOOLS);
    let registry = Arc::new(registry);"""),

    ("""/// Look up a built-in agent definition by `subagent_type`.""",
     """/// Tools no spawned teammate may use, whatever its agent definition allows.
///
/// `Agent` is denied because a teammate spawning teammates recurses without a
/// depth bound. Peer messaging is *not* denied here: `Team::can_communicate`
/// already decides who may talk to whom, and Coordinator Mode strips
/// `SendMessage` from its workers separately.
pub(crate) const TEAMMATE_DENIED_TOOLS: &[&str] = &[crab_tools::builtin::agent::AGENT_TOOL_NAME];

/// Copy a registry so the caller can filter it without touching the parent's.
fn clone_registry(parent: &ToolRegistry) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for name in parent.tool_names() {
        if let Some(tool) = parent.get(name) {
            reg.register(Arc::clone(tool));
        }
    }
    reg
}

/// Look up a built-in agent definition by `subagent_type`."""),
])

patch('crates/agents/src/teams/worker.rs', [
    ("""    /// Run a single task on a fresh conversation — the ephemeral path.
    pub async fn run_once(&self, task_prompt: String) -> WorkerResult {
        let mut conversation = self.new_conversation();
        self.run_turn(&mut conversation, task_prompt).await
    }

""", ""),
    ("""/// One worker serves both lifetimes: [`AgentWorker::run_turn`] takes `&self`
/// and an existing conversation, so a resident teammate reuses it across
/// messages and keeps its context, while an ephemeral teammate calls it once
/// on a fresh conversation via [`AgentWorker::run_once`].""",
     """/// One worker serves both lifetimes: [`AgentWorker::run_turn`] takes `&self`
/// and an existing conversation, so a resident teammate reuses it across
/// messages and keeps its context while an ephemeral one runs a single turn.
/// The caller owns the conversation either way, which is what lets it scan the
/// teammate's own output for team markers after each turn."""),
])
print('ok')
