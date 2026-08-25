//! Agent/Team browser overlay — browse team members and tasks.
//!
//! Two tabs: `Members` and `Tasks`, switchable with `Tab`.
//!
//! Triggered by `Action::OpenTeamBrowser` (default: Ctrl+K Ctrl+E). Rows come
//! straight from the runtime's roster snapshot and the shared work queue, so
//! this overlay defines no member or task model of its own.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

// Data model
//
// Members are the runtime's roster rows and tasks are the shared work-queue
// items; this overlay only decides how to draw them.

pub use crab_agents::TeamMemberSnapshot as MemberInfo;
pub use crab_core::task::{Task as TaskInfo, TaskStatus};

/// Glyph for a work-queue status.
#[must_use]
fn task_glyph(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "[ ]",
        TaskStatus::InProgress => "[~]",
        TaskStatus::Completed => "[x]",
        TaskStatus::Deleted => "[-]",
    }
}

/// Colour for a teammate's lifecycle state, matching `TeammateState`'s
/// `Display` output.
#[must_use]
fn state_color(state: &str) -> Color {
    match state {
        "running" => Color::Cyan,
        "done" => Color::Green,
        "failed" => Color::Red,
        "stopped" => Color::DarkGray,
        _ => Color::Yellow,
    }
}

/// Colour for a work-queue status.
#[must_use]
fn task_color(status: TaskStatus) -> Color {
    match status {
        TaskStatus::Pending => Color::DarkGray,
        TaskStatus::InProgress => Color::Yellow,
        TaskStatus::Completed => Color::Green,
        TaskStatus::Deleted => Color::Red,
    }
}

#[derive(Debug, Clone, Default)]
pub struct TeamSnapshot {
    pub members: Vec<MemberInfo>,
    pub tasks: Vec<TaskInfo>,
}

// Tab state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Members,
    Tasks,
}

impl Tab {
    fn toggle(self) -> Self {
        match self {
            Self::Members => Self::Tasks,
            Self::Tasks => Self::Members,
        }
    }
}

// TeamBrowserOverlay

pub struct TeamBrowserOverlay {
    snapshot: TeamSnapshot,
    tab: Tab,
    selected: usize,
}

impl TeamBrowserOverlay {
    #[must_use]
    pub fn new(snapshot: TeamSnapshot) -> Self {
        Self {
            snapshot,
            tab: Tab::Members,
            selected: 0,
        }
    }

    fn current_list_len(&self) -> usize {
        match self.tab {
            Tab::Members => self.snapshot.members.len(),
            Tab::Tasks => self.snapshot.tasks.len(),
        }
    }
}

// Rendering

