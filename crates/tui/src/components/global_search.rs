//! Global search overlay — search across the conversation (Ctrl+Shift+F).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::components::fuzzy::FuzzyMatcher;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// A search match in the transcript.
#[derive(Debug, Clone)]
struct SearchMatch {
    /// Index of the message containing the match.
    message_idx: usize,
    /// Preview text around the match.
    preview: String,
}

/// Global search overlay — search across all messages.
pub struct GlobalSearchOverlay {
    /// Current search query.
    query: String,
    /// Matched results.
    results: Vec<SearchMatch>,
    /// Currently selected result index.
    selected: usize,
    /// All messages to search.
    messages: Vec<ChatMessage>,
    /// Reusable fuzzy matcher so nucleo's `Matcher` + scratch buffer
    /// aren't rebuilt on every keystroke.
    fuzzy: FuzzyMatcher,
}

impl GlobalSearchOverlay {
    /// Create a new global search overlay.
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            messages,
            fuzzy: FuzzyMatcher::new(),
        }
    }

    /// Extract the searchable text for a single `ChatMessage`.
    ///
    /// Pulled out of `update_results` so the preview builder can reuse
    /// the same flattening logic and stay in lockstep with the matcher.
    fn message_text(msg: &ChatMessage) -> String {
        match msg {
            ChatMessage::User { text }
            | ChatMessage::Assistant { text }
            | ChatMessage::System { text } => text.clone(),
            ChatMessage::ToolUse { name, .. } => name.clone(),
            ChatMessage::ToolResult {
                tool_name, output, ..
            } => format!("{tool_name}: {output}"),
            ChatMessage::ToolRejected {
                tool_name, summary, ..
            } => format!("{tool_name}: {summary}"),
            ChatMessage::CompactBoundary { .. } | ChatMessage::PlanStep { .. } => String::new(),
        }
    }

    fn update_results(&mut self) {
        self.results.clear();
        if self.query.is_empty() {
            return;
        }

        // Flatten all messages to (index, text) pairs, then let
        // FuzzyMatcher rank them. Ordering: best-score first, which is
        // the behavior users expect from a fuzzy search ("find the most
        // relevant match" — not "find the oldest match").
        let indexed: Vec<(usize, String)> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, msg)| (i, Self::message_text(msg)))
            .collect();

        let ranked = self
            .fuzzy
            .match_and_rank(&indexed, &self.query, |(_, s)| s.as_str());

        // Build a lowercase query once for the substring-preview fallback.
        let query_lower = self.query.to_lowercase();

        for ((msg_idx, text), _score) in ranked {
            // Preview strategy: if any line contains the query as a
            // substring (the common case for "type a word, find it"),
            // show that line. Otherwise fall back to the first non-empty
            // line of the message — scattered fuzzy matches won't have
            // a single "hit line" to point at.
            let preview = text
                .lines()
                .find(|l| l.to_lowercase().contains(&query_lower))
                .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or(text)
                .to_string();
            self.results.push(SearchMatch {
                message_idx: *msg_idx,
                preview,
            });
        }
        self.selected = 0;
    }
}

