//! Lifecycle hooks for pre/post tool execution and lifecycle events.
//!
//! Implements the Claude Code hooks protocol: hooks are shell commands that
//! receive a JSON payload on **stdin** (`session_id`, `cwd`,
//! `hook_event_name`, tool fields) and respond either through their exit
//! code (0 = success, 2 = blocking error) or by printing a JSON object to
//! stdout (`decision`, `reason`, `hookSpecificOutput`, ...). All matching
//! hooks for an event run concurrently.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ─── Types ──────────────────────────────────────────────────────────────

pub use crab_core::hook::HookTrigger;

/// Default per-hook timeout in seconds, matching the CC hooks protocol.
pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 600;

/// A single hook definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    /// When this hook fires.
    pub trigger: HookTrigger,
    /// CC matcher pattern: `None` / empty / `"*"` matches everything;
    /// a plain name matches exactly; `A|B` matches either name; anything
    /// else is tried as a regex. Ignored for non-tool events.
    #[serde(default)]
    pub matcher: Option<String>,
    /// Shell command to execute.
    pub command: String,
    /// Timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_HOOK_TIMEOUT_SECS
}

/// Context passed to a hook via the stdin JSON payload.
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// Name of the tool being invoked (tool events only).
    pub tool_name: String,
    /// JSON-serialized tool input; for `UserPromptSubmit` this carries the
    /// prompt text.
    pub tool_input: String,
    /// Working directory for the hook process.
    pub working_dir: Option<PathBuf>,
    /// JSON-serialized tool output (only for post-tool-use).
    pub tool_output: Option<String>,
    /// Tool exit code (only for post-tool-use).
    pub tool_exit_code: Option<i32>,
    /// Current session ID.
    pub session_id: Option<String>,
    /// Path to the session transcript file, when persistence is enabled.
    pub transcript_path: Option<PathBuf>,
}

/// Action a hook commands the system to take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookAction {
    /// Allow the operation to proceed as-is.
    #[default]
    Allow,
    /// Block the operation.
    Deny,
    /// Allow but with modified input.
    Modify,
    /// Defer to the interactive permission system (CC `permissionDecision:
    /// "ask"`). Falls back to the normal permission flow.
    Ask,
    /// Request the query loop to retry the current turn (used by Stop hooks).
    Retry,
}

/// Result of executing one or more hooks for a trigger point.
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// Structured action resolved from hook output.
    pub action: HookAction,
    /// Whether some hook explicitly allowed the operation
    /// (`permissionDecision: "allow"` / `decision: "approve"`), as opposed
    /// to simply expressing no opinion. `PermissionRequest` consumers use
    /// this to skip the interactive prompt.
    pub explicit_allow: bool,
    /// Optional message from the hook (deny reason, stop reason, ...).
    pub message: Option<String>,
    /// Modified tool input (if action is Modify).
    pub modified_input: Option<serde_json::Value>,
    /// Extra context the hook wants injected into the conversation
    /// (CC `hookSpecificOutput.additionalContext`).
    pub additional_context: Option<String>,
    /// Warning message shown to the user (CC `systemMessage`).
    pub system_message: Option<String>,
    /// Combined hook stdout output.
    pub stdout: String,
    /// Combined hook stderr output.
    pub stderr: String,
    /// Last non-zero hook exit code (or 0).
    pub exit_code: i32,
    /// Whether any hook timed out.
    pub timed_out: bool,
}

impl HookResult {
    /// Whether the hook(s) allow the operation to proceed.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.action != HookAction::Deny
    }
}

// ─── CC output schema ───────────────────────────────────────────────────

/// The CC hooks stdout JSON schema (sync variant).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CcHookOutput {
    /// `false` requests that processing stop.
    #[serde(rename = "continue")]
    continue_: Option<bool>,
    stop_reason: Option<String>,
    /// `"approve"` or `"block"`.
    decision: Option<String>,
    reason: Option<String>,
    system_message: Option<String>,
    hook_specific_output: Option<CcHookSpecificOutput>,
    /// `{"async": true}` marks an async hook (not yet supported; treated
    /// as fire-and-forget success).
    #[serde(rename = "async")]
    async_: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CcHookSpecificOutput {
    /// `"allow"`, `"deny"`, or `"ask"` (`PreToolUse`).
    permission_decision: Option<String>,
    permission_decision_reason: Option<String>,
    updated_input: Option<serde_json::Value>,
    additional_context: Option<String>,
}

