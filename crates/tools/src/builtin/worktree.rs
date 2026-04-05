//! Git worktree management tools.
//!
//! - `EnterWorktreeTool` — create a git worktree in `.crab/worktrees/`
//! - `ExitWorktreeTool` — keep or remove an existing worktree

use std::future::Future;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use crab_process::spawn::{SpawnOptions, run};
use serde_json::Value;

// ─── EnterWorktreeTool ───────────────────────────────────────────────

/// Creates a git worktree in `.crab/worktrees/<name>` with a new branch.
pub struct EnterWorktreeTool;

impl Tool for EnterWorktreeTool {
    fn name(&self) -> &'static str {
        "enter_worktree"
    }

    fn description(&self) -> &'static str {
        "Create an isolated git worktree in .crab/worktrees/ with a new branch \
         based on HEAD. Use this to work on features in isolation without \
         affecting the current workspace."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Optional name for the worktree. If omitted, a random name is generated."
                }
            }
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let name = input["name"]
            .as_str()
            .map_or_else(|| format!("wt-{:08x}", rand_u32()), ToOwned::to_owned);
        let working_dir = ctx.working_dir.clone();

        Box::pin(async move {
            // Validate name
            if !is_valid_worktree_name(&name) {
                return Ok(ToolOutput::error(
                    "invalid worktree name: use only letters, digits, dots, underscores, \
                     dashes (max 64 chars)",
                ));
            }

            // Check we're in a git repo
            let check = run(SpawnOptions {
                command: "git".to_owned(),
                args: vec!["rev-parse".to_owned(), "--is-inside-work-tree".to_owned()],
                working_dir: Some(working_dir.clone()),
                env: vec![],
                timeout: Some(std::time::Duration::from_secs(10)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            })
            .await?;

            if check.exit_code != 0 {
                return Ok(ToolOutput::error(
                    "not inside a git repository — cannot create worktree",
                ));
            }

            let worktree_dir = working_dir.join(".crab").join("worktrees").join(&name);
            let branch_name = format!("crab-worktree/{name}");

            if worktree_dir.exists() {
                return Ok(ToolOutput::error(format!(
                    "worktree already exists: {}",
                    worktree_dir.display()
                )));
            }

            // Create parent directory
            if let Some(parent) = worktree_dir.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    crab_common::Error::Other(format!("failed to create worktree directory: {e}"))
                })?;
            }

            // git worktree add -b <branch> <path>
            let result = run(SpawnOptions {
                command: "git".to_owned(),
                args: vec![
                    "worktree".to_owned(),
                    "add".to_owned(),
                    "-b".to_owned(),
                    branch_name.clone(),
                    worktree_dir.to_string_lossy().into_owned(),
                ],
                working_dir: Some(working_dir),
                env: vec![],
                timeout: Some(std::time::Duration::from_secs(30)),
                stdin_data: None,
                clear_env: false,
                kill_grace_period: None,
            })
            .await?;

            if result.exit_code != 0 {
                let err = if result.stderr.is_empty() {
                    result.stdout
                } else {
                    result.stderr
                };
                return Ok(ToolOutput::error(format!("git worktree add failed: {err}")));
            }

            Ok(ToolOutput::success(format!(
                "Worktree created.\n  path: {}\n  branch: {branch_name}",
                worktree_dir.display()
            )))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
}

// ─── ExitWorktreeTool ────────────────────────────────────────────────

/// Remove or keep a git worktree.
pub struct ExitWorktreeTool;

