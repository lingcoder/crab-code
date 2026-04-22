//! FIFO approval queue — handles multiple concurrent permission requests.
//!
//! When the agent fires multiple tool calls that require approval, they
//! queue up here. The user sees "1/N" indicator and processes them in order.

use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::permission::{PermissionCard, PermissionResponse};

/// A pending approval request in the queue.
pub struct PendingApproval {
    /// The permission card for this request.
    pub card: PermissionCard,
    /// Whether to show the explanation panel.
    pub show_explanation: bool,
    /// Whether to show debug info.
    pub show_debug: bool,
}

impl PendingApproval {
    /// Create a new pending approval.
    pub fn new(card: PermissionCard) -> Self {
        Self {
            card,
            show_explanation: false,
            show_debug: false,
        }
    }

    /// Toggle the explanation panel.
    pub fn toggle_explanation(&mut self) {
        self.show_explanation = !self.show_explanation;
    }

    /// Toggle the debug info panel.
    pub fn toggle_debug(&mut self) {
        self.show_debug = !self.show_debug;
    }
}

/// FIFO queue of pending permission approvals.
pub struct ApprovalQueue {
    /// Pending approvals (front = next to process).
    pending: VecDeque<PendingApproval>,
}

impl ApprovalQueue {
    /// Create an empty queue.
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
        }
    }

    /// Enqueue a new approval request.
    pub fn push(&mut self, card: PermissionCard) {
        self.pending.push_back(PendingApproval::new(card));
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Number of pending approvals.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Get a mutable reference to the front (current) approval.
    pub fn current_mut(&mut self) -> Option<&mut PendingApproval> {
        self.pending.front_mut()
    }

    /// Get a reference to the front (current) approval.
    pub fn current(&self) -> Option<&PendingApproval> {
        self.pending.front()
    }

    /// Reject all pending approvals, returning their request IDs.
    ///
    /// Used when Ctrl+C is pressed during `Confirming` state — every
    /// queued permission is denied and the engine loop gets interrupted.
    pub fn reject_all(&mut self) -> Vec<String> {
        self.pending
            .drain(..)
            .map(|pa| pa.card.request_id)
            .collect()
    }

    /// Handle a key event on the current approval.
    ///
    /// Returns the request ID and response if the user made a decision.
    pub fn handle_key(
        &mut self,
        code: crossterm::event::KeyCode,
    ) -> Option<(String, PermissionResponse)> {
        // Ctrl+E / Ctrl+D toggle explanation/debug on current card
        // (handled by caller based on key modifiers)

        let front = self.pending.front_mut()?;
        let response = front.card.handle_key(code)?;
        let request_id = front.card.request_id.clone();
        self.pending.pop_front();
        Some((request_id, response))
    }

    /// Render the current approval card with queue indicator.
    #[allow(clippy::cast_possible_truncation)]
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let Some(current) = self.current() else {
            return;
        };

        // If multiple, show "1/N" indicator at top
        if self.pending.len() > 1 {
            let indicator = format!("  Request 1/{}", self.pending.len());
            let indicator_line = Line::from(Span::styled(
                indicator,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            if area.height > 0 {
                Widget::render(
                    indicator_line,
                    Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width,
                        height: 1,
                    },
                    buf,
                );
            }

            // Render card below indicator
            let card_area = Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: area.height.saturating_sub(1),
            };
            Widget::render(&current.card, card_area, buf);
        } else {
            Widget::render(&current.card, area, buf);
        }

        // Explanation panel
        if current.show_explanation && area.height > 2 {
            let explanation = Line::from(Span::styled(
                "  This tool requires permission to execute.",
                Style::default().fg(Color::DarkGray),
            ));
            let y = area.y + area.height.saturating_sub(2);
            Widget::render(
                Paragraph::new(explanation),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }
}

impl Default for ApprovalQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_card(tool: &str, summary: &str, id: &str) -> PermissionCard {
        PermissionCard::from_event(tool, summary, id.into())
    }

    #[test]
    fn queue_empty() {
        let queue = ApprovalQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn queue_push_and_process() {
        let mut queue = ApprovalQueue::new();
        queue.push(test_card("bash", "ls", "r1"));
        queue.push(test_card("edit", "file.rs", "r2"));
        assert_eq!(queue.len(), 2);
        assert!(!queue.is_empty());

        // Process first
        let result = queue.handle_key(crossterm::event::KeyCode::Char('y'));
        assert!(result.is_some());
        let (id, response) = result.unwrap();
        assert_eq!(id, "r1");
        assert_eq!(response, PermissionResponse::Allow);
        assert_eq!(queue.len(), 1);

        // Process second
        let result = queue.handle_key(crossterm::event::KeyCode::Char('n'));
        assert!(result.is_some());
        let (id, response) = result.unwrap();
        assert_eq!(id, "r2");
        assert_eq!(response, PermissionResponse::Deny);
        assert!(queue.is_empty());
    }

    #[test]
    fn pending_approval_toggle() {
        let mut approval = PendingApproval::new(test_card("bash", "ls", "r1"));
        assert!(!approval.show_explanation);
        approval.toggle_explanation();
        assert!(approval.show_explanation);
        approval.toggle_debug();
        assert!(approval.show_debug);
    }

    #[test]
    fn queue_render_does_not_panic() {
        let mut queue = ApprovalQueue::new();
        queue.push(test_card("bash", "ls", "r1"));
        queue.push(test_card("edit", "file.rs", "r2"));

        let area = Rect::new(0, 0, 60, 15);
        let mut buf = Buffer::empty(area);
        queue.render(area, &mut buf);
    }

    #[test]
    fn empty_queue_render_does_not_panic() {
        let queue = ApprovalQueue::new();
        let area = Rect::new(0, 0, 60, 15);
        let mut buf = Buffer::empty(area);
        queue.render(area, &mut buf);
    }

    #[test]
    fn reject_all_drains_queue() {
        let mut queue = ApprovalQueue::new();
        queue.push(test_card("bash", "ls", "r1"));
        queue.push(test_card("edit", "file.rs", "r2"));
        queue.push(test_card("write", "out.txt", "r3"));

        let ids = queue.reject_all();
        assert_eq!(ids, vec!["r1", "r2", "r3"]);
        assert!(queue.is_empty());
    }

    #[test]
    fn reject_all_empty_queue() {
        let mut queue = ApprovalQueue::new();
        let ids = queue.reject_all();
        assert!(ids.is_empty());
        assert!(queue.is_empty());
    }
}
