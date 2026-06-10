//! Keyboard and mouse input simulation for the computer-use subsystem.
//!
//! Platform integration is not yet available, so all functions return
//! a human-readable "not available" message.

/// The kind of input event to simulate.
///
/// Dead-code suppressed: variants are matched at runtime in `simulate_input`
/// and `ComputerUseTool::execute`.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// Press and release a key (e.g. "Enter", "a", "Ctrl+C").
    KeyPress(String),
    /// Type a string of text character by character.
    TypeText(String),
    /// Move the mouse to logical-pixel coordinates.
    MouseMove { x: i32, y: i32 },
    /// Click a mouse button at the current position.
    MouseClick { button: MouseButton },
}

/// Mouse button identifier.
///
/// Dead-code suppressed: used by `InputEvent::MouseClick`, dispatched at runtime.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary (left) button.
    Left,
    /// Secondary (right) button.
    Right,
    /// Middle button (scroll wheel press).
    Middle,
}

/// Result of an input simulation attempt.
///
/// Dead-code suppressed: returned by `simulate_input`, consumed by `ComputerUseTool`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct InputResult {
    /// Whether the simulation succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
}

/// Simulate an input event on the host system.
///
/// Returns an [`InputResult`] indicating that platform integration is
/// not yet available.
///
/// Dead-code suppressed: called by `ComputerUseTool` via runtime dispatch.
#[allow(dead_code)]
pub fn simulate_input(event: &InputEvent) -> InputResult {
    let detail = match event {
        InputEvent::KeyPress(key) => format!("key press '{key}'"),
        InputEvent::TypeText(text) => format!("type text ({} chars)", text.len()),
        InputEvent::MouseMove { x, y } => format!("mouse move to ({x}, {y})"),
        InputEvent::MouseClick { button } => format!("mouse click {button:?}"),
    };
    InputResult {
        success: false,
        message: format!(
            "Input simulation ({detail}) is not available without platform integration"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_key_press_returns_unavailable() {
        let result = simulate_input(&InputEvent::KeyPress("Enter".into()));
        assert!(!result.success);
        assert!(result.message.contains("not available"));
        assert!(result.message.contains("key press"));
    }

    #[test]
    fn simulate_type_text_returns_unavailable() {
        let result = simulate_input(&InputEvent::TypeText("hello world".into()));
        assert!(!result.success);
        assert!(result.message.contains("not available"));
        assert!(result.message.contains("11 chars"));
    }

    #[test]
    fn simulate_mouse_move_returns_unavailable() {
        let result = simulate_input(&InputEvent::MouseMove { x: 100, y: 200 });
        assert!(!result.success);
        assert!(result.message.contains("mouse move"));
    }

    #[test]
    fn simulate_mouse_click_returns_unavailable() {
        let result = simulate_input(&InputEvent::MouseClick {
            button: MouseButton::Left,
        });
        assert!(!result.success);
        assert!(result.message.contains("mouse click"));
    }
}
