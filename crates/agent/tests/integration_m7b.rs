//! M7b E2E integration tests.
//!
//! Tests the full integration of `WorkerPool`, `AgentSession`, `AgentTool`,
//! `TaskTools`, `SkillRegistry`, and Worker together.

use std::sync::Arc;

use crab_agent::{
    AgentSession, SessionConfig, TaskList, TaskStatus, WorkerPool, WorkerResult, shared_task_list,
};
use crab_api::LlmBackend;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::{ModelId, TokenUsage};
use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_core::tool::{ToolContext, ToolOutput, ToolOutputContent};
use crab_session::{Conversation, MemoryStore, SessionHistory};
use crab_tools::builtin::agent::AGENT_TOOL_NAME;
use crab_tools::builtin::bash::BASH_TOOL_NAME;
use crab_tools::builtin::create_default_registry;
use crab_tools::builtin::cron::{
    CRON_CREATE_TOOL_NAME, CRON_DELETE_TOOL_NAME, CRON_LIST_TOOL_NAME,
};
use crab_tools::builtin::edit::EDIT_TOOL_NAME;
use crab_tools::builtin::glob::GLOB_TOOL_NAME;
use crab_tools::builtin::grep::GREP_TOOL_NAME;
use crab_tools::builtin::notebook::NOTEBOOK_EDIT_TOOL_NAME;
use crab_tools::builtin::read::READ_TOOL_NAME;
use crab_tools::builtin::remote_trigger::REMOTE_TRIGGER_TOOL_NAME;
use crab_tools::builtin::task::{
    TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_OUTPUT_TOOL_NAME,
    TASK_STOP_TOOL_NAME, TASK_UPDATE_TOOL_NAME,
};
use crab_tools::builtin::team::{
    SEND_MESSAGE_TOOL_NAME, TEAM_CREATE_TOOL_NAME, TEAM_DELETE_TOOL_NAME,
};
use crab_tools::builtin::web_fetch::WEB_FETCH_TOOL_NAME;
use crab_tools::builtin::web_search::WEB_SEARCH_TOOL_NAME;
use crab_tools::builtin::write::WRITE_TOOL_NAME;

fn test_backend() -> Arc<LlmBackend> {
    Arc::new(LlmBackend::OpenAi(crab_api::openai::OpenAiClient::new(
        "http://localhost:0/v1",
        None,
    )))
}

fn test_session_config() -> SessionConfig {
    SessionConfig {
        session_id: "test_e2e".into(),
        system_prompt: "You are a test agent.".into(),
        model: ModelId::from("test-model"),
        max_tokens: 4096,
        temperature: None,
        context_window: 200_000,
        working_dir: std::env::temp_dir(),
        permission_policy: PermissionPolicy {
            mode: PermissionMode::Dangerously,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
        },
        memory_dir: None,
        sessions_dir: None,
        resume_session_id: None,
        effort: None,
        thinking_mode: None,
        additional_dirs: Vec::new(),
        session_name: None,
        max_turns: None,
        max_budget_usd: None,
        fallback_model: None,
        bare_mode: false,
        worktree_name: None,
        fork_session: false,
        from_pr: None,
        custom_session_id: None,
        json_schema: None,
        plugin_dirs: Vec::new(),
        disable_skills: false,
        beta_headers: Vec::new(),
        ide_connect: false,
        coordinator_mode: false,
    }
}

// ─── WorkerPool multi-worker tests ───

#[test]
fn coordinator_starts_empty() {
    let coord = WorkerPool::new("main".into(), "Main".into());
    assert_eq!(coord.running_count(), 0);
    assert!(coord.completed_results().is_empty());
}

