//! Project trust dialog overlay — confirms user trusts project-level settings.

use std::fmt::Write as _;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Widget};

use crate::app_event::AppEvent;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// Enumerated threats the user is being asked to trust.
///
/// Built from the project's settings + filesystem so the dialog can show
/// specific risks (named MCP servers, hook counts, env var names) instead
/// of a generic "contains settings" boolean.
#[derive(Debug, Clone, Default)]
pub struct TrustContext {
    /// Names of MCP servers configured for this project.
    pub mcp_servers: Vec<String>,
    /// Number of hook definitions active for this project.
    pub hook_count: usize,
    /// Custom environment variable names set in project settings.
    pub env_vars: Vec<String>,
    /// Whether a project-level `settings.json` file exists.
    pub has_settings: bool,
    /// Whether a `AGENTS.md` instruction file exists.
    pub has_agents_md: bool,
}

impl TrustContext {
    /// Build a context by inspecting the project directory and its merged
    /// project+local settings (but NOT user/global — only project-scoped
    /// threats should appear in the trust prompt).
    #[must_use]
    pub fn from_project(project_dir: &Path) -> Self {
        let has_settings = project_dir.join(".crab").join("settings.json").exists();
        let has_agents_md = project_dir.join("AGENTS.md").exists();

        let project_settings = crab_config::settings::load_project(project_dir).unwrap_or_default();
        let local_settings = crab_config::settings::load_local(project_dir).unwrap_or_default();
        let merged = project_settings.merge(&local_settings);

        let mcp_servers = merged
            .mcp_servers
            .as_ref()
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();

        let hook_count = merged.hooks.as_ref().map_or(0, |v| {
            crab_config::hooks::parse_hooks(v).map_or(0, |h| h.len())
        });

        let env_vars = merged
            .env
            .as_ref()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        Self {
            mcp_servers,
            hook_count,
            env_vars,
            has_settings,
            has_agents_md,
        }
    }

    /// True when no project configuration was found — the dialog should not
    /// be shown at all in that case.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.has_settings
            && !self.has_agents_md
            && self.mcp_servers.is_empty()
            && self.hook_count == 0
            && self.env_vars.is_empty()
    }
}

pub struct TrustDialogOverlay {
    project_path: String,
    ctx: TrustContext,
    selected: usize,
}

impl TrustDialogOverlay {
    pub fn new(project_path: String, ctx: TrustContext) -> Self {
        Self {
            project_path,
            ctx,
            selected: 0,
        }
    }
}

/// Render a single "threat" line: `  • <label>: <value>`.
fn threat_line<'a>(label: &'a str, value: impl Into<Span<'a>>) -> Line<'a> {
    Line::from(vec![
        Span::styled("  \u{2022} ", Style::default().fg(Color::Yellow)),
        Span::styled(label, Style::default().fg(Color::White)),
        Span::styled(": ", Style::default().fg(Color::Gray)),
        value.into(),
    ])
}

/// Truncate a comma-joined list so it fits inside the popup width.
fn join_truncated(items: &[String], max_chars: usize) -> String {
    if items.is_empty() {
        return String::new();
    }
    let full = items.join(", ");
    if full.chars().count() <= max_chars {
        return full;
    }
    // Show first N that fit, then "(+K more)".
    let mut buf = String::new();
    let mut shown = 0usize;
    for item in items {
        let sep_len = if buf.is_empty() { 0 } else { 2 };
        if buf.chars().count() + sep_len + item.chars().count() + 8 > max_chars {
            break;
        }
        if !buf.is_empty() {
            buf.push_str(", ");
        }
        buf.push_str(item);
        shown += 1;
    }
    let remaining = items.len().saturating_sub(shown);
    if remaining > 0 {
        if !buf.is_empty() {
            buf.push_str(", ");
        }
        let _ = write!(buf, "(+{remaining} more)");
    }
    buf
}