// ─── Executor ───────────────────────────────────────────────────────────

/// Executes lifecycle hooks around tool invocations.
pub struct HookExecutor {
    hooks: Vec<HookDef>,
}

impl HookExecutor {
    /// Create executor with no hooks.
    #[must_use]
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Create executor from parsed hook definitions.
    #[must_use]
    pub fn with_hooks(hooks: Vec<HookDef>) -> Self {
        Self { hooks }
    }

    /// Whether any configured hook would fire for this trigger and tool.
    ///
    /// Lets a caller cheaply decide, without running anything, whether a tool
    /// has an applicable hook — e.g. the inline streaming path uses this to
    /// avoid eagerly executing a tool that has a `PreToolUse` hook, which must
    /// run before the tool does.
    #[must_use]
    pub fn has_matching_hook(&self, trigger: HookTrigger, tool_name: &str) -> bool {
        !self.matching_hooks(trigger, tool_name).is_empty()
    }

    /// Get hooks matching a trigger point and tool name.
    fn matching_hooks(&self, trigger: HookTrigger, tool_name: &str) -> Vec<&HookDef> {
        let tool_scoped = matches!(trigger, HookTrigger::PreToolUse | HookTrigger::PostToolUse);
        self.hooks
            .iter()
            .filter(|h| {
                if h.trigger != trigger {
                    return false;
                }
                if !tool_scoped {
                    return true;
                }
                matches_pattern(h.matcher.as_deref(), tool_name)
            })
            .collect()
    }

    /// Run all hooks for a given trigger point. Matching hooks run
    /// concurrently; their outputs are aggregated with `Deny` winning over
    /// `Ask`/`Retry`, which win over `Modify`, which wins over `Allow`.
    ///
    /// Per CC protocol, each hook receives the JSON payload on stdin and
    /// responds via stdout JSON or exit code (0 = success, 2 = blocking
    /// error with stderr as the reason, anything else = non-blocking error).
    pub async fn run(
        &self,
        trigger: HookTrigger,
        ctx: &HookContext,
    ) -> crab_core::Result<HookResult> {
        let hooks = self.matching_hooks(trigger, &ctx.tool_name);
        if hooks.is_empty() {
            return Ok(HookResult::default());
        }

        let payload = build_stdin_payload(trigger, ctx);
        let payload_str = payload.to_string();

        let runs = hooks
            .iter()
            .map(|hook| run_one_hook(hook, ctx, &payload_str));
        let outcomes = futures::future::join_all(runs).await;

        let mut result = HookResult::default();
        for outcome in outcomes {
            merge_outcome(&mut result, outcome, trigger);
        }
        Ok(result)
    }

    /// Number of registered hooks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Whether there are no hooks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

impl Default for HookExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Single-hook execution ──────────────────────────────────────────────

/// Outcome of one hook process, before aggregation.
struct HookOutcome {
    action: HookAction,
    explicit_allow: bool,
    message: Option<String>,
    modified_input: Option<serde_json::Value>,
    additional_context: Option<String>,
    system_message: Option<String>,
    stdout: String,
    stderr: String,
    exit_code: i32,
    timed_out: bool,
}

async fn run_one_hook(hook: &HookDef, ctx: &HookContext, payload: &str) -> HookOutcome {
    let (shell, shell_flag) = if cfg!(windows) {
        ("cmd".to_string(), "/C".to_string())
    } else {
        ("sh".to_string(), "-c".to_string())
    };

    let project_dir = ctx
        .working_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let env = vec![
        ("CRAB_PROJECT_DIR".to_string(), project_dir.clone()),
        // CC scripts reference $CLAUDE_PROJECT_DIR in hook commands; export
        // the alias so they run unmodified.
        ("CLAUDE_PROJECT_DIR".to_string(), project_dir),
    ];

    let opts = crab_process::spawn::SpawnOptions {
        command: shell,
        args: vec![shell_flag, hook.command.clone()],
        working_dir: ctx.working_dir.clone(),
        env,
        timeout: Some(Duration::from_secs(hook.timeout_secs.max(1))),
        stdin_data: Some(payload.to_string()),
        clear_env: false,
        kill_grace_period: None,
    };

    let output = match crab_process::spawn::run(opts).await {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!(
                hook_command = hook.command.as_str(),
                error = %e,
                "hook execution failed"
            );
            return HookOutcome {
                action: if hook.trigger == HookTrigger::PreToolUse {
                    HookAction::Deny
                } else {
                    HookAction::Allow
                },
                explicit_allow: false,
                message: None,
                modified_input: None,
                additional_context: None,
                system_message: None,
                stdout: String::new(),
                stderr: format!("hook error: {e}"),
                exit_code: -1,
                timed_out: false,
            };
        }
    };