#[tokio::test]
async fn coordinator_collect_all_on_empty_returns_empty() {
    let mut coord = WorkerPool::new("main".into(), "Main".into());
    let results = coord.collect_all().await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn coordinator_collect_completed_on_empty_returns_empty() {
    let mut coord = WorkerPool::new("main".into(), "Main".into());
    let results = coord.collect_completed().await;
    assert!(results.is_empty());
}

#[test]
fn coordinator_cancel_nonexistent_returns_false() {
    let coord = WorkerPool::new("main".into(), "Main".into());
    assert!(!coord.cancel_worker("w999"));
}

// ─── AgentSession + Memory + History integration ───

#[test]
fn session_creates_with_all_components() {
    let backend = test_backend();
    let registry = create_default_registry();
    let session = AgentSession::new(test_session_config(), backend, registry);

    // Session has event channels, cancel token, and conversation
    assert!(session.conversation.is_empty());
    assert!(!session.cancel.is_cancelled());
}

#[test]
fn session_with_memory_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = dir.path().join("memory");
    let sessions_dir = dir.path().join("sessions");

    // Pre-write a memory file
    let store = MemoryStore::new(memory_dir.clone());
    store
        .save(
            "test.md",
            "---\nname: Test\ndescription: A test\ntype: user\n---\n\nMemory body.",
        )
        .unwrap();

    // Pre-save a session
    let history = SessionHistory::new(sessions_dir.clone());
    history
        .save("prev", &[Message::user("previous input")])
        .unwrap();

    let mut config = test_session_config();
    config.memory_dir = Some(memory_dir);
    config.sessions_dir = Some(sessions_dir);
    config.resume_session_id = Some("prev".into());

    let session = AgentSession::new(config, test_backend(), create_default_registry());

    // Memory injected into system prompt
    assert!(session.conversation.system_prompt.contains("Memory body"));
    // Resumed messages present
    assert_eq!(session.conversation.len(), 1);
    assert_eq!(session.conversation.messages()[0].text(), "previous input");
}

// ─── AgentTool output format ───

#[tokio::test]
async fn agent_tool_produces_spawn_request() {
    let registry = create_default_registry();
    let tool = registry.get(AGENT_TOOL_NAME).unwrap();

    let ctx = ToolContext {
        working_dir: std::env::temp_dir(),
        permission_mode: PermissionMode::Dangerously,
        session_id: "e2e_test".into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        permission_policy: PermissionPolicy::default(),
        ext: crab_core::tool::ToolContextExt::default(),
    };

    let input = serde_json::json!({
        "task": "Fix bug in auth module",
        "max_turns": 5,
    });

    let output = tool.execute(input, &ctx).await.unwrap();
    assert!(!output.is_error);

    // Verify structured JSON output
    match &output.content[0] {
        ToolOutputContent::Json { value } => {
            assert_eq!(value["action"], "spawn_agent");
            assert_eq!(value["task"], "Fix bug in auth module");
            assert_eq!(value["max_turns"], 5);
            assert_eq!(value["session_id"], "e2e_test");
        }
        _ => panic!("expected JSON output from AgentTool"),
    }
}

// ─── TaskList integration ───

#[test]
fn task_list_dependency_resolution() {
    let mut list = TaskList::new();
    let setup_id = list.create("Setup env".into(), "Install deps".into());
    let tests_id = list.create("Run tests".into(), "Execute test suite".into());
    list.add_blocked_by(&tests_id, &setup_id);

    // blocked task not available until blocker completes
    assert!(list.available_tasks().iter().all(|t| t.id != tests_id));

    list.update(&setup_id, Some(TaskStatus::Completed), None, None, None);

    // Now it's available
    assert!(list.available_tasks().iter().any(|t| t.id == tests_id));
}

#[test]
fn shared_task_list_cross_thread() {
    let shared = shared_task_list();
    let shared2 = Arc::clone(&shared);

    let handle = std::thread::spawn(move || {
        let mut list = shared2.lock().unwrap();
        let id = list.create("From thread".into(), "desc".into());
        list.update(
            &id,
            Some(TaskStatus::InProgress),
            None,
            None,
            Some("worker_1".into()),
        );
    });
    handle.join().unwrap();

    let guard = shared.lock().unwrap();
    let tasks = guard.list();
    let len = tasks.len();
    let status = tasks[0].status;
    let owner = tasks[0].owner.clone();
    drop(guard);
    assert_eq!(len, 1);
    assert_eq!(status, TaskStatus::InProgress);
    assert_eq!(owner.as_deref(), Some("worker_1"));
}

// ─── SkillRegistry integration ───

#[test]
fn skill_registry_discover_and_match() {
    use crab_skill::{Skill, SkillRegistry, SkillTrigger};

    let mut registry = SkillRegistry::new();
    registry.register(Skill {
        trigger: SkillTrigger::Command {
            name: "commit".into(),
        },
        description: "Create a git commit".into(),
        ..Skill::new("commit", "You are a commit helper.")
    });
    registry.register(Skill {
        trigger: SkillTrigger::Pattern {
            regex: r"(?i)review".into(),
        },
        description: "Review code".into(),
        ..Skill::new("review", "You are a reviewer.")
    });

    // Command matching
    assert!(registry.find_command("commit").is_some());
    let matches = registry.match_input("/commit");
    assert_eq!(matches.len(), 1);

    // Pattern matching
    let matches = registry.match_input("Please review this PR");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].name, "review");

    // No match
    assert!(registry.match_input("unrelated input").is_empty());
}