impl Renderable for TrustDialogOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup = centered_popup(area, 72, self.desired_height(area.width));
        Widget::render(Clear, popup, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Trust This Project? ")
            .title_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .padding(Padding::new(2, 2, 1, 1));
        let inner = block.inner(popup);
        Widget::render(block, popup, buf);

        // Budget for the right-hand value on each threat line.
        let value_budget = usize::from(inner.width).saturating_sub(20).max(8);

        let mut lines: Vec<Line<'_>> = vec![
            Line::from(""),
            Line::from(Span::styled(
                &*self.project_path,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "This project will affect how Crab Code behaves:",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
        ];

        if self.ctx.has_agents_md {
            lines.push(threat_line(
                "Instructions",
                Span::styled("AGENTS.md", Style::default().fg(Color::White)),
            ));
        }
        if self.ctx.has_settings {
            lines.push(threat_line(
                "Settings file",
                Span::styled(".crab/settings.json", Style::default().fg(Color::White)),
            ));
        }
        if !self.ctx.mcp_servers.is_empty() {
            let joined = join_truncated(&self.ctx.mcp_servers, value_budget);
            let label = if self.ctx.mcp_servers.len() == 1 {
                "MCP Server "
            } else {
                "MCP Servers"
            };
            lines.push(threat_line(
                label,
                Span::styled(joined, Style::default().fg(Color::Cyan)),
            ));
        }
        if self.ctx.hook_count > 0 {
            let value = format!(
                "{} configured (execute shell commands)",
                self.ctx.hook_count
            );
            lines.push(threat_line(
                "Hooks      ",
                Span::styled(value, Style::default().fg(Color::Red)),
            ));
        }
        if !self.ctx.env_vars.is_empty() {
            let joined = join_truncated(&self.ctx.env_vars, value_budget);
            lines.push(threat_line(
                "Env Vars   ",
                Span::styled(joined, Style::default().fg(Color::Magenta)),
            ));
        }

        lines.push(Line::from(""));

        let accept_style = if self.selected == 0 {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        let deny_style = if self.selected == 1 {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };

        lines.push(Line::from(vec![
            Span::styled("  [ Trust ] ", accept_style),
            Span::styled("  ", Style::default()),
            Span::styled("[ Skip (bare mode) ] ", deny_style),
        ]));

        for (i, line) in lines.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            Widget::render(
                line.clone(),
                Rect {
                    x: inner.x,
                    y: inner.y + i as u16,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // Base chrome = 11 (borders, padding, title, project path, intro, blank, buttons).
        // Each threat line adds 1.
        let mut threats: u16 = 0;
        if self.ctx.has_agents_md {
            threats += 1;
        }
        if self.ctx.has_settings {
            threats += 1;
        }
        if !self.ctx.mcp_servers.is_empty() {
            threats += 1;
        }
        if self.ctx.hook_count > 0 {
            threats += 1;
        }
        if !self.ctx.env_vars.is_empty() {
            threats += 1;
        }
        11 + threats
    }
}

impl Overlay for TrustDialogOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Char('y' | 'Y') => OverlayAction::Execute(AppEvent::TrustAccepted {
                project_path: self.project_path.clone(),
            }),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                OverlayAction::Execute(AppEvent::TrustDenied)
            }
            KeyCode::Enter => {
                if self.selected == 0 {
                    OverlayAction::Execute(AppEvent::TrustAccepted {
                        project_path: self.project_path.clone(),
                    })
                } else {
                    OverlayAction::Execute(AppEvent::TrustDenied)
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.selected = 1 - self.selected;
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::Permission]
    }

    fn name(&self) -> &'static str {
        "trust_dialog"
    }
}

