//! Window enumeration and management for the computer-use subsystem.
//!
//! Platform integration is not yet available, so all functions return
//! a human-readable "not available" message.

/// Metadata for a single window on the host desktop.
///
/// Dead-code suppressed: populated when platform integration (Win32/X11) is enabled.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Platform-specific window identifier.
    pub id: u64,
    /// Window title text.
    pub title: String,
    /// Position and size in logical pixels.
    pub bounds: WindowBounds,
    /// Whether the window is currently focused.
    pub focused: bool,
}

/// Position and size of a window.
///
/// Dead-code suppressed: part of the `WindowInfo` struct, populated by platform backends.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct WindowBounds {
    /// X-coordinate of the top-left corner.
    pub x: i32,
    /// Y-coordinate of the top-left corner.
    pub y: i32,
    /// Width in logical pixels.
    pub width: u32,
    /// Height in logical pixels.
    pub height: u32,
}

/// Result of a window enumeration attempt.
///
/// Dead-code suppressed: returned by `list_windows`/`focus_window`, used
/// when platform integration is available.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct WindowListResult {
    /// Whether the query succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
    /// Discovered windows (empty when platform integration is unavailable).
    pub windows: Vec<WindowInfo>,
}

/// List all visible windows on the host desktop.
///
/// Returns a [`WindowListResult`] indicating that platform integration
/// is not yet available.
///
/// Dead-code suppressed: called by `ComputerUseTool` when the `list_windows`
/// action is requested; the `#[allow]` is needed because the tool dispatches
/// at runtime, not compile time.
#[allow(dead_code)]
pub fn list_windows() -> WindowListResult {
    WindowListResult {
        success: false,
        message: "Window listing is not available without platform integration".into(),
        windows: Vec::new(),
    }
}

/// Attempt to focus a specific window by its platform identifier.
///
/// Returns a human-readable message about the attempt.
///
/// Dead-code suppressed: planned for the `focus_window` action in `ComputerUseTool`.
#[allow(dead_code)]
pub fn focus_window(_window_id: u64) -> WindowListResult {
    WindowListResult {
        success: false,
        message: "Window focus is not available without platform integration".into(),
        windows: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_windows_returns_unavailable() {
        let result = list_windows();
        assert!(!result.success);
        assert!(result.message.contains("not available"));
        assert!(result.windows.is_empty());
    }

    #[test]
    fn focus_window_returns_unavailable() {
        let result = focus_window(12345);
        assert!(!result.success);
        assert!(result.message.contains("not available"));
        assert!(result.windows.is_empty());
    }

    #[test]
    fn window_info_fields() {
        let info = WindowInfo {
            id: 42,
            title: "Test Window".into(),
            bounds: WindowBounds {
                x: 10,
                y: 20,
                width: 800,
                height: 600,
            },
            focused: true,
        };
        assert_eq!(info.id, 42);
        assert_eq!(info.title, "Test Window");
        assert!(info.focused);
        assert_eq!(info.bounds.width, 800);
    }
}
