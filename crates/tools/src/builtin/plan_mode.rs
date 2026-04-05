//! `EnterPlanMode` tool — switches the agent into planning mode.
//!
//! When invoked, this tool signals that the agent should enter a structured
//! planning phase (e.g., outlining steps before executing). The actual mode
//! transition is handled by the agent loop; this tool returns a confirmation.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;

use super::plan_file::{self, PlanFile};

/// Tracks plan execution progress alongside the plan mode tool.
#[derive(Debug, Clone)]
pub struct PlanProgress {
    /// The parsed plan being tracked.
    pub plan: PlanFile,
    /// Whether auto-completion tracking is enabled.
    pub auto_track: bool,
}

impl PlanProgress {
    /// Create a new progress tracker from a `PlanFile`.
    #[must_use]
    pub fn new(plan: PlanFile) -> Self {
        Self {
            plan,
            auto_track: true,
        }
    }

    /// Mark a step as completed by section and step index. Returns true if
    /// the step existed and was marked.
    pub fn complete_step(&mut self, section: usize, step: usize) -> bool {
        self.plan.complete_step(section, step)
    }

    /// Get a Markdown summary of current progress.
    #[must_use]
    pub fn progress_summary(&self) -> String {
        format!(
            "Progress: {}/{} steps ({}%)",
            self.plan.completed_steps(),
            self.plan.total_steps(),
            self.plan.completion_pct()
        )
    }

    /// Whether all steps are done.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.plan.is_complete()
    }

    /// Render the full plan with status markers.
    #[must_use]
    pub fn render(&self) -> String {
        plan_file::render_plan(&self.plan)
    }
}

/// Tool that triggers a transition to plan mode in the agent session.
pub struct EnterPlanModeTool;

impl Tool for EnterPlanModeTool {
    fn name(&self) -> &'static str {
        "enter_plan_mode"
    }

    fn description(&self) -> &'static str {
        "Switch the agent into planning mode. In plan mode, the agent outlines \
         a structured plan before executing any changes. Use this when facing a \
         complex task that benefits from upfront planning. Optionally provide an \
         initial plan description."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Optional description of what the plan should cover"
                },
                "steps": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional initial list of planned steps"
                },
                "allowed_prompts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Restrict allowed user prompts while in plan mode (e.g., approve, reject, revise)"
                },
                "enable_tracking": {
                    "type": "boolean",
                    "description": "Enable automatic step completion tracking (default: true)"
                }
            },
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let description = input["description"].as_str().unwrap_or("").to_owned();
        let steps = parse_steps(&input["steps"]);
        let allowed_prompts = parse_steps(&input["allowed_prompts"]);
        let enable_tracking = input["enable_tracking"].as_bool().unwrap_or(true);

        Box::pin(async move {
            let mut output = String::from("[Plan Mode Activated]");

            if !description.is_empty() {
                let _ = write!(output, "\n\nObjective: {description}");
            }

            if !steps.is_empty() {
                output.push_str("\n\nPlanned steps:");
                for (i, step) in steps.iter().enumerate() {
                    let _ = write!(output, "\n  {}. {step}", i + 1);
                }
            }

            if !allowed_prompts.is_empty() {
                output.push_str("\n\nAllowed prompts: ");
                output.push_str(&allowed_prompts.join(", "));
            }

            if enable_tracking {
                output.push_str("\n\nCompletion tracking: enabled");
            }

            // Build a PlanFile from the steps for progress tracking
            if !steps.is_empty() {
                let md = build_plan_markdown(&description, &steps);
                let plan = plan_file::parse_plan(&md);
                let progress = PlanProgress::new(plan);
                let _ = write!(output, "\n\n{}", progress.progress_summary());
            }

            Ok(ToolOutput::success(output))
        })
    }
}

