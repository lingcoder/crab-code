//! M3 Integration tests: tools + fs + process end-to-end verification.
//!
//! These tests exercise the full ToolRegistry → ToolExecutor → Tool.execute()
//! pipeline with real filesystem and process operations.

use std::path::Path;
use std::sync::Arc;

use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_core::tool::ToolContext;
use crab_tools::builtin::{create_default_registry, register_all_builtins};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use tokio_util::sync::CancellationToken;

// ─── Helpers ───

fn make_ctx(working_dir: &Path, mode: PermissionMode) -> ToolContext {
    ToolContext {
        working_dir: working_dir.to_path_buf(),
        permission_mode: mode,
        session_id: "integration-test".into(),
        cancellation_token: CancellationToken::new(),
        permission_policy: PermissionPolicy {
            mode,
            allowed_tools: vec![],
            denied_tools: vec![],
        },
    }
}

fn make_executor() -> ToolExecutor {
    let registry = create_default_registry();
    ToolExecutor::new(Arc::new(registry))
}

// ═══════════════════════════════════════════════════════════════════
// 1. BashTool integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bash_echo_via_executor() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let input = serde_json::json!({ "command": "echo integration-test" });
    let output = executor.execute("bash", input, &ctx).await.unwrap();
    assert!(!output.is_error, "output: {}", output.text());
    assert!(
        output.text().contains("integration-test"),
        "expected 'integration-test' in output: {}",
        output.text()
    );
}

#[tokio::test]
async fn bash_ls_via_executor() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("visible.txt"), "content").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let cmd = if cfg!(windows) { "dir /b" } else { "ls" };
    let input = serde_json::json!({ "command": cmd });
    let output = executor.execute("bash", input, &ctx).await.unwrap();
    assert!(!output.is_error, "output: {}", output.text());
    assert!(
        output.text().contains("visible.txt"),
        "expected 'visible.txt' in: {}",
        output.text()
    );
}

#[tokio::test]
async fn bash_nonzero_exit_is_error_via_executor() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let cmd = if cfg!(windows) {
        "exit /b 42"
    } else {
        "exit 42"
    };
    let input = serde_json::json!({ "command": cmd });
    let output = executor.execute("bash", input, &ctx).await.unwrap();
    assert!(output.is_error);
}

#[tokio::test]
async fn bash_working_dir_is_respected() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let cmd = if cfg!(windows) { "cd" } else { "pwd" };
    let input = serde_json::json!({ "command": cmd });
    let output = executor.execute("bash", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    // The output should contain the temp dir path
    let text = output.text();
    let tmp_canonical = tmp.path().canonicalize().unwrap();
    assert!(
        text.contains(tmp_canonical.to_str().unwrap())
            || text.contains(tmp.path().to_str().unwrap()),
        "expected working dir in output: {text}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// 2. ReadTool integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn read_file_with_line_numbers() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("sample.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\ndelta\n").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "file_path": file.to_str().unwrap() });
    let output = executor.execute("read", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    // cat -n format: "     1\talpha"
    assert!(text.contains("1\talpha"), "line 1 format: {text}");
    assert!(text.contains("2\tbeta"), "line 2 format: {text}");
    assert!(text.contains("3\tgamma"), "line 3 format: {text}");
    assert!(text.contains("4\tdelta"), "line 4 format: {text}");
}

#[tokio::test]
async fn read_file_with_offset_and_limit() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("offtest.txt");
    std::fs::write(&file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({
        "file_path": file.to_str().unwrap(),
        "offset": 2,
        "limit": 2
    });
    let output = executor.execute("read", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    // Should show lines 2-3 only
    assert!(text.contains("2\tline2"), "expected line2: {text}");
    assert!(text.contains("3\tline3"), "expected line3: {text}");
    assert!(
        !text.contains("1\tline1"),
        "should not contain line1: {text}"
    );
    assert!(
        !text.contains("4\tline4"),
        "should not contain line4: {text}"
    );
}

#[tokio::test]
async fn read_nonexistent_file_is_error() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Default);

    let input = serde_json::json!({ "file_path": "/nonexistent/path/file.txt" });
    let output = executor.execute("read", input, &ctx).await.unwrap();
    assert!(output.is_error);
    assert!(output.text().contains("Failed to read"));
}

#[tokio::test]
async fn read_binary_extension_returns_info() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Default);

    let input = serde_json::json!({ "file_path": "/some/image.png" });
    let output = executor.execute("read", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    assert!(output.text().contains("Binary file"));
}

// ═══════════════════════════════════════════════════════════════════
// 3. WriteTool integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn write_tool_creates_file() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let file = tmp.path().join("output.txt");
    let input = serde_json::json!({
        "file_path": file.to_str().unwrap(),
        "content": "hello world"
    });
    let output = executor.execute("write", input, &ctx).await.unwrap();
    assert!(!output.is_error, "write should succeed: {}", output.text());
    assert!(file.exists());
}