    tracing::debug!(
        hook_command = hook.command.as_str(),
        exit_code = output.exit_code,
        timed_out = output.timed_out,
        "hook completed"
    );

    if output.timed_out {
        // CC treats a timeout as a non-blocking error: warn and proceed.
        tracing::warn!(
            hook_command = hook.command.as_str(),
            timeout_secs = hook.timeout_secs,
            "hook timed out; continuing"
        );
        return HookOutcome {
            action: HookAction::Allow,
            explicit_allow: false,
            message: None,
            modified_input: None,
            additional_context: None,
            system_message: None,
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: true,
        };
    }

    // Exit code 2 is a blocking error: stderr becomes the reason and the
    // operation is denied (for deny-capable events).
    if output.exit_code == 2 {
        let message = if output.stderr.trim().is_empty() {
            "Blocked by hook".to_string()
        } else {
            output.stderr.trim().to_string()
        };
        return HookOutcome {
            action: deny_action_for(hook.trigger),
            explicit_allow: false,
            message: Some(message),
            modified_input: None,
            additional_context: None,
            system_message: None,
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: 2,
            timed_out: false,
        };
    }

    // Other non-zero exit codes are non-blocking errors.
    if output.exit_code != 0 {
        tracing::warn!(
            hook_command = hook.command.as_str(),
            exit_code = output.exit_code,
            stderr = output.stderr.trim(),
            "hook exited non-zero (non-blocking)"
        );
        return HookOutcome {
            action: HookAction::Allow,
            explicit_allow: false,
            message: None,
            modified_input: None,
            additional_context: None,
            system_message: None,
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: false,
        };
    }

    // Exit 0: interpret stdout as CC JSON if present.
    match parse_cc_output(&output.stdout) {
        Some(cc) => cc_output_to_outcome(cc, hook.trigger, output.stdout, output.stderr),
        None => HookOutcome {
            action: HookAction::Allow,
            explicit_allow: false,
            message: None,
            modified_input: None,
            additional_context: None,
            system_message: None,
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: 0,
            timed_out: false,
        },
    }
}

/// The deny-equivalent action for a trigger: Stop hooks express "keep
/// going" as `Retry`; everything else denies.
fn deny_action_for(trigger: HookTrigger) -> HookAction {
    if trigger == HookTrigger::Stop {
        HookAction::Retry
    } else {
        HookAction::Deny
    }
}

fn cc_output_to_outcome(
    cc: CcHookOutput,
    trigger: HookTrigger,
    stdout: String,
    stderr: String,
) -> HookOutcome {
    let mut action = HookAction::Allow;
    let mut explicit_allow = false;
    let mut message = None;
    let mut modified_input = None;
    let mut additional_context = None;

    if cc.async_ == Some(true) {
        tracing::warn!("async hooks are not supported yet; treating as completed");
    }

    if let Some(specific) = cc.hook_specific_output {
        match specific.permission_decision.as_deref() {
            Some("allow") => {
                explicit_allow = true;
                message.clone_from(&specific.permission_decision_reason);
            }
            Some("deny") => {
                action = HookAction::Deny;
                message.clone_from(&specific.permission_decision_reason);
            }
            Some("ask") => {
                action = HookAction::Ask;
                message.clone_from(&specific.permission_decision_reason);
            }
            _ => {}
        }
        if action == HookAction::Allow && specific.updated_input.is_some() {
            action = HookAction::Modify;
            modified_input = specific.updated_input;
        }
        additional_context = specific.additional_context;
    }

    if cc.decision.as_deref() == Some("approve") {
        explicit_allow = true;
    }

    // Top-level decision: "block" denies (Stop hooks: retry the loop).
    if action == HookAction::Allow && cc.decision.as_deref() == Some("block") {
        action = deny_action_for(trigger);
        message = cc.reason.clone().or(message);
    }

    // continue:false halts processing — strongest signal after an explicit
    // deny; carries stopReason as the message.
    if cc.continue_ == Some(false) && action == HookAction::Allow {
        action = deny_action_for(trigger);
        message = cc.stop_reason.clone().or_else(|| cc.reason.clone());
    }

    if message.is_none() {
        message = cc.reason;
    }

    HookOutcome {
        action,
        explicit_allow,
        message,
        modified_input,
        additional_context,
        system_message: cc.system_message,
        stdout,
        stderr,
        exit_code: 0,
        timed_out: false,
    }
}