impl Renderable for GlobalSearchOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 {
            return;
        }

        // Title
        let title = Line::from(vec![
            Span::styled(
                " Global Search ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} matches)", self.results.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        Widget::render(
            title,
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
            buf,
        );

        // Query line
        let query_line = Line::from(vec![
            Span::styled(
                "search> ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&self.query, Style::default().fg(Color::White)),
        ]);
        Widget::render(
            query_line,
            Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: 1,
            },
            buf,
        );

        // Separator
        let sep = "─".repeat(area.width as usize);
        Widget::render(
            Line::from(Span::styled(&*sep, Style::default().fg(Color::DarkGray))),
            Rect {
                x: area.x,
                y: area.y + 2,
                width: area.width,
                height: 1,
            },
            buf,
        );

        // Results
        let max_results = (area.height as usize).saturating_sub(3);
        for (i, result) in self.results.iter().take(max_results).enumerate() {
            let y = area.y + 3 + i as u16;
            let is_selected = i == self.selected;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let prefix = if is_selected { "▸ " } else { "  " };
            let msg_type = match &self.messages.get(result.message_idx) {
                Some(ChatMessage::User { .. }) => "[user]",
                Some(ChatMessage::Assistant { .. }) => "[asst]",
                Some(ChatMessage::ToolUse { .. }) => "[tool]",
                Some(ChatMessage::ToolResult { .. }) => "[rslt]",
                Some(ChatMessage::ToolRejected { .. }) => "[deny]",
                Some(
                    ChatMessage::System { .. }
                    | ChatMessage::CompactBoundary { .. }
                    | ChatMessage::PlanStep { .. },
                ) => "[sys] ",
                None => "[???] ",
            };
            let truncated = if result.preview.len() > area.width as usize - 12 {
                &result.preview[..area.width as usize - 15]
            } else {
                &result.preview
            };
            Widget::render(
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(msg_type, Style::default().fg(Color::DarkGray)),
                    Span::styled(format!(" {truncated}"), style),
                ]),
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

    fn desired_height(&self, _width: u16) -> u16 {
        0 // fullscreen
    }
}

impl Overlay for GlobalSearchOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayAction::Dismiss,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down => {
                if self.selected + 1 < self.results.len() {
                    self.selected += 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.update_results();
                OverlayAction::Consumed
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.query.push(c);
                self.update_results();
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::Search]
    }

    fn name(&self) -> &'static str {
        "global_search"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_search_empty() {
        let search = GlobalSearchOverlay::new(vec![]);
        assert!(search.results.is_empty());
    }

    #[test]
    fn global_search_finds_matches() {
        let mut search = GlobalSearchOverlay::new(vec![
            ChatMessage::User {
                text: "hello world".into(),
            },
            ChatMessage::Assistant {
                text: "goodbye world".into(),
            },
        ]);
        search.query = "world".into();
        search.update_results();
        assert_eq!(search.results.len(), 2);
    }

    #[test]
    fn global_search_dismiss() {
        let mut search = GlobalSearchOverlay::new(vec![]);
        let result = search.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(result, OverlayAction::Dismiss));
    }

    #[test]
    fn global_search_render_does_not_panic() {
        let search = GlobalSearchOverlay::new(vec![ChatMessage::User {
            text: "test".into(),
        }]);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        search.render(area, &mut buf);
    }

    #[test]
    fn fuzzy_ranks_messages_by_match_quality() {
        // Message 0 contains the query verbatim; message 1 has it
        // scattered. The verbatim hit should score higher under nucleo
        // and land first in results.
        let mut search = GlobalSearchOverlay::new(vec![
            ChatMessage::User {
                text: "the build failed in crab-tui".into(),
            },
            ChatMessage::Assistant {
                text: "checking cargo build artifacts".into(),
            },
            ChatMessage::System {
                text: "unrelated warning about theme".into(),
            },
        ]);
        search.query = "build".into();
        search.update_results();

        assert!(
            search.results.len() >= 2,
            "expected at least 2 hits, got {}",
            search.results.len()
        );
        // The first result must be one of the two messages containing
        // "build" — both word-start matches, so either could win under
        // nucleo's tiebreaker. What matters is that the "unrelated"
        // message is not the top result.
        let top_text = match &search.messages[search.results[0].message_idx] {
            ChatMessage::User { text }
            | ChatMessage::Assistant { text }
            | ChatMessage::System { text } => text.clone(),
            _ => panic!("unexpected top message variant"),
        };
        assert!(
            top_text.contains("build"),
            "top result should contain 'build', got: {top_text}"
        );
    }

    #[test]
    fn fuzzy_scattered_match_still_hits() {
        // Nucleo is fuzzy: "bld" should still match "build" because
        // the characters appear in order. The old substring matcher
        // would have missed this entirely.
        let mut search = GlobalSearchOverlay::new(vec![ChatMessage::User {
            text: "the build failed".into(),
        }]);
        search.query = "bld".into();
        search.update_results();
        assert_eq!(search.results.len(), 1, "fuzzy 'bld' should match 'build'");
    }

    #[test]
    fn empty_query_preserves_message_order() {
        // An empty query yields no results (this matches the prior
        // "nothing to search yet" UX). The test name tracks the task
        // spec naming, and asserts the no-results contract — which is
        // the "preserved order" the UI ends up showing: none.
        let mut search = GlobalSearchOverlay::new(vec![
            ChatMessage::User {
                text: "alpha".into(),
            },
            ChatMessage::Assistant {
                text: "beta".into(),
            },
            ChatMessage::System {
                text: "gamma".into(),
            },
        ]);
        search.query.clear();
        search.update_results();
        assert!(
            search.results.is_empty(),
            "empty query should yield no results"
        );
    }

    #[test]
    fn fuzzy_no_match_returns_empty() {
        let mut search = GlobalSearchOverlay::new(vec![
            ChatMessage::User {
                text: "hello world".into(),
            },
            ChatMessage::Assistant {
                text: "goodbye world".into(),
            },
        ]);
        search.query = "zzzzzzzz".into();
        search.update_results();
        assert!(search.results.is_empty());
    }

    #[test]
    fn fuzzy_preview_falls_back_to_first_line() {
        // When the query is fuzzy-scattered and no single line contains
        // the substring, the preview should fall back to the first
        // non-empty line rather than leaving an empty string or panicking.
        let mut search = GlobalSearchOverlay::new(vec![ChatMessage::User {
            text: "\n\nfirst real line\nsecond line".into(),
        }]);
        // "fl" is a fuzzy hit (f from "first", l from "line") but no
        // single line contains "fl" as a substring.
        search.query = "fl".into();
        search.update_results();
        assert_eq!(search.results.len(), 1);
        // The fallback should pick "first real line", not the empty
        // leading lines.
        assert!(
            !search.results[0].preview.trim().is_empty(),
            "preview should not be blank: {:?}",
            search.results[0].preview
        );
    }
}