// ═══════════════════════════════════════════════════════════════════
// 4. EditTool integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn edit_tool_returns_error_for_nonexistent_file() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let input = serde_json::json!({
        "file_path": tmp.path().join("nonexistent.rs").to_str().unwrap(),
        "old_string": "foo",
        "new_string": "bar"
    });
    let output = executor.execute("edit", input, &ctx).await.unwrap();
    assert!(output.is_error);
}

// ═══════════════════════════════════════════════════════════════════
// 5. GlobTool integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn glob_finds_files_in_temp_tree() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(tmp.path().join("lib.rs"), "pub mod lib;").unwrap();
    std::fs::write(tmp.path().join("readme.md"), "# Readme").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "pattern": "*.rs" });
    let output = executor.execute("glob", input, &ctx).await.unwrap();
    assert!(!output.is_error, "output: {}", output.text());
    let text = output.text();
    assert!(text.contains("main.rs"), "expected main.rs: {text}");
    assert!(text.contains("lib.rs"), "expected lib.rs: {text}");
    assert!(!text.contains("readme.md"), "should not match md: {text}");
}

#[tokio::test]
async fn glob_recursive_pattern() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("top.rs"), "// top").unwrap();
    let nested = tmp.path().join("src").join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("deep.rs"), "// deep").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "pattern": "**/*.rs" });
    let output = executor.execute("glob", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("top.rs"), "expected top.rs: {text}");
    assert!(text.contains("deep.rs"), "expected deep.rs: {text}");
}

#[tokio::test]
async fn glob_no_matches_returns_message() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("readme.md"), "# hi").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "pattern": "*.xyz" });
    let output = executor.execute("glob", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    assert!(output.text().contains("No files matched"));
}

#[tokio::test]
async fn glob_with_explicit_path() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("inner.rs"), "// inner").unwrap();
    std::fs::write(tmp.path().join("outer.rs"), "// outer").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "pattern": "*.rs", "path": "subdir" });
    let output = executor.execute("glob", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("inner.rs"), "expected inner.rs: {text}");
    assert!(
        !text.contains("outer.rs"),
        "should not contain outer.rs: {text}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// 6. GrepTool integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn grep_finds_content_in_files() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("code.rs"),
        "fn main() {}\nfn helper() {}\nlet x = 5;\n",
    )
    .unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({
        "pattern": "fn\\s+\\w+",
        "output_mode": "content"
    });
    let output = executor.execute("grep", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("fn main()"), "expected fn main(): {text}");
    assert!(text.contains("fn helper()"), "expected fn helper(): {text}");
    assert!(!text.contains("let x"), "should not match let x: {text}");
}

#[tokio::test]
async fn grep_files_with_matches_mode() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.rs"), "hello world\n").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "goodbye world\n").unwrap();
    std::fs::write(tmp.path().join("c.txt"), "no match here\n").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "pattern": "world" });
    let output = executor.execute("grep", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("a.rs"), "expected a.rs: {text}");
    assert!(text.contains("b.rs"), "expected b.rs: {text}");
    assert!(!text.contains("c.txt"), "should not contain c.txt: {text}");
}

#[tokio::test]
async fn grep_count_mode() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("multi.txt"), "match1\nmatch2\nmatch3\n").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({
        "pattern": "match",
        "output_mode": "count"
    });
    let output = executor.execute("grep", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    assert!(
        output.text().contains(":3"),
        "expected count 3: {}",
        output.text()
    );
}

#[tokio::test]
async fn grep_no_matches_returns_message() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("test.txt"), "nothing here\n").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "pattern": "zzz_nonexistent" });
    let output = executor.execute("grep", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    assert!(output.text().contains("No matches found"));
}

#[tokio::test]
async fn grep_with_glob_filter() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("code.rs"), "hello\n").unwrap();
    std::fs::write(tmp.path().join("doc.md"), "hello\n").unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({
        "pattern": "hello",
        "glob": "*.rs"
    });
    let output = executor.execute("grep", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("code.rs"), "expected code.rs: {text}");
    assert!(
        !text.contains("doc.md"),
        "should not contain doc.md: {text}"
    );
}

#[tokio::test]
async fn grep_with_context_lines() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("ctx.txt"),
        "line1\nline2\nTARGET\nline4\nline5\n",
    )
    .unwrap();

    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({
        "pattern": "TARGET",
        "output_mode": "content",
        "context": 1
    });
    let output = executor.execute("grep", input, &ctx).await.unwrap();
    assert!(!output.is_error);
    let text = output.text();
    assert!(text.contains("TARGET"), "expected TARGET: {text}");
    assert!(text.contains("line2"), "expected context before: {text}");
    assert!(text.contains("line4"), "expected context after: {text}");
}

// ═══════════════════════════════════════════════════════════════════
// 7. Permission integration
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn permission_denied_tool_blocked() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);
    ctx.permission_policy.denied_tools = vec!["bash".into()];

    let input = serde_json::json!({ "command": "echo hello" });
    let output = executor.execute("bash", input, &ctx).await.unwrap();
    assert!(output.is_error);
    assert!(
        output.text().contains("denied"),
        "expected denied message: {}",
        output.text()
    );
}

