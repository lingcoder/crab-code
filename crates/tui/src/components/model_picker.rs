//! Model picker overlay — select from available models (Alt+P).
//!
//! Supports fuzzy type-to-filter: start typing to narrow the visible
//! list. Keybindings mirror `history_search.rs` so users get a
//! consistent UX across all searchable overlays — vim-style Ctrl+P/K
//! and Ctrl+N/J move the selection, arrow keys work too, and the bare
//! `k`/`j` keys are now captured as query characters (consistent with
//! how history search treats them).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::components::fuzzy::FuzzyMatcher;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// Model picker overlay with fuzzy type-to-filter.
pub struct ModelPickerOverlay {
    /// All available model names (unfiltered).
    models: Vec<String>,
    /// Currently active model name (for the "(current)" marker).
    current: String,
    /// Current search query.
    query: String,
    /// Filtered view: `(index_into_models, model_name)`. Stored as
    /// `(usize, String)` pairs to match `history_search.rs`'s shape,
    /// which keeps `selected_model()` simple.
    results: Vec<(usize, String)>,
    /// Selected index *within `results`*, not within `models`.
    selected: usize,
    /// Reusable fuzzy matcher so nucleo's `Matcher` + scratch buffer
    /// aren't rebuilt on every keystroke.
    fuzzy: FuzzyMatcher,
}

impl ModelPickerOverlay {
    /// Create a new model picker.
    ///
    /// Initial state: empty query → all models visible, cursor parked
    /// on whichever entry matches `current` (falling back to index 0).
    pub fn new(models: Vec<String>, current: String) -> Self {
        let results: Vec<(usize, String)> = models
            .iter()
            .enumerate()
            .map(|(i, m)| (i, m.clone()))
            .collect();
        let selected = results.iter().position(|(_, m)| *m == current).unwrap_or(0);
        Self {
            models,
            current,
            query: String::new(),
            results,
            selected,
            fuzzy: FuzzyMatcher::new(),
        }
    }

    /// Get the selected model name.
    pub fn selected_model(&self) -> Option<&str> {
        self.results.get(self.selected).map(|(_, m)| m.as_str())
    }

    fn update_results(&mut self) {
        if self.query.is_empty() {
            self.results = self
                .models
                .iter()
                .enumerate()
                .map(|(i, m)| (i, m.clone()))
                .collect();
        } else {
            // Pair each model with its original index so we can
            // recover it after fuzzy ranking sorts by score.
            let indexed: Vec<(usize, String)> = self
                .models
                .iter()
                .enumerate()
                .map(|(i, m)| (i, m.clone()))
                .collect();
            let ranked = self
                .fuzzy
                .match_and_rank(&indexed, &self.query, |(_, s)| s.as_str());
            self.results = ranked
                .into_iter()
                .map(|((i, m), _score)| (*i, m.clone()))
                .collect();
        }
        self.selected = 0;
    }
}