/// Merge one hook's outcome into the aggregated result. Precedence:
/// `Deny` > `Ask` / `Retry` > `Modify` > `Allow`.
fn merge_outcome(result: &mut HookResult, outcome: HookOutcome, _trigger: HookTrigger) {
    if !result.stdout.is_empty() && !outcome.stdout.is_empty() {
        result.stdout.push('\n');
    }
    result.stdout.push_str(&outcome.stdout);

    if !result.stderr.is_empty() && !outcome.stderr.is_empty() {
        result.stderr.push('\n');
    }
    result.stderr.push_str(&outcome.stderr);

    if outcome.exit_code != 0 {
        result.exit_code = outcome.exit_code;
    }
    result.timed_out |= outcome.timed_out;

    result.explicit_allow |= outcome.explicit_allow;

    let stronger = action_rank(outcome.action) > action_rank(result.action);
    if stronger {
        result.action = outcome.action;
        if outcome.message.is_some() {
            result.message = outcome.message;
        }
    } else if result.message.is_none() {
        result.message = outcome.message;
    }
    if outcome.modified_input.is_some() && result.modified_input.is_none() {
        result.modified_input = outcome.modified_input;
    }
    if let Some(ctx) = outcome.additional_context {
        match &mut result.additional_context {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(&ctx);
            }
            None => result.additional_context = Some(ctx),
        }
    }
    if outcome.system_message.is_some() {
        result.system_message = outcome.system_message;
    }
}

fn action_rank(action: HookAction) -> u8 {
    match action {
        HookAction::Allow => 0,
        HookAction::Modify => 1,
        HookAction::Ask | HookAction::Retry => 2,
        HookAction::Deny => 3,
    }
}

// ─── Payload / parsing helpers ──────────────────────────────────────────

/// Build the CC-protocol stdin payload for a hook invocation.
fn build_stdin_payload(trigger: HookTrigger, ctx: &HookContext) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "hook_event_name": trigger.event_name(),
        "session_id": ctx.session_id.clone().unwrap_or_default(),
        "cwd": ctx.working_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
    });
    let obj = payload.as_object_mut().expect("payload is an object");

    if let Some(path) = &ctx.transcript_path {
        obj.insert(
            "transcript_path".into(),
            serde_json::Value::String(path.display().to_string()),
        );
    }

    match trigger {
        HookTrigger::PreToolUse | HookTrigger::PostToolUse => {
            obj.insert(
                "tool_name".into(),
                serde_json::Value::String(ctx.tool_name.clone()),
            );
            obj.insert("tool_input".into(), parse_or_string(&ctx.tool_input));
            if let Some(output) = &ctx.tool_output {
                obj.insert("tool_response".into(), parse_or_string(output));
            }
        }
        HookTrigger::UserPromptSubmit => {
            obj.insert(
                "prompt".into(),
                serde_json::Value::String(ctx.tool_input.clone()),
            );
        }
        _ => {
            if !ctx.tool_input.is_empty() {
                obj.insert("context".into(), parse_or_string(&ctx.tool_input));
            }
            if let Some(output) = &ctx.tool_output {
                obj.insert("last_message".into(), parse_or_string(output));
            }
        }
    }

    payload
}

/// Parse a string as JSON, falling back to a JSON string value.
fn parse_or_string(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.to_string()))
}