#[tokio::test]
async fn permission_denied_glob_pattern() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);
    ctx.permission_policy.denied_tools = vec!["g*".into()];

    // Both glob and grep should be denied
    let glob_input = serde_json::json!({ "pattern": "*.rs" });
    let glob_output = executor.execute("glob", glob_input, &ctx).await.unwrap();
    assert!(glob_output.is_error, "glob should be denied");

    let grep_input = serde_json::json!({ "pattern": "hello" });
    let grep_output = executor.execute("grep", grep_input, &ctx).await.unwrap();
    assert!(grep_output.is_error, "grep should be denied");
}

#[tokio::test]
async fn permission_read_only_always_allowed() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("readable.txt");
    std::fs::write(&file, "content\n").unwrap();

    // Default mode — read-only tools should be auto-allowed
    let ctx = make_ctx(tmp.path(), PermissionMode::Default);
    let input = serde_json::json!({ "file_path": file.to_str().unwrap() });
    let output = executor.execute("read", input, &ctx).await.unwrap();
    assert!(
        !output.is_error,
        "read should be allowed: {}",
        output.text()
    );
}

#[tokio::test]
async fn permission_dangerously_allows_all() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Dangerously);

    let input = serde_json::json!({ "command": "echo permitted" });
    let output = executor.execute("bash", input, &ctx).await.unwrap();
    assert!(
        !output.is_error,
        "Dangerously should auto-allow: {}",
        output.text()
    );
    assert!(output.text().contains("permitted"));
}

#[tokio::test]
async fn permission_missing_tool_is_error() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(tmp.path(), PermissionMode::Default);

    let result = executor
        .execute("nonexistent_tool", serde_json::json!({}), &ctx)
        .await;
    assert!(result.is_err(), "nonexistent tool should return Err");
}

#[tokio::test]
async fn execute_unchecked_skips_permission() {
    let executor = make_executor();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("unchecked.txt");
    std::fs::write(&file, "unchecked content\n").unwrap();

    // Even with denied list, execute_unchecked should work
    let mut ctx = make_ctx(tmp.path(), PermissionMode::Default);
    ctx.permission_policy.denied_tools = vec!["read".into()];

    let input = serde_json::json!({ "file_path": file.to_str().unwrap() });
    let output = executor
        .execute_unchecked("read", input, &ctx)
        .await
        .unwrap();
    assert!(
        !output.is_error,
        "unchecked should bypass denial: {}",
        output.text()
    );
    assert!(output.text().contains("unchecked content"));
}

// ═══════════════════════════════════════════════════════════════════
// 8. register_all_builtins() verification
// ═══════════════════════════════════════════════════════════════════

#[test]
fn register_all_builtins_produces_expected_tools() {
    let registry = create_default_registry();
    let expected = if cfg!(target_os = "windows") { 32 } else { 31 };
    assert_eq!(registry.len(), expected);
}

#[test]
fn all_expected_tools_registered() {
    let registry = create_default_registry();
    let expected = [
        "bash",
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "notebook_edit",
        "notebook_read",
        "lsp",
        "agent",
        "web_search",
        "web_fetch",
        "ask_user",
        "enter_plan_mode",
        "image_read",
        "task_create",
        "task_list",
        "task_update",
        "task_get",
        "enter_worktree",
        "exit_worktree",
        "team_create",
        "team_delete",
        "send_message",
        "task_stop",
        "task_output",
        "cron_create",
        "cron_delete",
        "cron_list",
        "remote_trigger",
    ];
    for name in &expected {
        assert!(
            registry.get(name).is_some(),
            "expected tool '{name}' not found"
        );
    }
}

#[test]
fn all_tools_have_valid_schemas() {
    let registry = create_default_registry();
    let schemas = registry.tool_schemas();
    let expected = if cfg!(target_os = "windows") { 32 } else { 31 };
    assert_eq!(schemas.len(), expected);
    for schema in &schemas {
        let name = schema["name"].as_str().unwrap();
        assert!(!name.is_empty(), "tool name should not be empty");
        assert!(
            schema["description"].as_str().is_some(),
            "tool '{name}' missing description"
        );
        assert!(
            schema["input_schema"]["type"].as_str().is_some(),
            "tool '{name}' missing input_schema type"
        );
    }
}

#[test]
fn tool_names_are_sorted() {
    let registry = create_default_registry();
    let names = registry.tool_names();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "tool_names() should return sorted names");
}

#[test]
fn tool_schemas_are_sorted_by_name() {
    let registry = create_default_registry();
    let schemas = registry.tool_schemas();
    let names: Vec<&str> = schemas
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "schemas should be sorted by name");
}

#[test]
fn register_same_tool_twice_overwrites() {
    let mut registry = ToolRegistry::new();
    register_all_builtins(&mut registry, None);
    let count_before = registry.len();
    // Re-register — should overwrite, not duplicate
    register_all_builtins(&mut registry, None);
    assert_eq!(registry.len(), count_before);
}

#[test]
fn filtered_schemas_works() {
    let registry = create_default_registry();
    let filtered = registry.tool_schemas_filtered(&["bash", "read", "missing_tool"]);
    assert_eq!(filtered.len(), 2);
    let names: Vec<&str> = filtered
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"bash"));
    assert!(names.contains(&"read"));
}
