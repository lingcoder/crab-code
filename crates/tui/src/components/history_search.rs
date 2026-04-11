//! History search overlay — fuzzy-match previous inputs (Ctrl+R).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crab_tools::str_utils::truncate_chars;

use crate::components::fuzzy::FuzzyMatcher;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// History search overlay — fuzzy-match previous user inputs.
pub struct HistorySearchOverlay {
    /// Current search query.
    query: String,
    /// Matched results: `(original_index, text)`.
    results: Vec<(usize, String)>,
    /// Currently selected result index.
    selected: usize,
    /// All input history (newest first).
    history: Vec<String>,
    /// Reusable fuzzy matcher — constructed once per overlay so nucleo's
    /// scratch buffer and `Matcher` aren't rebuilt on every keystroke.
    fuzzy: FuzzyMatcher,
}

impl HistorySearchOverlay {
    /// Create a new history search overlay.
    pub fn new(history: Vec<String>) -> Self {
        let results: Vec<(usize, String)> = history
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.clone()))
            .collect();
        Self {
            query: String::new(),
            results,
            selected: 0,
            history,
            fuzzy: FuzzyMatcher::new(),
        }
    }

    /// Get the selected result text.
    pub fn selected_text(&self) -> Option<&str> {
        self.results.get(self.selected).map(|(_, s)| s.as_str())
    }

    fn update_results(&mut self) {
        if self.query.is_empty() {
            self.results = self
                .history
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.clone()))
                .collect();
        } else {
            // Pair each history entry with its original index so we can
            // recover it after fuzzy ranking sorts by score.
            let indexed: Vec<(usize, String)> = self
                .history
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.clone()))
                .collect();
            let ranked = self
                .fuzzy
                .match_and_rank(&indexed, &self.query, |(_, s)| s.as_str());
            self.results = ranked
                .into_iter()
                .map(|((i, s), _score)| (*i, s.clone()))
                .collect();
        }
        self.selected = 0;
    }
}

impl Renderable for HistorySearchOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }

        // Render from bottom of area (like a dropdown above input)
        let max_results = (area.height as usize).saturating_sub(2).min(8);
        let result_count = self.results.len().min(max_results);

        let overlay_height = result_count as u16 + 2; // results + separator + query
        let overlay_y = area.y + area.height - overlay_height;

        // Query line
        let query_line = Line::from(vec![
            Span::styled(
                "history> ",
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
                y: overlay_y + overlay_height - 1,
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
                y: overlay_y + overlay_height - 2,
                width: area.width,
                height: 1,
            },
            buf,
        );

        // Results (newest first)
        for (i, (_, text)) in self.results.iter().take(max_results).enumerate() {
            let y = overlay_y + (result_count.saturating_sub(1) - i) as u16;
            let is_selected = i == self.selected;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let prefix = if is_selected { "▸ " } else { "  " };
            // Char-safe truncation: the old code byte-sliced into `text`
            // and panicked on multi-byte history entries wider than the
            // terminal. `truncate_chars` cuts on codepoint boundaries and
            // handles the no-op short-string case internally. `saturating_sub(5)`
            // reserves room for the 2-char prefix plus a small slack, matching
            // the prior layout intent without the `area.width < 5` underflow.
            let max = (area.width as usize).saturating_sub(5);
            let truncated = truncate_chars(text, max, "…");
            Widget::render(
                Line::from(Span::styled(format!("{prefix}{truncated}"), style)),
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
        10
    }
}

impl Overlay for HistorySearchOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc => OverlayAction::Dismiss,
            KeyCode::Enter => {
                if let Some(text) = self.selected_text() {
                    // Fill the input box with the selected history entry.
                    // Users can then edit or press Enter again to submit.
                    OverlayAction::Execute(crate::app_event::AppEvent::InsertInputText(
                        text.to_string(),
                    ))
                } else {
                    OverlayAction::Dismiss
                }
            }
            // Up / Ctrl+P / Ctrl+K — move selection up (toward newer results,
            // which are rendered at the top of the results list)
            KeyCode::Up => {
                if self.selected + 1 < self.results.len() {
                    self.selected += 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::Char('p' | 'k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.selected + 1 < self.results.len() {
                    self.selected += 1;
                }
                OverlayAction::Consumed
            }
            // Down / Ctrl+N / Ctrl+J — move selection down
            KeyCode::Down => {
                self.selected = self.selected.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Char('n' | 'j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected = self.selected.saturating_sub(1);
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
        vec![KeyContext::HistorySearch]
    }

    fn name(&self) -> &'static str {
        "history_search"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_search_basic() {
        let overlay = HistorySearchOverlay::new(vec![
            "hello world".into(),
            "foo bar".into(),
            "hello there".into(),
        ]);
        assert_eq!(overlay.results.len(), 3);
        assert_eq!(overlay.selected_text(), Some("hello world"));
    }

    #[test]
    fn history_search_filter() {
        let mut overlay = HistorySearchOverlay::new(vec![
            "hello world".into(),
            "foo bar".into(),
            "hello there".into(),
        ]);
        overlay.query = "hello".into();
        overlay.update_results();
        assert_eq!(overlay.results.len(), 2);
    }

    #[test]
    fn history_search_dismiss() {
        let mut overlay = HistorySearchOverlay::new(vec![]);
        let result = overlay.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(result, OverlayAction::Dismiss));
    }

    #[test]
    fn history_search_render_does_not_panic_on_multibyte_truncation() {
        // Regression test for the byte-slice bug at the previous
        // truncation site: a CJK history entry wider than the overlay
        // area forces the truncation path, and the byte-slice would
        // panic if the cut point lands mid-codepoint. `truncate_chars`
        // must handle this on codepoint boundaries.
        //
        // We also throw an emoji + accented string into the mix so the
        // test covers the grapheme classes most likely to break byte
        // slicing: 3-byte CJK, 4-byte emoji, and 2-byte Latin-1
        // supplement.
        let overlay = HistorySearchOverlay::new(vec![
            "这是一个很长的中文历史记录条目需要被截断".into(),
            "🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀 rust".into(),
            "naïve café résumé — a long accented entry".into(),
        ]);
        // Narrow width forces the truncation branch for every entry.
        let area = Rect::new(0, 0, 10, 8);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);
    }
}
