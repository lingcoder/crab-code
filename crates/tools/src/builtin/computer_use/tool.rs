//! The [`ComputerUseTool`] — the one-entry-point tool that dispatches
//! screenshot / input / window actions to their respective sub-modules.

use std::future::Future;
use std::pin::Pin;

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;

use super::{input, screenshot, window};

/// Tool name constant for [`ComputerUseTool`].
pub const COMPUTER_USE_TOOL_NAME: &str = "ComputerUse";

/// Desktop automation tool that dispatches to screenshot, input, and window
/// subsystems.
///
/// Supported actions (passed via the `action` input field):
/// - `screenshot` — capture the current screen
/// - `input` — simulate keyboard or mouse events
/// - `list_windows` — enumerate visible desktop windows
pub struct ComputerUseTool;

impl Tool for ComputerUseTool {
    fn name(&self) -> &'static str {
        COMPUTER_USE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Desktop automation: take screenshots, simulate keyboard/mouse input, and \
         list windows. Requires platform integration to be available."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The action to perform: screenshot, input, list_windows",
                    "enum": ["screenshot", "input", "list_windows"]
                },
                "input_type": {
                    "type": "string",
                    "description": "For 'input' action: key_press, type_text, mouse_move, mouse_click"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type or key to press"
                },
                "x": {
                    "type": "integer",
                    "description": "X coordinate for mouse_move or screenshot region"
                },
                "y": {
                    "type": "integer",
                    "description": "Y coordinate for mouse_move or screenshot region"
                }
            },
            "required": ["action"]
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        input["action"]
            .as_str()
            .map(|a| format!("ComputerUse ({a})"))
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let action = input["action"].as_str().unwrap_or("").to_owned();
        let input_type = input["input_type"].as_str().unwrap_or("").to_owned();
        let text = input["text"].as_str().unwrap_or("").to_owned();
        let x = input["x"].as_i64().unwrap_or(0);
        let y = input["y"].as_i64().unwrap_or(0);

        Box::pin(async move {
            match action.as_str() {
                "screenshot" => {
                    let result = screenshot::capture_screenshot();
                    Ok(ToolOutput::success(result.message))
                }
                "input" => {
                    let event = match input_type.as_str() {
                        "key_press" => input::InputEvent::KeyPress(text),
                        "type_text" => input::InputEvent::TypeText(text),
                        "mouse_move" => input::InputEvent::MouseMove {
                            x: i32::try_from(x).unwrap_or(0),
                            y: i32::try_from(y).unwrap_or(0),
                        },
                        "mouse_click" => input::InputEvent::MouseClick {
                            button: input::MouseButton::Left,
                        },
                        other => {
                            return Ok(ToolOutput::error(format!(
                                "Unknown input_type: '{other}'. \
                                 Expected: key_press, type_text, mouse_move, mouse_click"
                            )));
                        }
                    };
                    let result = input::simulate_input(&event);
                    Ok(ToolOutput::success(result.message))
                }
                "list_windows" => {
                    let result = window::list_windows();
                    Ok(ToolOutput::success(result.message))
                }
                other => Ok(ToolOutput::error(format!(
                    "Unknown action: '{other}'. Expected: screenshot, input, list_windows"
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = ComputerUseTool;
        assert_eq!(tool.name(), "ComputerUse");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_action() {
        let schema = ComputerUseTool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["action"]));
        assert!(schema["properties"]["action"].is_object());
    }

    #[tokio::test]
    async fn screenshot_action_returns_not_available() {
        let tool = ComputerUseTool;
        let result = tool
            .execute(serde_json::json!({"action": "screenshot"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("not available"));
    }

    #[tokio::test]
    async fn input_action_returns_not_available() {
        let tool = ComputerUseTool;
        let result = tool
            .execute(
                serde_json::json!({"action": "input", "input_type": "key_press", "text": "Enter"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("not available"));
    }

    #[tokio::test]
    async fn list_windows_action_returns_not_available() {
        let tool = ComputerUseTool;
        let result = tool
            .execute(serde_json::json!({"action": "list_windows"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("not available"));
    }

    #[tokio::test]
    async fn unknown_action_returns_error() {
        let tool = ComputerUseTool;
        let result = tool
            .execute(serde_json::json!({"action": "fly"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("Unknown action"));
    }

    #[tokio::test]
    async fn unknown_input_type_returns_error() {
        let tool = ComputerUseTool;
        let result = tool
            .execute(
                serde_json::json!({"action": "input", "input_type": "telekinesis"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("Unknown input_type"));
    }
}