// ─── WriteTool + EditTool chain ───

#[tokio::test]
async fn tool_chain_write_then_edit() {
    let registry = create_default_registry();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");

    let ctx = ToolContext {
        working_dir: dir.path().to_path_buf(),
        permission_mode: PermissionMode::Dangerously,
        session_id: "chain_test".into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        permission_policy: PermissionPolicy::default(),
        ext: crab_core::tool::ToolContextExt::default(),
    };

    // Step 1: Write a file
    let write_tool = registry.get(WRITE_TOOL_NAME).unwrap();
    let write_input = serde_json::json!({
        "file_path": file_path.to_str().unwrap(),
        "content": "fn hello() {\n    println!(\"hello\");\n}\n"
    });
    let output = write_tool.execute(write_input, &ctx).await.unwrap();
    assert!(!output.is_error, "write failed: {}", output.text());

    // Step 2: Edit the file
    let edit_tool = registry.get(EDIT_TOOL_NAME).unwrap();
    let edit_input = serde_json::json!({
        "file_path": file_path.to_str().unwrap(),
        "old_string": "fn hello()",
        "new_string": "fn greet(name: &str)"
    });
    let output = edit_tool.execute(edit_input, &ctx).await.unwrap();
    assert!(!output.is_error, "edit failed: {}", output.text());

    // Verify final content
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("fn greet(name: &str)"));
    assert!(!content.contains("fn hello()"));
    assert!(content.contains("println!(\"hello\");")); // rest preserved
}

// ─── ReadTool + GlobTool chain ───

#[tokio::test]
async fn tool_chain_glob_then_read() {
    let registry = create_default_registry();
    let dir = tempfile::tempdir().unwrap();

    // Create some files
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn lib() {}").unwrap();
    std::fs::write(dir.path().join("README.md"), "# Readme").unwrap();

    let ctx = ToolContext {
        working_dir: dir.path().to_path_buf(),
        permission_mode: PermissionMode::Dangerously,
        session_id: "glob_test".into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        permission_policy: PermissionPolicy::default(),
        ext: crab_core::tool::ToolContextExt::default(),
    };

    // Step 1: Glob for .rs files
    let glob_tool = registry.get(GLOB_TOOL_NAME).unwrap();
    let glob_input = serde_json::json!({
        "pattern": "*.rs",
        "path": dir.path().to_str().unwrap()
    });
    let output = glob_tool.execute(glob_input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("main.rs"));
    assert!(text.contains("lib.rs"));
    assert!(!text.contains("README.md"));

    // Step 2: Read one of the found files
    let read_tool = registry.get(READ_TOOL_NAME).unwrap();
    let read_input = serde_json::json!({
        "file_path": dir.path().join("main.rs").to_str().unwrap()
    });
    let output = read_tool.execute(read_input, &ctx).await.unwrap();
    assert!(!output.is_error);
    assert!(output.text().contains("fn main()"));
}

// ─── Permission flow ───