impl Tool for ExitWorktreeTool {
    fn name(&self) -> &'static str {
        "exit_worktree"
    }

    fn description(&self) -> &'static str {
        "Exit a git worktree session. Use action 'keep' to leave the worktree \
         on disk, or 'remove' to delete it. When removing, set discard_changes \
         to true if there are uncommitted changes."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "worktree_path": {
                    "type": "string",
                    "description": "Absolute path to the worktree to exit"
                },
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": "'keep' leaves the worktree on disk; 'remove' deletes it"
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": "If true and action is 'remove', force-remove even with uncommitted changes"
                }
            },
            "required": ["worktree_path", "action"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let worktree_path = input["worktree_path"].as_str().unwrap_or("").to_owned();
        let action = input["action"].as_str().unwrap_or("").to_owned();
        let discard = input["discard_changes"].as_bool().unwrap_or(false);
        let working_dir = ctx.working_dir.clone();

        Box::pin(async move {
            if worktree_path.is_empty() {
                return Ok(ToolOutput::error("worktree_path is required"));
            }

            match action.as_str() {
                "keep" => Ok(ToolOutput::success(format!(
                    "Worktree kept at: {worktree_path}\n\
                     You can return to it later or remove it with action 'remove'."
                ))),
                "remove" => {
                    let wt_path = std::path::PathBuf::from(&worktree_path);
                    if !wt_path.exists() {
                        return Ok(ToolOutput::error(format!(
                            "worktree path does not exist: {worktree_path}"
                        )));
                    }

                    // Check for uncommitted changes if not discarding
                    if !discard {
                        let status = run(SpawnOptions {
                            command: "git".to_owned(),
                            args: vec!["status".to_owned(), "--porcelain".to_owned()],
                            working_dir: Some(wt_path.clone()),
                            env: vec![],
                            timeout: Some(std::time::Duration::from_secs(10)),
                            stdin_data: None,
                            clear_env: false,
                            kill_grace_period: None,
                        })
                        .await?;

                        if !status.stdout.trim().is_empty() {
                            return Ok(ToolOutput::error(format!(
                                "worktree has uncommitted changes:\n{}\n\
                                 Set discard_changes=true to force remove.",
                                status.stdout.trim()
                            )));
                        }
                    }

                    // git worktree remove [--force] <path>
                    let mut args = vec!["worktree".to_owned(), "remove".to_owned()];
                    if discard {
                        args.push("--force".to_owned());
                    }
                    args.push(worktree_path.clone());

                    let result = run(SpawnOptions {
                        command: "git".to_owned(),
                        args,
                        working_dir: Some(working_dir),
                        env: vec![],
                        timeout: Some(std::time::Duration::from_secs(30)),
                        stdin_data: None,
                        clear_env: false,
                        kill_grace_period: None,
                    })
                    .await?;

                    if result.exit_code != 0 {
                        let err = if result.stderr.is_empty() {
                            result.stdout
                        } else {
                            result.stderr
                        };
                        return Ok(ToolOutput::error(format!(
                            "git worktree remove failed: {err}"
                        )));
                    }

                    Ok(ToolOutput::success(format!(
                        "Worktree removed: {worktree_path}"
                    )))
                }
                "" => Ok(ToolOutput::error("action is required ('keep' or 'remove')")),
                other => Ok(ToolOutput::error(format!(
                    "unknown action: '{other}' (expected 'keep' or 'remove')"
                ))),
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Validate worktree name: letters, digits, dots, underscores, dashes, max 64.
fn is_valid_worktree_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Simple pseudo-random u32 based on the current time.
fn rand_u32() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Mix nanoseconds for some randomness
    let n = now.as_nanos();
    #[allow(clippy::cast_possible_truncation)]
    let v = (n ^ (n >> 16) ^ (n >> 32)) as u32;
    v
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
        }
    }

    // ─── EnterWorktreeTool ──────────────────────────────────────────

    #[test]
    fn enter_worktree_metadata() {
        let tool = EnterWorktreeTool;
        assert_eq!(tool.name(), "enter_worktree");
        assert!(tool.requires_confirmation());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn enter_worktree_schema() {
        let schema = EnterWorktreeTool.input_schema();
        assert!(schema["properties"]["name"].is_object());
        // name is optional — no "required" array or name not in it
        let required = schema.get("required").and_then(Value::as_array);
        assert!(required.is_none() || !required.unwrap().iter().any(|v| v == "name"));
    }

    #[tokio::test]
    async fn enter_worktree_invalid_name() {
        let tool = EnterWorktreeTool;
        let input = serde_json::json!({ "name": "bad name with spaces!" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("invalid worktree name"));
    }

    #[tokio::test]
    async fn enter_worktree_name_too_long() {
        let tool = EnterWorktreeTool;
        let long_name = "a".repeat(65);
        let input = serde_json::json!({ "name": long_name });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("invalid worktree name"));
    }

    #[tokio::test]
    async fn enter_worktree_not_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            working_dir: dir.path().to_path_buf(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
        };
        let tool = EnterWorktreeTool;
        let input = serde_json::json!({ "name": "test-wt" });
        let out = tool.execute(input, &ctx).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("git"));
    }

    // ─── ExitWorktreeTool ───────────────────────────────────────────

    #[test]
    fn exit_worktree_metadata() {
        let tool = ExitWorktreeTool;
        assert_eq!(tool.name(), "exit_worktree");
        assert!(tool.requires_confirmation());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn exit_worktree_schema() {
        let schema = ExitWorktreeTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "worktree_path"));
        assert!(required.iter().any(|v| v == "action"));
        assert!(schema["properties"]["discard_changes"].is_object());
    }

    #[tokio::test]
    async fn exit_worktree_empty_path() {
        let tool = ExitWorktreeTool;
        let input = serde_json::json!({ "worktree_path": "", "action": "keep" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("required"));
    }

    #[tokio::test]
    async fn exit_worktree_empty_action() {
        let tool = ExitWorktreeTool;
        let input = serde_json::json!({ "worktree_path": "/tmp/some-wt", "action": "" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("required"));
    }

    #[tokio::test]
    async fn exit_worktree_unknown_action() {
        let tool = ExitWorktreeTool;
        let input = serde_json::json!({ "worktree_path": "/tmp/some-wt", "action": "nuke" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("unknown action"));
    }

    #[tokio::test]
    async fn exit_worktree_keep_returns_path() {
        let tool = ExitWorktreeTool;
        let input = serde_json::json!({ "worktree_path": "/tmp/my-worktree", "action": "keep" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("/tmp/my-worktree"));
        assert!(out.text().contains("kept"));
    }

    #[tokio::test]
    async fn exit_worktree_remove_nonexistent_path() {
        let tool = ExitWorktreeTool;
        let input = serde_json::json!({
            "worktree_path": "/nonexistent/worktree/path",
            "action": "remove"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("does not exist"));
    }

    // ─── Helpers ────────────────────────────────────────────────────

    #[test]
    fn valid_worktree_names() {
        assert!(is_valid_worktree_name("my-feature"));
        assert!(is_valid_worktree_name("fix_bug.123"));
        assert!(is_valid_worktree_name("a"));
        assert!(is_valid_worktree_name("wt-0001"));
    }

    #[test]
    fn invalid_worktree_names() {
        assert!(!is_valid_worktree_name(""));
        assert!(!is_valid_worktree_name("has space"));
        assert!(!is_valid_worktree_name("slash/path"));
        assert!(!is_valid_worktree_name("special!char"));
        assert!(!is_valid_worktree_name(&"a".repeat(65)));
    }

    #[test]
    fn rand_u32_produces_value() {
        // Just verify it doesn't panic and returns something
        let v = rand_u32();
        // Can't assert a specific value, just that it's a u32
        assert!(v <= u32::MAX);
    }
}