impl Renderable for ModelPickerOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 20 {
            return;
        }

        // Center the popup. Height is now +1 to make room for the
        // "model> " query line under the title.
        let popup_width = 40.min(area.width.saturating_sub(4));
        let popup_height = (self.results.len() as u16 + 5).min(area.height.saturating_sub(4));
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        // Title
        Widget::render(
            Line::from(vec![
                Span::styled("╭─ ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    "Select Model",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", "─".repeat((popup_width as usize).saturating_sub(17))),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Rect {
                x: popup_x,
                y: popup_y,
                width: popup_width,
                height: 1,
            },
            buf,
        );

        // Query line — matches `history_search.rs`'s "history> " style.
        Widget::render(
            Line::from(vec![
                Span::styled(
                    "model> ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&self.query, Style::default().fg(Color::White)),
            ]),
            Rect {
                x: popup_x,
                y: popup_y + 1,
                width: popup_width,
                height: 1,
            },
            buf,
        );

        // Models — iterate the filtered `results`, not the full list.
        for (i, (_, model)) in self.results.iter().enumerate() {
            let y = popup_y + 2 + i as u16;
            if y >= popup_y + popup_height - 1 {
                break;
            }
            let is_selected = i == self.selected;
            let is_current = *model == self.current;
            let prefix = if is_selected { " ▸ " } else { "   " };
            let suffix = if is_current { " (current)" } else { "" };
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Widget::render(
                Line::from(Span::styled(format!("{prefix}{model}{suffix}"), style)),
                Rect {
                    x: popup_x,
                    y,
                    width: popup_width,
                    height: 1,
                },
                buf,
            );
        }

        // Footer
        Widget::render(
            Line::from(Span::styled(
                "  Type to filter, Enter to select, Esc to cancel",
                Style::default().fg(Color::DarkGray),
            )),
            Rect {
                x: popup_x,
                y: popup_y + popup_height - 1,
                width: popup_width,
                height: 1,
            },
            buf,
        );
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0 // overlay
    }
}

impl Overlay for ModelPickerOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc => OverlayAction::Dismiss,
            KeyCode::Enter => {
                if let Some(model) = self.selected_model() {
                    OverlayAction::Execute(crate::app_event::AppEvent::SwitchModel(
                        model.to_string(),
                    ))
                } else {
                    OverlayAction::Dismiss
                }
            }
            // Up / Ctrl+P / Ctrl+K — move selection up (toward top).
            // Note: the Ctrl variants MUST match before the bare
            // `Char(c)` catch-all below, or typing `k` while holding
            // Ctrl would be swallowed by the query input branch.
            KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::Char('p' | 'k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                OverlayAction::Consumed
            }
            // Down / Ctrl+N / Ctrl+J — move selection down.
            KeyCode::Down => {
                if self.selected + 1 < self.results.len() {
                    self.selected += 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::Char('n' | 'j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
        vec![KeyContext::ModelPicker]
    }

    fn name(&self) -> &'static str {
        "model_picker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_picker_basic() {
        let picker = ModelPickerOverlay::new(
            vec!["gpt-4o".into(), "claude-sonnet".into()],
            "gpt-4o".into(),
        );
        // `selected` is now an index into `results`, not `models`. With
        // an empty query, results mirrors models 1:1, so the index of
        // "gpt-4o" is still 0.
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.selected_model(), Some("gpt-4o"));
    }

    #[test]
    fn model_picker_navigation() {
        let mut picker =
            ModelPickerOverlay::new(vec!["a".into(), "b".into(), "c".into()], "a".into());
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        picker.handle_key(down);
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn model_picker_dismiss() {
        let mut picker = ModelPickerOverlay::new(vec![], String::new());
        let result = picker.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(result, OverlayAction::Dismiss));
    }

    #[test]
    fn model_picker_initial_shows_all_models() {
        // Empty query at construction time means every model should
        // be visible (no filtering) — the "just opened the picker"
        // baseline users expect.
        let picker = ModelPickerOverlay::new(
            vec![
                "claude-opus-4-6".into(),
                "claude-sonnet-4-6".into(),
                "claude-haiku-4-5".into(),
                "gpt-4o".into(),
                "deepseek-chat".into(),
            ],
            "claude-opus-4-6".into(),
        );
        assert_eq!(picker.results.len(), picker.models.len());
        assert_eq!(picker.results.len(), 5);
    }

    #[test]
    fn model_picker_fuzzy_ranks_by_match_quality() {
        // Feed a realistic multi-provider list and query "sonet"
        // (intentionally missing an 'n') — nucleo's fuzzy scoring
        // should still surface "claude-sonnet-4-6" ahead of the
        // unrelated models. We don't assert strict first-place (nucleo
        // tiebreaks can shift), only that the top hit contains "sonnet".
        let mut picker = ModelPickerOverlay::new(
            vec![
                "claude-opus-4-6".into(),
                "gpt-4o".into(),
                "claude-sonnet-4-6".into(),
                "deepseek-chat".into(),
            ],
            "claude-opus-4-6".into(),
        );
        picker.query = "sonet".into();
        picker.update_results();
        assert!(
            !picker.results.is_empty(),
            "expected at least one fuzzy hit for 'sonet'"
        );
        let top = &picker.results[0].1;
        assert!(
            top.contains("sonnet"),
            "top result should contain 'sonnet', got: {top}"
        );
    }

    #[test]
    fn model_picker_typing_updates_results() {
        // Simulate the user typing "so" one character at a time and
        // assert the results actually narrow. This exercises the
        // `KeyCode::Char(c)` branch end-to-end rather than poking
        // `query` directly, so we catch regressions in the input path.
        let mut picker = ModelPickerOverlay::new(
            vec![
                "claude-opus-4-6".into(),
                "gpt-4o".into(),
                "claude-sonnet-4-6".into(),
                "deepseek-chat".into(),
            ],
            "claude-opus-4-6".into(),
        );
        let initial = picker.results.len();
        picker.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(picker.query, "so");
        assert!(
            picker.results.len() < initial,
            "typing should narrow results: before={initial}, after={}",
            picker.results.len()
        );
        // "so" must still hit claude-sonnet and claude-opus (both have
        // the letters in order), so at least one result survives.
        assert!(!picker.results.is_empty());
    }

    #[test]
    fn model_picker_backspace_restores_results() {
        // After typing enough to narrow the list, backspacing every
        // character must fully restore the unfiltered view. This is the
        // "I typed wrong, start over" path and it's easy to break if
        // `update_results` forgets the empty-query branch.
        let mut picker = ModelPickerOverlay::new(
            vec![
                "claude-opus-4-6".into(),
                "gpt-4o".into(),
                "claude-sonnet-4-6".into(),
                "deepseek-chat".into(),
            ],
            "claude-opus-4-6".into(),
        );
        let full = picker.results.len();
        picker.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(picker.results.len() < full);
        // Backspace 3 times — should land back on the full list.
        picker.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(picker.query, "");
        assert_eq!(picker.results.len(), full);
    }

    #[test]
    fn model_picker_ctrl_nav_still_works() {
        // Ctrl+J / Ctrl+K must move the selection even though bare
        // `j` / `k` are now consumed by the query input branch. This
        // is the "vim users don't lose their muscle memory" guarantee.
        let mut picker =
            ModelPickerOverlay::new(vec!["a".into(), "b".into(), "c".into()], "a".into());
        picker.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(picker.selected, 1, "Ctrl+J should move selection down");
        picker.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(picker.selected, 0, "Ctrl+K should move selection back up");
    }
}