fn centered_popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn base_ctx() -> TrustContext {
        TrustContext {
            has_settings: true,
            has_agents_md: true,
            ..TrustContext::default()
        }
    }

    #[test]
    fn trust_y_accepts() {
        let mut overlay = TrustDialogOverlay::new("/my/project".into(), base_ctx());
        let action = overlay.handle_key(key(KeyCode::Char('y')));
        assert!(matches!(
            action,
            OverlayAction::Execute(AppEvent::TrustAccepted { .. })
        ));
    }

    #[test]
    fn trust_n_denies() {
        let mut overlay = TrustDialogOverlay::new(
            "/my/project".into(),
            TrustContext {
                has_settings: true,
                ..TrustContext::default()
            },
        );
        let action = overlay.handle_key(key(KeyCode::Char('n')));
        assert!(matches!(
            action,
            OverlayAction::Execute(AppEvent::TrustDenied)
        ));
    }

    #[test]
    fn trust_esc_denies() {
        let mut overlay = TrustDialogOverlay::new(
            "/my/project".into(),
            TrustContext {
                has_agents_md: true,
                ..TrustContext::default()
            },
        );
        let action = overlay.handle_key(key(KeyCode::Esc));
        assert!(matches!(
            action,
            OverlayAction::Execute(AppEvent::TrustDenied)
        ));
    }

    #[test]
    fn trust_enter_default_accepts() {
        let mut overlay = TrustDialogOverlay::new("/my/project".into(), base_ctx());
        let action = overlay.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            action,
            OverlayAction::Execute(AppEvent::TrustAccepted { .. })
        ));
    }

    #[test]
    fn trust_tab_toggles() {
        let mut overlay = TrustDialogOverlay::new("/my/project".into(), base_ctx());
        assert_eq!(overlay.selected, 0);

        overlay.handle_key(key(KeyCode::Tab));
        assert_eq!(overlay.selected, 1);

        let action = overlay.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            action,
            OverlayAction::Execute(AppEvent::TrustDenied)
        ));
    }

    #[test]
    fn trust_render_no_panic() {
        let overlay = TrustDialogOverlay::new("/test/project".into(), base_ctx());
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);
    }

    #[test]
    fn trust_render_with_full_threats() {
        let ctx = TrustContext {
            mcp_servers: vec!["filesystem".into(), "github".into(), "sentry".into()],
            hook_count: 3,
            env_vars: vec!["API_KEY".into(), "DEBUG".into()],
            has_settings: true,
            has_agents_md: true,
        };
        let overlay = TrustDialogOverlay::new("/test/project".into(), ctx);
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        // Collect rendered text and verify threat details appear.
        let text = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| {
                        buf.cell((x, y))
                            .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            text.contains("filesystem"),
            "expected MCP server names: {text}"
        );
        assert!(text.contains("github"));
        assert!(text.contains("3 configured"));
        assert!(text.contains("API_KEY"));
        assert!(text.contains("AGENTS.md"));
    }

    #[test]
    fn trust_context_from_empty_dir_is_empty() {
        let dir = std::env::temp_dir().join("crab-trust-empty-dir");
        let _ = std::fs::create_dir_all(&dir);
        let ctx = TrustContext::from_project(&dir);
        assert!(ctx.is_empty());
        assert!(ctx.mcp_servers.is_empty());
        assert_eq!(ctx.hook_count, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trust_context_from_project_enumerates_threats() {
        let dir = std::env::temp_dir().join("crab-trust-ctx-threats");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{
                "mcpServers": {
                    "filesystem": {"command": "fs-mcp"},
                    "github": {"command": "gh-mcp"}
                },
                "hooks": [
                    {"trigger": "pre_tool_use", "command": "echo a"},
                    {"trigger": "stop", "command": "echo b"}
                ],
                "env": {"MY_KEY": "v1", "OTHER": "v2"}
            }"#,
        )
        .unwrap();
        std::fs::write(dir.join("AGENTS.md"), "# hi").unwrap();

        let ctx = TrustContext::from_project(&dir);
        assert!(ctx.has_settings);
        assert!(ctx.has_agents_md);
        assert_eq!(ctx.mcp_servers.len(), 2);
        assert!(ctx.mcp_servers.contains(&"filesystem".to_string()));
        assert!(ctx.mcp_servers.contains(&"github".to_string()));
        assert_eq!(ctx.hook_count, 2);
        assert_eq!(ctx.env_vars.len(), 2);
        assert!(ctx.env_vars.contains(&"MY_KEY".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn join_truncated_shows_overflow_summary() {
        let items: Vec<String> = (0..20).map(|i| format!("server-{i}")).collect();
        let joined = join_truncated(&items, 40);
        assert!(
            joined.contains("more"),
            "expected overflow summary in {joined}"
        );
        assert!(joined.chars().count() <= 40);
    }

    #[test]
    fn join_truncated_short_list_is_full() {
        let items = vec!["a".into(), "b".into()];
        let joined = join_truncated(&items, 40);
        assert_eq!(joined, "a, b");
    }
}
