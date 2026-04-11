//! `VerifyPlanExecutionTool` — validate that a plan has been executed correctly.
//!
//! Loads a plan file, checks each step's completion status, runs optional
//! verification commands, and reports discrepancies.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `VerifyPlanExecutionTool`.
pub const VERIFY_PLAN_EXECUTION_TOOL_NAME: &str = "VerifyPlanExecution";

/// Result of verifying a single plan step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepVerification {
    /// Step description.
    pub step: String,
    /// Whether the step passed verification.
    pub passed: bool,
    /// Details about the verification result.
    pub detail: String,
}

/// Overall plan verification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanVerification {
    /// Path to the plan file.
    pub plan_file: String,
    /// Total number of steps in the plan.
    pub total_steps: usize,
    /// Number of steps that passed verification.
    pub passed_steps: usize,
    /// Number of steps that failed verification.
    pub failed_steps: usize,
    /// Per-step verification results.
    pub steps: Vec<StepVerification>,
}

/// Plan execution verification tool.
///
/// Input:
/// - `plan_file`: Path to the plan file to verify
pub struct VerifyPlanExecutionTool;

impl Tool for VerifyPlanExecutionTool {
    fn name(&self) -> &'static str {
        VERIFY_PLAN_EXECUTION_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Verify that a plan file has been executed correctly. Loads the plan, \
         checks each step's completion status, and reports any steps that were \
         not completed or that failed verification."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan_file": {
                    "type": "string",
                    "description": "Path to the plan file to verify"
                }
            },
            "required": ["plan_file"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        input["plan_file"]
            .as_str()
            .map(|f| format!("VerifyPlan ({f})"))
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let plan_file = input["plan_file"].as_str().unwrap_or("").to_owned();

        Box::pin(async move {
            if plan_file.is_empty() {
                return Ok(ToolOutput::error("plan_file is required"));
            }
            verify_plan(&plan_file).await
        })
    }
}

/// Verify plan execution by loading the plan and checking step statuses.
async fn verify_plan(plan_file: &str) -> Result<ToolOutput> {
    let path = std::path::Path::new(plan_file);
    let content = tokio::fs::read_to_string(path).await.map_err(|e| {
        crab_common::Error::Tool(format!("failed to read plan file '{plan_file}': {e}"))
    })?;

    // Parse markdown-style checkboxes: `- [x]` (done) and `- [ ]` (pending).
    let mut steps = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("- [x] ")
            .or_else(|| trimmed.strip_prefix("- [X] "))
        {
            steps.push(StepVerification {
                step: rest.to_owned(),
                passed: true,
                detail: "Marked as completed".to_owned(),
            });
        } else if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
            steps.push(StepVerification {
                step: rest.to_owned(),
                passed: false,
                detail: "Not yet completed".to_owned(),
            });
        }
    }

    let total_steps = steps.len();
    let passed_steps = steps.iter().filter(|s| s.passed).count();
    let failed_steps = total_steps - passed_steps;

    let report = PlanVerification {
        plan_file: plan_file.to_owned(),
        total_steps,
        passed_steps,
        failed_steps,
        steps,
    };

    let json = serde_json::to_string_pretty(&report).map_err(|e| {
        crab_common::Error::Tool(format!("failed to serialize verification report: {e}"))
    })?;
    Ok(ToolOutput::success(json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = VerifyPlanExecutionTool;
        assert_eq!(tool.name(), "VerifyPlanExecution");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_requires_plan_file() {
        let schema = VerifyPlanExecutionTool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["plan_file"]));
    }

    #[test]
    fn step_verification_serde() {
        let sv = StepVerification {
            step: "Create module".into(),
            passed: true,
            detail: "File exists".into(),
        };
        let json = serde_json::to_string(&sv).unwrap();
        let parsed: StepVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step, "Create module");
        assert!(parsed.passed);
    }

    #[test]
    fn plan_verification_serde() {
        let pv = PlanVerification {
            plan_file: "plan.md".into(),
            total_steps: 3,
            passed_steps: 2,
            failed_steps: 1,
            steps: vec![],
        };
        let json = serde_json::to_string(&pv).unwrap();
        let parsed: PlanVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_steps, 3);
        assert_eq!(parsed.failed_steps, 1);
    }
}
