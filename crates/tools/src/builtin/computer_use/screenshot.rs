//! Screenshot capture for the computer-use subsystem.
//!
//! Platform integration is not yet available, so all functions return
//! a human-readable "not available" message instead of actual screen data.

/// Result of a screenshot capture attempt.
///
/// Dead-code suppressed: returned by `capture_screenshot`/`capture_region`,
/// consumed when platform integration is available.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ScreenshotResult {
    /// Whether the capture succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
    /// Raw image bytes (empty when platform integration is unavailable).
    pub data: Vec<u8>,
}

/// Attempt to capture a screenshot of the current display.
///
/// Returns a [`ScreenshotResult`] indicating that platform integration
/// is not yet available.
///
/// Dead-code suppressed: called by `ComputerUseTool` via runtime dispatch.
#[allow(dead_code)]
pub fn capture_screenshot() -> ScreenshotResult {
    ScreenshotResult {
        success: false,
        message: "Screenshot capture is not available without platform integration".into(),
        data: Vec::new(),
    }
}

/// Attempt to capture a screenshot of a specific screen region.
///
/// The `_x`, `_y`, `_width`, and `_height` parameters describe the desired
/// region in logical pixels. Currently returns an unavailability message.
///
/// Dead-code suppressed: planned for a region-capture action in `ComputerUseTool`.
#[allow(dead_code)]
pub fn capture_region(_x: u32, _y: u32, _width: u32, _height: u32) -> ScreenshotResult {
    ScreenshotResult {
        success: false,
        message: "Region screenshot capture is not available without platform integration".into(),
        data: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_screenshot_returns_unavailable() {
        let result = capture_screenshot();
        assert!(!result.success);
        assert!(result.message.contains("not available"));
        assert!(result.data.is_empty());
    }

    #[test]
    fn capture_region_returns_unavailable() {
        let result = capture_region(0, 0, 1920, 1080);
        assert!(!result.success);
        assert!(result.message.contains("not available"));
        assert!(result.data.is_empty());
    }
}