/// Parse a JSON array of strings into step descriptions.
fn parse_steps(value: &Value) -> Vec<String> {
    value.as_array().map_or_else(Vec::new, |arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
}

/// Build a simple Markdown plan from a description and step list.
fn build_plan_markdown(description: &str, steps: &[String]) -> String {
    let title = if description.is_empty() {
        "Plan"
    } else {
        description
    };
    let mut md = format!("# {title}\n\n## Steps\n");
    for step in steps {
        md.push_str(&format!("- [ ] {step}\n"));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::tool::ToolContext;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
        }
    }

    #[tokio::test]
    async fn basic_plan_mode() {
        let tool = EnterPlanModeTool;
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("Plan Mode Activated"));
    }

    #[tokio::test]
    async fn plan_with_description() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(
                json!({"description": "Refactor the auth module"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Plan Mode Activated"));
        assert!(text.contains("Objective: Refactor the auth module"));
    }

    #[tokio::test]
    async fn plan_with_steps() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(
                json!({
                    "description": "Add caching",
                    "steps": ["Audit current queries", "Add Redis layer", "Write tests"]
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("1. Audit current queries"));
        assert!(text.contains("2. Add Redis layer"));
        assert!(text.contains("3. Write tests"));
    }

    #[tokio::test]
    async fn plan_empty_description_and_steps() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(json!({"description": "", "steps": []}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("[Plan Mode Activated]"));
        assert!(text.contains("Completion tracking: enabled"));
    }

    #[tokio::test]
    async fn plan_with_tracking_disabled() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(json!({"enable_tracking": false}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Plan Mode Activated"));
        assert!(!text.contains("Completion tracking"));
    }

    #[tokio::test]
    async fn plan_with_allowed_prompts() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(
                json!({
                    "allowed_prompts": ["approve", "reject", "revise"]
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Allowed prompts: approve, reject, revise"));
    }

    #[tokio::test]
    async fn plan_with_steps_shows_progress() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(
                json!({
                    "description": "Test plan",
                    "steps": ["Step A", "Step B"]
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Progress: 0/2 steps (0%)"));
    }

    #[tokio::test]
    async fn schema_has_no_required_fields() {
        let tool = EnterPlanModeTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!([]));
        assert!(schema["properties"]["description"].is_object());
        assert!(schema["properties"]["steps"].is_object());
        assert!(schema["properties"]["allowed_prompts"].is_object());
        assert!(schema["properties"]["enable_tracking"].is_object());
    }

    #[test]
    fn tool_metadata() {
        let tool = EnterPlanModeTool;
        assert_eq!(tool.name(), "enter_plan_mode");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn parse_steps_empty() {
        assert!(parse_steps(&json!(null)).is_empty());
        assert!(parse_steps(&json!([])).is_empty());
    }

    #[test]
    fn parse_steps_filters_non_strings() {
        let steps = parse_steps(&json!(["a", 1, "b", null, "c"]));
        assert_eq!(steps, vec!["a", "b", "c"]);
    }

    #[test]
    fn build_plan_markdown_with_description() {
        let md = build_plan_markdown("My Plan", &["Step 1".into(), "Step 2".into()]);
        assert!(md.contains("# My Plan"));
        assert!(md.contains("- [ ] Step 1"));
        assert!(md.contains("- [ ] Step 2"));
    }

    #[test]
    fn build_plan_markdown_empty_description() {
        let md = build_plan_markdown("", &["Do something".into()]);
        assert!(md.contains("# Plan"));
    }

    #[test]
    fn plan_progress_new() {
        let plan = plan_file::parse_plan("# Test\n## S\n- [ ] A\n- [ ] B\n");
        let progress = PlanProgress::new(plan);
        assert!(!progress.is_complete());
        assert!(progress.auto_track);
        assert!(progress.progress_summary().contains("0/2"));
    }

    #[test]
    fn plan_progress_complete_step() {
        let plan = plan_file::parse_plan("# Test\n## S\n- [ ] A\n- [ ] B\n");
        let mut progress = PlanProgress::new(plan);
        assert!(progress.complete_step(0, 0));
        assert!(progress.progress_summary().contains("1/2"));
        assert!(!progress.is_complete());
        assert!(progress.complete_step(0, 1));
        assert!(progress.is_complete());
    }

    #[test]
    fn plan_progress_render() {
        let plan = plan_file::parse_plan("# Test\n## S\n- [x] Done\n- [ ] Todo\n");
        let progress = PlanProgress::new(plan);
        let rendered = progress.render();
        assert!(rendered.contains("[x] Done"));
        assert!(rendered.contains("[ ] Todo"));
    }
}
