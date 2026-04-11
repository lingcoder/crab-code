//! Overlay system — modal view stack for command palette, dialogs, etc.
//!
//! Overlays are pushed onto a stack. The topmost overlay receives input first.
//! If it doesn't consume the input, it falls through to the next layer.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::app_event::AppEvent;
use crate::keybindings::KeyContext;
use crate::traits::Renderable;

/// Result of an overlay handling a key event.
#[derive(Debug)]
pub enum OverlayAction {
    /// The overlay consumed the key event — no further handling.
    Consumed,
    /// The overlay wants to dismiss itself.
    Dismiss,
    /// The overlay produced an `AppEvent` to apply.
    Execute(AppEvent),
    /// The overlay did not handle the key — pass it to the next layer.
    Passthrough,
}

/// Trait for modal overlay views (command palette, history search, etc.).
pub trait Overlay: Renderable + Send {
    /// Handle a key event. Return how it was handled.
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;

    /// Which keybinding contexts this overlay activates.
    fn contexts(&self) -> Vec<KeyContext>;

    /// Human-readable name for debugging.
    fn name(&self) -> &'static str;
}

/// Stack of active overlays. Topmost gets input first.
pub struct OverlayStack {
    stack: Vec<Box<dyn Overlay>>,
}

impl OverlayStack {
    /// Create an empty overlay stack.
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a new overlay onto the stack.
    pub fn push(&mut self, overlay: Box<dyn Overlay>) {
        self.stack.push(overlay);
    }

    /// Pop the topmost overlay, returning it.
    pub fn pop(&mut self) -> Option<Box<dyn Overlay>> {
        self.stack.pop()
    }

    /// Whether any overlays are active.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Number of active overlays.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Handle a key event — route to topmost overlay first.
    ///
    /// Returns `Some(AppEvent)` if the overlay produced one, or `None`
    /// if the event was consumed or should pass through.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<OverlayAction> {
        if let Some(overlay) = self.stack.last_mut() {
            let action = overlay.handle_key(key);
            match action {
                OverlayAction::Dismiss => {
                    self.stack.pop();
                    Some(OverlayAction::Consumed)
                }
                OverlayAction::Passthrough => None,
                other => Some(other),
            }
        } else {
            None
        }
    }

    /// Get active keybinding contexts from the overlay stack.
    pub fn active_contexts(&self) -> Vec<KeyContext> {
        self.stack.iter().flat_map(|o| o.contexts()).collect()
    }

    /// Render all overlays (bottom to top).
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        for overlay in &self.stack {
            overlay.render(area, buf);
        }
    }
}

impl Default for OverlayStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub overlay for testing.
    struct TestOverlay {
        consume: bool,
    }

    impl Renderable for TestOverlay {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn desired_height(&self, _width: u16) -> u16 {
            5
        }
    }

    impl Overlay for TestOverlay {
        fn handle_key(&mut self, _key: KeyEvent) -> OverlayAction {
            if self.consume {
                OverlayAction::Consumed
            } else {
                OverlayAction::Passthrough
            }
        }
        fn contexts(&self) -> Vec<KeyContext> {
            vec![KeyContext::CommandPalette]
        }
        fn name(&self) -> &'static str {
            "test"
        }
    }

    #[test]
    fn overlay_stack_empty() {
        let stack = OverlayStack::new();
        assert!(stack.is_empty());
    }

    #[test]
    fn overlay_stack_push_pop() {
        let mut stack = OverlayStack::new();
        stack.push(Box::new(TestOverlay { consume: true }));
        assert!(!stack.is_empty());
        stack.pop();
        assert!(stack.is_empty());
    }

    #[test]
    fn overlay_stack_handle_key_passthrough() {
        let mut stack = OverlayStack::new();
        stack.push(Box::new(TestOverlay { consume: false }));

        let key = KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        );
        let result = stack.handle_key(key);
        assert!(result.is_none()); // passthrough
    }

    #[test]
    fn overlay_stack_handle_key_consumed() {
        let mut stack = OverlayStack::new();
        stack.push(Box::new(TestOverlay { consume: true }));

        let key = KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        );
        let result = stack.handle_key(key);
        assert!(matches!(result, Some(OverlayAction::Consumed)));
    }

    #[test]
    fn overlay_stack_active_contexts() {
        let mut stack = OverlayStack::new();
        assert!(stack.active_contexts().is_empty());

        stack.push(Box::new(TestOverlay { consume: true }));
        let contexts = stack.active_contexts();
        assert!(contexts.contains(&KeyContext::CommandPalette));
    }
}
