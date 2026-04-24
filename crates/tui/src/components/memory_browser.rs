//! Memory browser overlay — browse memory files with type-colored badges.
//!
//! Two views: `List` (type badge + name + description) and `Detail` (full body).
//! Triggered by `Action::OpenMemoryBrowser` (default: Ctrl+K Ctrl+M) which
//! loads the entries via a fresh `MemoryStore::scan()` on demand.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crab_memory::{MemoryFile, MemoryStore, MemoryType};

use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

// ---------------------------------------------------------------------------
// Data model (TUI-local, decoupled from crab-memory)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    User,
    Feedback,
    Project,
    Reference,
}

impl MemoryKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    #[must_use]
    pub fn color(self) -> Color {
        match self {
            Self::User => Color::Blue,
            Self::Feedback => Color::Yellow,
            Self::Project => Color::Green,
            Self::Reference => Color::Cyan,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub name: String,
    pub description: String,
    pub kind: MemoryKind,
    pub body: String,
}

impl From<MemoryType> for MemoryKind {
    fn from(t: MemoryType) -> Self {
        match t {
            MemoryType::User => Self::User,
            MemoryType::Feedback => Self::Feedback,
            MemoryType::Project => Self::Project,
            MemoryType::Reference => Self::Reference,
        }
    }
}

impl From<MemoryFile> for MemoryEntry {
    fn from(f: MemoryFile) -> Self {
        Self {
            name: f.metadata.name,
            description: f.metadata.description,
            kind: f.metadata.memory_type.into(),
            body: f.body,
        }
    }
}

/// Load all memories under `dir` into a list suitable for the browser.
/// Errors and empty directories both yield an empty vec (the overlay shows
/// a friendly empty state instead of crashing).
#[must_use]
pub fn load_memories(dir: &std::path::Path) -> Vec<MemoryEntry> {
    let store = MemoryStore::new(dir.to_path_buf());
    store
        .scan()
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect()
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    List,
    Detail,
}

// ---------------------------------------------------------------------------
// MemoryBrowserOverlay
// ---------------------------------------------------------------------------

pub struct MemoryBrowserOverlay {
    entries: Vec<MemoryEntry>,
    selected: usize,
    detail_scroll: usize,
    view: View,
}

impl MemoryBrowserOverlay {
    #[must_use]
    pub fn new(entries: Vec<MemoryEntry>) -> Self {
        Self {
            entries,
            selected: 0,
            detail_scroll: 0,
            view: View::List,
        }
    }

    fn current_entry(&self) -> Option<&MemoryEntry> {
        self.entries.get(self.selected)
    }

    fn enter_detail(&mut self) {
        if self.current_entry().is_some() {
            self.view = View::Detail;
            self.detail_scroll = 0;
        }
    }

    fn exit_detail(&mut self) {
        self.view = View::List;
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl Renderable for MemoryBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 20 {
            return;
        }

        Widget::render(Clear, area, buf);

        let title = match self.view {
            View::List => format!(" Memory ({}) ", self.entries.len()),
            View::Detail => {
                let name = self.current_entry().map_or("???", |e| e.name.as_str());
                format!(" {name} ")
            }
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        Widget::render(block, area, buf);

        match self.view {
            View::List => self.render_list(inner, buf),
            View::Detail => self.render_detail(inner, buf),
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0 // fullscreen
    }
}

impl MemoryBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        if self.entries.is_empty() {
            let msg = Line::from(Span::styled(
                "  No memory files found.",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(
                msg,
                Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
            return;
        }

        let visible = area.height.saturating_sub(1) as usize;

        for (i, entry) in self.entries.iter().enumerate() {
            if i >= visible {
                break;
            }
            let is_selected = i == self.selected;
            let prefix = if is_selected { "▸ " } else { "  " };
            let name_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let badge = format!("[{}]", entry.kind.label());
            let badge_style = Style::default()
                .fg(Color::Black)
                .bg(entry.kind.color())
                .add_modifier(Modifier::BOLD);

            let desc_width = (area.width as usize)
                .saturating_sub(prefix.len() + badge.len() + entry.name.len() + 6);
            let desc: String = entry.description.chars().take(desc_width).collect();

            let line = Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(badge, badge_style),
                Span::raw(" "),
                Span::styled(entry.name.clone(), name_style),
                Span::styled(" — ", Style::default().fg(Color::DarkGray)),
                Span::styled(desc, Style::default().fg(Color::Gray)),
            ]);
            Widget::render(
                line,
                Rect {
                    x: area.x,
                    y: area.y + i as u16,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }

        render_footer(area, buf, " ↑↓/jk navigate  Enter view  q/Esc close");
    }

    #[allow(clippy::cast_possible_truncation)]
    fn render_detail(&self, area: Rect, buf: &mut Buffer) {
        let Some(entry) = self.current_entry() else {
            return;
        };

        let mut lines: Vec<Line<'static>> = Vec::new();

        let badge = format!("[{}]", entry.kind.label());
        lines.push(Line::from(vec![
            Span::styled(
                badge,
                Style::default()
                    .fg(Color::Black)
                    .bg(entry.kind.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                entry.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(Line::from(Span::styled(
            entry.description.clone(),
            Style::default().fg(Color::Gray),
        )));

        lines.push(Line::default());

        for body_line in entry.body.lines() {
            lines.push(Line::from(Span::styled(
                body_line.to_string(),
                Style::default().fg(Color::White),
            )));
        }

        let visible = area.height.saturating_sub(1) as usize;
        let start = self.detail_scroll;

        for (i, line) in lines.iter().enumerate().skip(start) {
            let row = i - start;
            if row >= visible {
                break;
            }
            Widget::render(
                line.clone(),
                Rect {
                    x: area.x,
                    y: area.y + row as u16,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }

        render_footer(area, buf, " ↑↓/jk scroll  Esc back  q close");
    }
}

#[allow(clippy::cast_possible_truncation)]
fn render_footer(area: Rect, buf: &mut Buffer, text: &str) {
    if area.height > 1 {
        let hint = Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)));
        Widget::render(
            hint,
            Rect {
                x: area.x,
                y: area.y + area.height - 1,
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

impl Overlay for MemoryBrowserOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match self.view {
            View::List => self.handle_list_key(key),
            View::Detail => self.handle_detail_key(key),
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::ScrollBox]
    }

    fn name(&self) -> &'static str {
        "memory_browser"
    }
}

impl MemoryBrowserOverlay {
    fn handle_list_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.entries.is_empty() {
                    self.selected = (self.selected + 1).min(self.entries.len() - 1);
                }
                OverlayAction::Consumed
            }
            KeyCode::Enter => {
                self.enter_detail();
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                self.exit_detail();
                OverlayAction::Consumed
            }
            KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_entries() -> Vec<MemoryEntry> {
        vec![
            MemoryEntry {
                name: "user_role".into(),
                description: "User is a senior Rust developer".into(),
                kind: MemoryKind::User,
                body: "Prefers terse code reviews.\nDislikes verbose comments.".into(),
            },
            MemoryEntry {
                name: "no_todo_scaffolding".into(),
                description: "No todo!() placeholders".into(),
                kind: MemoryKind::Feedback,
                body: "Always provide real implementations.".into(),
            },
            MemoryEntry {
                name: "mcp_permission_naming".into(),
                description: "3 distinct permission features".into(),
                kind: MemoryKind::Reference,
                body: "core::permission / mcp::server_acl / bridge::permission_relay".into(),
            },
        ]
    }

    // --- View state ---

    #[test]
    fn starts_in_list() {
        let ov = MemoryBrowserOverlay::new(sample_entries());
        assert_eq!(ov.view, View::List);
    }

    #[test]
    fn enter_drills_into_detail() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::Detail);
    }

    #[test]
    fn esc_from_detail_returns_to_list() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Esc));
        assert_eq!(ov.view, View::List);
    }

    #[test]
    fn esc_from_list_dismisses() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        assert!(matches!(
            ov.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn q_always_dismisses() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        ov.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            ov.handle_key(key(KeyCode::Char('q'))),
            OverlayAction::Dismiss
        ));
    }

    // --- List navigation ---

    #[test]
    fn navigate_list() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected, 1);
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected, 2);
        ov.handle_key(key(KeyCode::Down)); // clamped
        assert_eq!(ov.selected, 2);
        ov.handle_key(key(KeyCode::Up));
        assert_eq!(ov.selected, 1);
    }

    // --- Detail scroll ---

    #[test]
    fn detail_scroll() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Char('j')));
        assert_eq!(ov.detail_scroll, 1);
        ov.handle_key(key(KeyCode::Char('k')));
        assert_eq!(ov.detail_scroll, 0);
    }

    // --- Empty ---

    #[test]
    fn empty_entries_no_panic() {
        let mut ov = MemoryBrowserOverlay::new(vec![]);
        ov.handle_key(key(KeyCode::Enter)); // stays in list
        assert_eq!(ov.view, View::List);
    }

    // --- Memory kind ---

    #[test]
    fn kind_labels() {
        assert_eq!(MemoryKind::User.label(), "user");
        assert_eq!(MemoryKind::Feedback.label(), "feedback");
        assert_eq!(MemoryKind::Project.label(), "project");
        assert_eq!(MemoryKind::Reference.label(), "reference");
    }

    #[test]
    fn kind_colors_distinct() {
        let colors = [
            MemoryKind::User.color(),
            MemoryKind::Feedback.color(),
            MemoryKind::Project.color(),
            MemoryKind::Reference.color(),
        ];
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(colors[i], colors[j]);
            }
        }
    }

    // --- Render ---

    #[test]
    fn render_list_no_panic() {
        let ov = MemoryBrowserOverlay::new(sample_entries());
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_detail_no_panic() {
        let mut ov = MemoryBrowserOverlay::new(sample_entries());
        ov.handle_key(key(KeyCode::Enter));
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_empty_no_panic() {
        let ov = MemoryBrowserOverlay::new(vec![]);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn overlay_name() {
        let ov = MemoryBrowserOverlay::new(vec![]);
        assert_eq!(ov.name(), "memory_browser");
    }

    #[test]
    fn overlay_context() {
        let ov = MemoryBrowserOverlay::new(vec![]);
        assert_eq!(ov.contexts(), vec![KeyContext::ScrollBox]);
    }
}