/// Try to parse hook stdout as CC output JSON.
/// Returns `None` if stdout is empty or not a JSON object.
fn parse_cc_output(stdout: &str) -> Option<CcHookOutput> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// CC matcher semantics: `None`/empty/`"*"` matches all; a word matches
/// exactly; `A|B` matches either word; anything else is tried as a regex
/// (invalid regexes match nothing).
fn matches_pattern(matcher: Option<&str>, tool_name: &str) -> bool {
    let Some(pattern) = matcher else {
        return true;
    };
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    if pattern
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
    {
        return pattern.split('|').any(|p| p == tool_name);
    }
    if let Ok(re) = regex::Regex::new(pattern) {
        re.is_match(tool_name)
    } else {
        tracing::warn!(pattern, "invalid regex in hook matcher; matching nothing");
        false
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn def(trigger: HookTrigger, matcher: Option<&str>, command: &str) -> HookDef {
        HookDef {
            trigger,
            matcher: matcher.map(str::to_string),
            command: command.to_string(),
            timeout_secs: 10,
        }
    }

    fn ctx() -> HookContext {
        HookContext {
            tool_name: "Bash".into(),
            tool_input: r#"{"command": "ls"}"#.into(),
            session_id: Some("sess-1".into()),
            ..Default::default()
        }
    }

    // ── Matcher semantics ───────────────────────────────────────────

    #[test]
    fn matcher_none_empty_star_match_all() {
        assert!(matches_pattern(None, "Bash"));
        assert!(matches_pattern(Some(""), "Bash"));
        assert!(matches_pattern(Some("*"), "Bash"));
    }

    #[test]
    fn matcher_exact() {
        assert!(matches_pattern(Some("Bash"), "Bash"));
        assert!(!matches_pattern(Some("Bash"), "Read"));
    }

    #[test]
    fn matcher_pipe_alternatives() {
        assert!(matches_pattern(Some("Edit|Write"), "Edit"));
        assert!(matches_pattern(Some("Edit|Write"), "Write"));
        assert!(!matches_pattern(Some("Edit|Write"), "Read"));
    }

    #[test]
    fn matcher_regex() {
        assert!(matches_pattern(Some("mcp__.*"), "mcp__server__tool"));
        assert!(!matches_pattern(Some("mcp__.*"), "Bash"));
        assert!(matches_pattern(Some("^(Edit|Write)$"), "Edit"));
    }

    #[test]
    fn matcher_invalid_regex_matches_nothing() {
        assert!(!matches_pattern(Some("[unclosed"), "Bash"));
    }

    // ── Config / matching ───────────────────────────────────────────

    #[test]
    fn hook_def_default_timeout_is_cc_default() {
        let json = r#"{"trigger": "PostToolUse", "command": "echo done"}"#;
        let hook: HookDef = serde_json::from_str(json).unwrap();
        assert_eq!(hook.timeout_secs, DEFAULT_HOOK_TIMEOUT_SECS);
    }

    #[test]
    fn matching_hooks_filters_by_trigger_and_matcher() {
        let executor = HookExecutor::with_hooks(vec![
            def(HookTrigger::PreToolUse, None, "all"),
            def(HookTrigger::PreToolUse, Some("Bash"), "bash-only"),
            def(HookTrigger::PostToolUse, None, "post"),
        ]);

        assert_eq!(
            executor
                .matching_hooks(HookTrigger::PreToolUse, "Bash")
                .len(),
            2
        );
        assert_eq!(
            executor
                .matching_hooks(HookTrigger::PreToolUse, "Read")
                .len(),
            1
        );
        assert_eq!(
            executor
                .matching_hooks(HookTrigger::PostToolUse, "Bash")
                .len(),
            1
        );
    }

    #[test]
    fn non_tool_events_ignore_matcher() {
        let executor = HookExecutor::with_hooks(vec![def(
            HookTrigger::UserPromptSubmit,
            Some("Bash"),
            "submit",
        )]);
        assert_eq!(
            executor
                .matching_hooks(HookTrigger::UserPromptSubmit, "anything")
                .len(),
            1
        );
    }

    #[test]
    fn has_matching_hook_predicate() {
        let executor =
            HookExecutor::with_hooks(vec![def(HookTrigger::PreToolUse, Some("Read"), "guard")]);
        assert!(executor.has_matching_hook(HookTrigger::PreToolUse, "Read"));
        assert!(!executor.has_matching_hook(HookTrigger::PreToolUse, "Write"));
        assert!(!executor.has_matching_hook(HookTrigger::PostToolUse, "Read"));
    }

    // ── CC output parsing ───────────────────────────────────────────

    #[test]
    fn parse_cc_output_permission_deny() {
        let cc = parse_cc_output(
            r#"{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "nope"}}"#,
        )
        .unwrap();
        let outcome =
            cc_output_to_outcome(cc, HookTrigger::PreToolUse, String::new(), String::new());
        assert_eq!(outcome.action, HookAction::Deny);
        assert_eq!(outcome.message.as_deref(), Some("nope"));
    }

    #[test]
    fn parse_cc_output_permission_ask() {
        let cc = parse_cc_output(
            r#"{"hookSpecificOutput": {"permissionDecision": "ask", "permissionDecisionReason": "confirm this"}}"#,
        )
        .unwrap();
        let outcome =
            cc_output_to_outcome(cc, HookTrigger::PreToolUse, String::new(), String::new());
        assert_eq!(outcome.action, HookAction::Ask);
    }

    #[test]
    fn parse_cc_output_updated_input() {
        let cc =
            parse_cc_output(r#"{"hookSpecificOutput": {"updatedInput": {"command": "safe"}}}"#)
                .unwrap();
        let outcome =
            cc_output_to_outcome(cc, HookTrigger::PreToolUse, String::new(), String::new());
        assert_eq!(outcome.action, HookAction::Modify);
        assert_eq!(outcome.modified_input.unwrap()["command"], "safe");
    }

    #[test]
    fn parse_cc_output_decision_block() {
        let cc = parse_cc_output(r#"{"decision": "block", "reason": "policy"}"#).unwrap();
        let outcome =
            cc_output_to_outcome(cc, HookTrigger::PreToolUse, String::new(), String::new());
        assert_eq!(outcome.action, HookAction::Deny);
        assert_eq!(outcome.message.as_deref(), Some("policy"));
    }

    #[test]
    fn parse_cc_output_stop_block_is_retry() {
        let cc = parse_cc_output(r#"{"decision": "block", "reason": "keep going"}"#).unwrap();
        let outcome = cc_output_to_outcome(cc, HookTrigger::Stop, String::new(), String::new());
        assert_eq!(outcome.action, HookAction::Retry);
        assert_eq!(outcome.message.as_deref(), Some("keep going"));
    }

    #[test]
    fn parse_cc_output_continue_false() {
        let cc =
            parse_cc_output(r#"{"continue": false, "stopReason": "budget exceeded"}"#).unwrap();
        let outcome =
            cc_output_to_outcome(cc, HookTrigger::PostToolUse, String::new(), String::new());
        assert_eq!(outcome.action, HookAction::Deny);
        assert_eq!(outcome.message.as_deref(), Some("budget exceeded"));
    }

    #[test]
    fn parse_cc_output_additional_context() {
        let cc = parse_cc_output(
            r#"{"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": "today is Friday"}}"#,
        )
        .unwrap();
        let outcome = cc_output_to_outcome(
            cc,
            HookTrigger::UserPromptSubmit,
            String::new(),
            String::new(),
        );
        assert_eq!(outcome.action, HookAction::Allow);
        assert_eq!(
            outcome.additional_context.as_deref(),
            Some("today is Friday")
        );
    }

    #[test]
    fn parse_cc_output_system_message() {
        let cc = parse_cc_output(r#"{"systemMessage": "heads up"}"#).unwrap();
        let outcome =
            cc_output_to_outcome(cc, HookTrigger::PostToolUse, String::new(), String::new());
        assert_eq!(outcome.system_message.as_deref(), Some("heads up"));
    }

    #[test]
    fn parse_cc_output_non_json() {
        assert!(parse_cc_output("").is_none());
        assert!(parse_cc_output("plain text").is_none());
    }

    // ── Stdin payload ───────────────────────────────────────────────

    #[test]
    fn stdin_payload_pre_tool_use() {
        let payload = build_stdin_payload(HookTrigger::PreToolUse, &ctx());
        assert_eq!(payload["hook_event_name"], "PreToolUse");
        assert_eq!(payload["session_id"], "sess-1");
        assert_eq!(payload["tool_name"], "Bash");
        assert_eq!(payload["tool_input"]["command"], "ls");
    }

    #[test]
    fn stdin_payload_post_tool_use_includes_response() {
        let mut c = ctx();
        c.tool_output = Some(r#"{"ok": true}"#.into());
        let payload = build_stdin_payload(HookTrigger::PostToolUse, &c);
        assert_eq!(payload["tool_response"]["ok"], true);
    }

    #[test]
    fn stdin_payload_user_prompt_submit() {
        let c = HookContext {
            tool_input: "fix the bug".into(),
            ..Default::default()
        };
        let payload = build_stdin_payload(HookTrigger::UserPromptSubmit, &c);
        assert_eq!(payload["hook_event_name"], "UserPromptSubmit");
        assert_eq!(payload["prompt"], "fix the bug");
    }

    // ── Aggregation ─────────────────────────────────────────────────

    #[test]
    fn merge_deny_wins() {
        let mut result = HookResult::default();
        let allow = HookOutcome {
            action: HookAction::Allow,
            explicit_allow: false,
            message: None,
            modified_input: None,
            additional_context: None,
            system_message: None,
            stdout: "a".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let deny = HookOutcome {
            action: HookAction::Deny,
            explicit_allow: false,
            message: Some("no".into()),
            modified_input: None,
            additional_context: None,
            system_message: None,
            stdout: "b".into(),
            stderr: String::new(),
            exit_code: 2,
            timed_out: false,
        };
        merge_outcome(&mut result, allow, HookTrigger::PreToolUse);
        merge_outcome(&mut result, deny, HookTrigger::PreToolUse);
        assert_eq!(result.action, HookAction::Deny);
        assert_eq!(result.message.as_deref(), Some("no"));
        assert_eq!(result.stdout, "a\nb");
    }

    #[test]
    fn merge_concatenates_additional_context() {
        let mut result = HookResult::default();
        for text in ["ctx one", "ctx two"] {
            merge_outcome(
                &mut result,
                HookOutcome {
                    action: HookAction::Allow,
                    explicit_allow: false,
                    message: None,
                    modified_input: None,
                    additional_context: Some(text.into()),
                    system_message: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                    timed_out: false,
                },
                HookTrigger::UserPromptSubmit,
            );
        }
        assert_eq!(
            result.additional_context.as_deref(),
            Some("ctx one\nctx two")
        );
    }

    // ── End-to-end process tests ────────────────────────────────────

    #[tokio::test]
    async fn run_no_hooks_returns_allowed() {
        let exec = HookExecutor::new();
        let result = exec.run(HookTrigger::PreToolUse, &ctx()).await.unwrap();
        assert!(result.is_allowed());
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn run_pre_hook_success() {
        let exec = HookExecutor::with_hooks(vec![def(HookTrigger::PreToolUse, None, "echo ok")]);
        let result = exec.run(HookTrigger::PreToolUse, &ctx()).await.unwrap();
        assert!(result.is_allowed());
        assert!(result.stdout.contains("ok"));
    }

    #[tokio::test]
    async fn run_exit_code_2_blocks_with_stderr() {
        let cmd = if cfg!(windows) {
            "echo blocked by test 1>&2 & exit 2"
        } else {
            "echo 'blocked by test' >&2; exit 2"
        };
        let exec = HookExecutor::with_hooks(vec![def(HookTrigger::PreToolUse, None, cmd)]);
        let result = exec.run(HookTrigger::PreToolUse, &ctx()).await.unwrap();
        assert_eq!(result.action, HookAction::Deny);
        assert!(result.message.unwrap().contains("blocked by test"));
    }

    #[tokio::test]
    async fn run_exit_code_1_is_non_blocking() {
        let exec = HookExecutor::with_hooks(vec![def(HookTrigger::PreToolUse, None, "exit 1")]);
        let result = exec.run(HookTrigger::PreToolUse, &ctx()).await.unwrap();
        assert!(result.is_allowed());
        assert_eq!(result.exit_code, 1);
    }

    #[tokio::test]
    async fn run_hook_receives_stdin_payload() {
        let cmd = if cfg!(windows) {
            // findstr echoes matching stdin lines
            "findstr PreToolUse"
        } else {
            "grep -o PreToolUse | head -1"
        };
        let exec = HookExecutor::with_hooks(vec![def(HookTrigger::PreToolUse, None, cmd)]);
        let result = exec.run(HookTrigger::PreToolUse, &ctx()).await.unwrap();
        assert!(
            result.stdout.contains("PreToolUse"),
            "hook should see the payload on stdin; got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn run_hook_matcher_skips_other_tools() {
        let exec =
            HookExecutor::with_hooks(vec![def(HookTrigger::PreToolUse, Some("Edit"), "exit 2")]);
        let result = exec.run(HookTrigger::PreToolUse, &ctx()).await.unwrap();
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn run_stop_hook_exit_2_retries() {
        let cmd = if cfg!(windows) {
            "echo keep working 1>&2 & exit 2"
        } else {
            "echo 'keep working' >&2; exit 2"
        };
        let exec = HookExecutor::with_hooks(vec![def(HookTrigger::Stop, None, cmd)]);
        let result = exec.run(HookTrigger::Stop, &ctx()).await.unwrap();
        assert_eq!(result.action, HookAction::Retry);
    }
}