impl Renderable for TeamBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 20 {
            return;
        }

        Widget::render(Clear, area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Team Browser ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        Widget::render(block, area, buf);

        if inner.height < 3 {
            return;
        }

        // Tab bar
        let members_style = if self.tab == Tab::Members {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let tasks_style = if self.tab == Tab::Tasks {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let tab_line = Line::from(vec![
            Span::styled("  Members", members_style),
            Span::styled("  |  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Tasks", tasks_style),
            Span::styled(
                format!("  (Tab to switch, {} items)", self.current_list_len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        Widget::render(
            tab_line,
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            },
            buf,
        );

        let sep = "\u{2500}".repeat(inner.width as usize);
        Widget::render(
            Line::from(Span::styled(&*sep, Style::default().fg(Color::DarkGray))),
            Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            },
            buf,
        );

        let content_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height.saturating_sub(3),
        };

        match self.tab {
            Tab::Members => self.render_members(content_area, buf),
            Tab::Tasks => self.render_tasks(content_area, buf),
        }

        // Footer
        if inner.height > 2 {
            let hint = Line::from(Span::styled(
                " Tab switch  ↑↓/jk navigate  q/Esc close",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(
                hint,
                Rect {
                    x: inner.x,
                    y: inner.y + inner.height - 1,
                    width: inner.width,
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

impl TeamBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render_members(&self, area: Rect, buf: &mut Buffer) {
        if self.snapshot.members.is_empty() {
            let msg = Line::from(Span::styled(
                "  No team members.",
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

        for (i, member) in self.snapshot.members.iter().enumerate() {
            if i as u16 >= area.height {
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

            let mut spans = vec![
                Span::styled(prefix, name_style),
                Span::styled(member.name.clone(), name_style),
            ];

            if member.is_leader {
                spans.push(Span::styled(
                    " [leader]",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            spans.push(Span::styled(
                format!("  {}", member.state),
                Style::default().fg(state_color(&member.state)),
            ));

            let mut parts = vec![member.lifetime.clone()];
            if !member.role.is_empty() {
                parts.push(member.role.clone());
            }
            if let Some(model) = &member.model {
                parts.push(model.clone());
            }
            let detail = format!("  ({})", parts.join(", "));
            spans.push(Span::styled(detail, Style::default().fg(Color::DarkGray)));

            Widget::render(
                Line::from(spans),
                Rect {
                    x: area.x,
                    y: area.y + i as u16,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn render_tasks(&self, area: Rect, buf: &mut Buffer) {
        if self.snapshot.tasks.is_empty() {
            let msg = Line::from(Span::styled(
                "  No tasks.",
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

        for (i, task) in self.snapshot.tasks.iter().enumerate() {
            if i as u16 >= area.height {
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

            let glyph = task_glyph(task.status);
            let glyph_color = task_color(task.status);

            let mut spans = vec![
                Span::styled(prefix, name_style),
                Span::styled(format!("{glyph} "), Style::default().fg(glyph_color)),
                Span::styled(task.subject.clone(), name_style),
            ];

            if let Some(owner) = &task.owner {
                spans.push(Span::styled(
                    format!("  @{owner}"),
                    Style::default().fg(Color::Magenta),
                ));
            }

            if !task.blocked_by.is_empty() {
                spans.push(Span::styled(
                    format!("  blocked by: {}", task.blocked_by.join(", ")),
                    Style::default().fg(Color::Red),
                ));
            }

            Widget::render(
                Line::from(spans),
                Rect {
                    x: area.x,
                    y: area.y + i as u16,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }
}

// Input handling

impl Overlay for TeamBrowserOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Tab => {
                self.tab = self.tab.toggle();
                self.selected = 0;
                OverlayAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = self.current_list_len();
                if len > 0 {
                    self.selected = (self.selected + 1).min(len - 1);
                }
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::AgentDetail]
    }

    fn name(&self) -> &'static str {
        "team_browser"
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn member(name: &str, model: &str, leader: bool, caps: &[&str]) -> MemberInfo {
        MemberInfo {
            name: name.into(),
            role: "reviewer".into(),
            model: Some(model.into()),
            state: "running".into(),
            lifetime: "resident".into(),
            is_leader: leader,
            capabilities: caps.iter().map(|c| (*c).to_string()).collect(),
        }
    }

    fn task(id: &str, subject: &str, status: TaskStatus, owner: Option<&str>) -> TaskInfo {
        TaskInfo {
            id: id.into(),
            subject: subject.into(),
            description: String::new(),
            status,
            owner: owner.map(str::to_owned),
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        }
    }

    fn sample_snapshot() -> TeamSnapshot {
        let mut blocked = task("3", "Write tests", TaskStatus::Pending, None);
        blocked.blocked_by = vec!["2".into()];
        TeamSnapshot {
            members: vec![
                member("team-lead", "opus", true, &["all"]),
                member("researcher", "sonnet", false, &["read", "search"]),
            ],
            tasks: vec![
                task("1", "Design API", TaskStatus::Completed, Some("team-lead")),
                task(
                    "2",
                    "Implement endpoints",
                    TaskStatus::InProgress,
                    Some("researcher"),
                ),
                blocked,
            ],
        }
    }

    // --- Tab switching ---

    #[test]
    fn starts_on_members_tab() {
        let ov = TeamBrowserOverlay::new(sample_snapshot());
        assert_eq!(ov.tab, Tab::Members);
    }

    #[test]
    fn tab_switches_to_tasks() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        ov.handle_key(key(KeyCode::Tab));
        assert_eq!(ov.tab, Tab::Tasks);
    }

    #[test]
    fn tab_switches_back() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        ov.handle_key(key(KeyCode::Tab));
        ov.handle_key(key(KeyCode::Tab));
        assert_eq!(ov.tab, Tab::Members);
    }

    #[test]
    fn tab_resets_selection() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected, 1);
        ov.handle_key(key(KeyCode::Tab));
        assert_eq!(ov.selected, 0);
    }

    // --- Navigation ---

    #[test]
    fn navigate_members() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected, 1);
        ov.handle_key(key(KeyCode::Down)); // clamped at 1 (2 members)
        assert_eq!(ov.selected, 1);
        ov.handle_key(key(KeyCode::Up));
        assert_eq!(ov.selected, 0);
    }

    #[test]
    fn navigate_tasks() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        ov.handle_key(key(KeyCode::Tab)); // tasks tab
        ov.handle_key(key(KeyCode::Down));
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected, 2);
        ov.handle_key(key(KeyCode::Down)); // clamped at 2 (3 tasks)
        assert_eq!(ov.selected, 2);
    }

    // --- Dismiss ---

    #[test]
    fn esc_dismisses() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        assert!(matches!(
            ov.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn q_dismisses() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        assert!(matches!(
            ov.handle_key(key(KeyCode::Char('q'))),
            OverlayAction::Dismiss
        ));
    }

    // --- Status and state rendering ---

    #[test]
    fn task_status_glyphs() {
        assert_eq!(task_glyph(TaskStatus::Pending), "[ ]");
        assert_eq!(task_glyph(TaskStatus::InProgress), "[~]");
        assert_eq!(task_glyph(TaskStatus::Completed), "[x]");
        assert_eq!(task_glyph(TaskStatus::Deleted), "[-]");
    }

    #[test]
    fn task_status_colors_distinct() {
        assert_ne!(
            task_color(TaskStatus::Pending),
            task_color(TaskStatus::InProgress)
        );
        assert_ne!(
            task_color(TaskStatus::InProgress),
            task_color(TaskStatus::Completed)
        );
    }

    #[test]
    fn teammate_state_colors_distinct() {
        // The strings match `TeammateState`'s Display output.
        assert_ne!(state_color("running"), state_color("done"));
        assert_ne!(state_color("done"), state_color("failed"));
        assert_eq!(state_color("idle"), state_color("something-new"));
    }

    // --- Empty ---

    #[test]
    fn empty_snapshot_no_panic() {
        let mut ov = TeamBrowserOverlay::new(TeamSnapshot {
            members: vec![],
            tasks: vec![],
        });
        ov.handle_key(key(KeyCode::Down)); // should not panic
        assert_eq!(ov.selected, 0);
    }

    // --- Render ---

    #[test]
    fn render_members_no_panic() {
        let ov = TeamBrowserOverlay::new(sample_snapshot());
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_tasks_no_panic() {
        let mut ov = TeamBrowserOverlay::new(sample_snapshot());
        ov.handle_key(key(KeyCode::Tab));
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_empty_no_panic() {
        let ov = TeamBrowserOverlay::new(TeamSnapshot {
            members: vec![],
            tasks: vec![],
        });
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_tiny_area_no_panic() {
        let ov = TeamBrowserOverlay::new(sample_snapshot());
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn overlay_name() {
        let ov = TeamBrowserOverlay::new(TeamSnapshot {
            members: vec![],
            tasks: vec![],
        });
        assert_eq!(ov.name(), "team_browser");
    }

    #[test]
    fn overlay_context() {
        let ov = TeamBrowserOverlay::new(TeamSnapshot {
            members: vec![],
            tasks: vec![],
        });
        assert_eq!(ov.contexts(), vec![KeyContext::AgentDetail]);
    }
}
