//! Bottom bar component — contextual shortcut hints.
//!
//! When a chord prefix is in flight (e.g. `Ctrl+K` pressed, waiting for
//! the second key), the chord hint takes precedence over the normal
//! state-specific hint so the user sees what the resolver is waiting
//! for.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{AppState, ExitKey};
use crate::keybindings::KeyChord;
use crate::traits::Renderable;

/// Bottom bar showing contextual key hints + right-aligned status line.
pub struct BottomBar<'a> {
    pub state: AppState,
    pub search_active: bool,
    pub permission_mode: crab_core::permission::PermissionMode,
    /// In-flight chord prefix. When present, rendered as
    /// `"Ctrl+K …"` to tell the user another key is expected.
    pub chord_prefix: Option<&'a [KeyChord]>,
    /// Vim mode label (e.g. "NORMAL", "INSERT") when vim is active.
    pub vim_mode: Option<&'a str>,
    /// When `Some`, show `"Press <keyName> again to exit"` instead of the
    /// normal state hint. The key identifies which of Ctrl-C or Ctrl-D
    /// started the double-press window.
    pub exit_pending: Option<ExitKey>,
    /// Short model name shown in the status line (e.g. "Opus 4.6").
    pub model_name: Option<&'a str>,
    /// Context window usage as percentage 0-100.
    pub context_used_pct: u8,
    /// Total context window size in tokens.
    pub context_window_size: u64,
    /// Tokens already consumed (input + output combined).
    pub used_tokens: u64,
    /// Cumulative cost in USD.
    pub total_cost_usd: f64,
    /// Cumulative token usage for the right-aligned stats.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