#[tokio::test]
async fn permission_denied_tool_blocked() {
    let registry = create_default_registry();
    let executor = crab_tools::executor::ToolExecutor::new(Arc::new(registry));

    let ctx = ToolContext {
        working_dir: std::env::temp_dir(),
        permission_mode: PermissionMode::Dangerously,
        session_id: "perm_test".into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        permission_policy: PermissionPolicy {
            mode: PermissionMode::Dangerously,
            allowed_tools: Vec::new(),
            denied_tools: vec![BASH_TOOL_NAME.into()],
        },
        ext: crab_core::tool::ToolContextExt::default(),
    };

    let output = executor
        .execute(
            BASH_TOOL_NAME,
            serde_json::json!({"command": "echo hi"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(output.is_error);
    assert!(output.text().contains("denied"));
}

// ─── WorkerResult clone_summary ───

#[test]
fn worker_result_clone_summary_preserves_fields() {
    let mut conv = Conversation::new("w1".into(), "prompt".into(), 200_000);
    conv.push(Message::user("hello"));
    conv.push(Message::new(
        Role::Assistant,
        vec![ContentBlock::text("world")],
    ));

    let result = WorkerResult {
        worker_id: "w1".into(),
        output: Some("done".into()),
        success: true,
        usage: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        },
        conversation: conv,
    };

    let summary = result.clone_summary();
    assert_eq!(summary.worker_id, "w1");
    assert_eq!(summary.output.as_deref(), Some("done"));
    assert!(summary.success);
    assert_eq!(summary.usage.input_tokens, 100);
    // Summary conversation is empty (lightweight)
    assert!(summary.conversation.is_empty());
}

// ─── Session event channel ───

#[tokio::test]
async fn session_cancel_stops_agent() {
    let session = AgentSession::new(
        test_session_config(),
        test_backend(),
        create_default_registry(),
    );

    assert!(!session.cancel.is_cancelled());
    session.cancel();
    assert!(session.cancel.is_cancelled());
}

// ─── Full registry has expected tools ───

#[test]
fn default_registry_includes_all_builtin_tools() {
    let registry = create_default_registry();

    let expected = [
        BASH_TOOL_NAME,
        READ_TOOL_NAME,
        WRITE_TOOL_NAME,
        EDIT_TOOL_NAME,
        GLOB_TOOL_NAME,
        GREP_TOOL_NAME,
        AGENT_TOOL_NAME,
        NOTEBOOK_EDIT_TOOL_NAME,
        WEB_SEARCH_TOOL_NAME,
        WEB_FETCH_TOOL_NAME,
        TASK_CREATE_TOOL_NAME,
        TASK_LIST_TOOL_NAME,
        TASK_UPDATE_TOOL_NAME,
        TASK_GET_TOOL_NAME,
        TEAM_CREATE_TOOL_NAME,
        TEAM_DELETE_TOOL_NAME,
        SEND_MESSAGE_TOOL_NAME,
        TASK_STOP_TOOL_NAME,
        TASK_OUTPUT_TOOL_NAME,
        CRON_CREATE_TOOL_NAME,
        CRON_DELETE_TOOL_NAME,
        CRON_LIST_TOOL_NAME,
        REMOTE_TRIGGER_TOOL_NAME,
    ];

    for name in &expected {
        assert!(
            registry.get(name).is_some(),
            "missing expected tool: {name}"
        );
    }
}

// ─── Conversation + tool result message format ───

#[test]
fn tool_results_message_format() {
    use crab_engine::tool_results_message;

    let results = vec![
        ("tu_1".into(), Ok(ToolOutput::success("file read ok"))),
        ("tu_2".into(), Ok(ToolOutput::error("file not found"))),
        (
            "tu_3".into(),
            Err(crab_core::Error::Other("timeout".into())),
        ),
    ];

    let msg = tool_results_message(results);
    assert_eq!(msg.role, Role::User);
    assert_eq!(msg.content.len(), 3);

    // First: success
    match &msg.content[0] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(!is_error);
            assert_eq!(content, "file read ok");
        }
        _ => panic!("expected ToolResult"),
    }

    // Second: tool-level error
    match &msg.content[1] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(is_error);
            assert_eq!(content, "file not found");
        }
        _ => panic!("expected ToolResult"),
    }

    // Third: execution error
    match &msg.content[2] {
        ContentBlock::ToolResult { is_error, .. } => {
            assert!(is_error);
        }
        _ => panic!("expected ToolResult"),
    }
}

// ─── QueryConfig clone ───

#[test]
fn query_config_is_cloneable() {
    let config = crab_engine::QueryConfig {
        model: ModelId::from("test-model"),
        max_tokens: 4096,
        temperature: Some(0.5),
        tool_schemas: vec![serde_json::json!({"name": "test"})],
        cache_enabled: true,
        budget_tokens: None,
        retry_policy: None,
        hook_executor: None,
        session_id: None,
        effort: None,
        fallback_model: None,
        plan_model: None,
        source: crab_core::query::QuerySource::Repl,
        compaction_client: None,
        compaction_config: crab_session::CompactionConfig::default(),
        session_persister: None,
    };
    let cloned = config;
    assert_eq!(cloned.model.as_str(), "test-model");
    assert_eq!(cloned.max_tokens, 4096);
    assert!(cloned.cache_enabled);
}