impl Renderable for BottomBar<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(prefix) = self.chord_prefix {
            render_chord_hint(prefix, area, buf);
            return;
        }

        if let Some(key) = self.exit_pending {
            let hint = format!("  Press {} again to exit", key.display_name());
            let line = Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)));
            Widget::render(line, area, buf);
            return;
        }

        // Right-aligned status line (model | context | cost | tokens)
        let stats = format_status_right(
            self.model_name,
            self.context_used_pct,
            self.context_window_size,
            self.used_tokens,
            self.total_cost_usd,
            self.total_input_tokens,
            self.total_output_tokens,
        );
        let stats_width = stats.chars().count() as u16;
        let left_budget = area.width.saturating_sub(stats_width + 1);

        let left_area = Rect {
            width: left_budget,
            ..area
        };

        if let Some(vim_label) = self.vim_mode {
            let (_, rest_area) = render_vim_badge(vim_label, left_area, buf);
            if rest_area.width > 0 {
                render_bottom_bar(
                    self.state,
                    self.search_active,
                    self.permission_mode,
                    rest_area,
                    buf,
                );
            }
        } else {
            render_bottom_bar(
                self.state,
                self.search_active,
                self.permission_mode,
                left_area,
                buf,
            );
        }

        // Render token stats on the right
        if !stats.is_empty() && stats_width < area.width {
            let stats_area = Rect {
                x: area.x + area.width - stats_width,
                y: area.y,
                width: stats_width,
                height: 1,
            };
            let stats_line = Line::from(Span::styled(stats, Style::default().fg(Color::DarkGray)));
            Widget::render(stats_line, stats_area, buf);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

fn render_vim_badge(label: &str, area: Rect, buf: &mut Buffer) -> (Style, Rect) {
    let badge = format!(" [{label}] ");
    let badge_width = badge.len() as u16;
    let style = Style::default()
        .fg(Color::Black)
        .bg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let badge_area = Rect {
        x: area.x,
        y: area.y,
        width: badge_width.min(area.width),
        height: 1,
    };
    Widget::render(Span::styled(badge, style), badge_area, buf);
    let rest = Rect {
        x: area.x + badge_area.width,
        y: area.y,
        width: area.width.saturating_sub(badge_area.width),
        height: 1,
    };
    (style, rest)
}

fn render_chord_hint(prefix: &[KeyChord], area: Rect, buf: &mut Buffer) {
    let prefix_text = format_chord_prefix(prefix);
    let line = Line::from(vec![
        Span::styled(
            format!("  {prefix_text} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "… (waiting for next key)",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    Widget::render(line, area, buf);
}

/// Render a chord sequence like `[Ctrl+K]` as the hint string `Ctrl+K`.
/// Multiple chords are separated by spaces: `Ctrl+K Ctrl+S`.
fn format_chord_prefix(prefix: &[KeyChord]) -> String {
    prefix
        .iter()
        .map(format_chord)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_chord(chord: &KeyChord) -> String {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mut parts: Vec<&str> = Vec::new();
    if chord.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if chord.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if chord.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }
    let key = match chord.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "BS".to_string(),
        KeyCode::Delete => "Del".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    parts.push(key.as_str());
    parts.join("+")
}

fn format_tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_cost(usd: f64) -> Option<String> {
    if usd <= 0.0 {
        None
    } else {
        Some(format!("${usd:.2}"))
    }
}

fn format_status_right(
    model: Option<&str>,
    ctx_pct: u8,
    ctx_window: u64,
    used_tokens: u64,
    cost: f64,
    input: u64,
    output: u64,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(name) = model
        && !name.is_empty()
    {
        parts.push(name.to_string());
    }

    if ctx_window > 0 {
        parts.push(format!(
            "Context {}% ({}/{})",
            ctx_pct,
            format_tokens_short(used_tokens),
            format_tokens_short(ctx_window),
        ));
    }

    if let Some(cost_str) = format_cost(cost) {
        parts.push(cost_str);
    }

    if input > 0 || output > 0 {
        parts.push(format!(
            "{}↑ {}↓",
            format_tokens_short(input),
            format_tokens_short(output),
        ));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("{} ", parts.join(" \u{2502} "))
    }
}

fn render_bottom_bar(
    state: AppState,
    search_active: bool,
    perm_mode: crab_core::permission::PermissionMode,
    area: Rect,
    buf: &mut Buffer,
) {
    let line = if search_active {
        Line::from(Span::styled(
            "Enter: next | Shift+Enter: prev | Esc: close | type to search",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        match state {
            AppState::Confirming => Line::from(Span::styled(
                "y: allow | n: deny | a: always | Esc: deny",
                Style::default().fg(Color::DarkGray),
            )),
            AppState::Processing => Line::from(vec![
                Span::styled("  ▶▶ ", Style::default().fg(Color::DarkGray)),
                Span::styled(perm_mode.to_string(), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " (shift+tab to cycle) · esc to interrupt",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            _ => {
                if perm_mode == crab_core::permission::PermissionMode::Default {
                    Line::from(Span::styled(
                        "  ? for shortcuts",
                        Style::default().fg(Color::DarkGray),
                    ))
                } else {
                    Line::from(vec![
                        Span::styled("  ▶▶ ", Style::default().fg(Color::DarkGray)),
                        Span::styled(perm_mode.to_string(), Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            " (shift+tab to cycle) · ? for shortcuts",
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])
                }
            }
        }
    };
    Widget::render(line, area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn bottom_bar_desired_height() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: None,
            model_name: None,
            context_used_pct: 0,
            context_window_size: 0,
            used_tokens: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        assert_eq!(bb.desired_height(80), 1);
    }

    #[test]
    fn bottom_bar_render_does_not_panic() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: None,
            model_name: None,
            context_used_pct: 0,
            context_window_size: 0,
            used_tokens: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        bb.render(area, &mut buf);
    }

    #[test]
    fn format_single_ctrl_chord() {
        let chord = KeyChord::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(format_chord(&chord), "Ctrl+K");
    }

    #[test]
    fn format_multi_chord_prefix() {
        let prefix = [
            KeyChord::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            KeyChord::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        ];
        assert_eq!(format_chord_prefix(&prefix), "Ctrl+K Ctrl+S");
    }

    #[test]
    fn format_alt_shift_chord() {
        let chord = KeyChord::new(KeyCode::Char('p'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(format_chord(&chord), "Alt+Shift+P");
    }

    #[test]
    fn format_named_key() {
        let chord = KeyChord::new(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(format_chord(&chord), "PgUp");
    }

    fn rendered_line(bb: &BottomBar<'_>) -> String {
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        bb.render(area, &mut buf);
        (0..area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn exit_pending_ctrl_c_shows_ctrl_c_hint() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: Some(ExitKey::CtrlC),
            model_name: None,
            context_used_pct: 0,
            context_window_size: 0,
            used_tokens: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        assert!(rendered_line(&bb).contains("Press Ctrl-C again to exit"));
    }

    #[test]
    fn exit_pending_ctrl_d_shows_ctrl_d_hint() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: Some(ExitKey::CtrlD),
            model_name: None,
            context_used_pct: 0,
            context_window_size: 0,
            used_tokens: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        assert!(rendered_line(&bb).contains("Press Ctrl-D again to exit"));
    }

    #[test]
    fn non_default_idle_includes_shortcuts_hint() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::AcceptEdits,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: None,
            model_name: None,
            context_used_pct: 0,
            context_window_size: 0,
            used_tokens: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        let line = rendered_line(&bb);
        assert!(
            line.contains("? for shortcuts"),
            "non-Default idle must show ? for shortcuts; got: {line:?}"
        );
        assert!(
            line.contains("shift+tab to cycle"),
            "non-Default idle must show shift+tab hint; got: {line:?}"
        );
    }

    #[test]
    fn chord_hint_takes_precedence_over_state_hint() {
        let prefix = [KeyChord::new(KeyCode::Char('k'), KeyModifiers::CONTROL)];
        let bb = BottomBar {
            state: AppState::Processing, // would normally show the processing hint
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: Some(&prefix),
            vim_mode: None,
            exit_pending: None,
            model_name: None,
            context_used_pct: 0,
            context_window_size: 0,
            used_tokens: 0,
            total_cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        bb.render(area, &mut buf);

        // Confirm the chord prefix text was written into the buffer.
        let rendered: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("Ctrl+K"));
    }
}
