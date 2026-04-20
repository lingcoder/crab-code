//! Inline permission card — CC-aligned tool execution confirmation.
//!
//! Renders as an inline card in the message flow with top-border only,
//! per-tool-type content, and vertical option selection.
//! Matches CC's `PermissionDialog.tsx` + per-tool `*PermissionRequest` components.

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap};

use crate::theme::{self, Accents};

// ─── Types ───────────────────────────────────────────────────────────

/// Permission card variant — determines title, content display, and available options.
///
/// Maps to CC's per-tool `*PermissionRequest` components:
/// `BashPermissionRequest`, `FileEditPermissionRequest`, `FileWritePermissionRequest`,
/// `WebFetchPermissionRequest`, `FallbackPermissionRequest`.
#[derive(Debug, Clone)]
pub enum PermissionKind {
    /// Shell command execution.
    /// CC: `BashPermissionRequest` — title "Bash command", shows command text.
    Bash {
        command: String,
        description: Option<String>,
    },
    /// File edit operation.
    /// CC: `FileEditPermissionRequest` — title "Edit file", shows path.
    FileEdit { path: String },
    /// File creation or overwrite.
    /// CC: `FileWritePermissionRequest` — title "Create file" / "Overwrite file".
    FileWrite { path: String, file_exists: bool },
    /// URL fetch.
    /// CC: `WebFetchPermissionRequest` — title "Fetch", shows domain.
    WebFetch { url: String },
    /// Notebook cell edit.
    /// CC: `NotebookEditPermissionRequest` — title "Edit notebook".
    NotebookEdit { path: String },
    /// Generic / fallback for any other tool.
    /// CC: `FallbackPermissionRequest` — title "Tool use".
    Generic {
        tool_name: String,
        input_summary: String,
    },
}

impl PermissionKind {
    /// Card title — matches CC's per-component title strings.
    fn title(&self) -> &str {
        match self {
            Self::Bash { .. } => "Bash command",
            Self::FileEdit { .. } => "Edit file",
            Self::FileWrite { file_exists, .. } => {
                if *file_exists {
                    "Overwrite file"
                } else {
                    "Create file"
                }
            }
            Self::WebFetch { .. } => "Fetch",
            Self::NotebookEdit { .. } => "Edit notebook",
            Self::Generic { .. } => "Tool use",
        }
    }
}

/// Resolve the accent triple for the permission card from the current
/// theme. `border` is the top-border color, `selected` tints the active
/// option, `label` styles the primary title.
fn accents() -> Accents {
    theme::current().accents()
}

/// Permission-specific border / selection color.
fn permission_color() -> Color {
    accents().permission
}

/// Selected-option color (uses the theme's main accent).
fn selected_color() -> Color {
    theme::current().accent
}

/// Label (title text, content emphasis) color.
fn label_color() -> Color {
    theme::current().text_bright
}

/// Body text color for non-emphasized content.
fn body_color() -> Color {
    theme::current().fg
}

/// Muted color for descriptions / hints.
fn muted_color() -> Color {
    theme::current().muted
}

/// User response to a permission card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResponse {
    /// Allow this single execution.
    Allow,
    /// Deny this execution.
    Deny,
    /// Allow and remember (don't ask again for this tool/prefix in this session).
    AllowAlways,
}

/// A single selectable option in the permission card.
#[derive(Debug, Clone)]
struct PermissionOption {
    /// Display label (may contain bold segments via spans).
    label: String,
    /// Shortcut key hint (shown dimmed).
    hint: Option<char>,
    /// Response value when selected.
    response: PermissionResponse,
}

/// Inline permission card — the main permission UI component.
///
/// CC architecture: `PermissionDialog` base wrapper + per-tool content.
/// Renders inline in the message flow with top-border only, vertical options.
pub struct PermissionCard {
    /// Permission type — determines title, content, and options.
    pub kind: PermissionKind,
    /// Unique request ID for tracking.
    pub request_id: String,
    /// Available options (built from kind).
    options: Vec<PermissionOption>,
    /// Currently highlighted option index.
    selected: usize,
}

impl PermissionCard {
    /// Create a permission card from a raw event.
    ///
    /// Classifies the tool name into the appropriate `PermissionKind` and
    /// builds the option set. Maps CC's `PermissionRequest.tsx` routing logic.
    pub fn from_event(tool_name: &str, input_summary: &str, request_id: String) -> Self {
        let kind = classify_permission_kind(tool_name, input_summary);
        let options = build_options(&kind);
        Self {
            kind,
            request_id,
            options,
            selected: 0,
        }
    }

    /// Handle a key event. Returns `Some(response)` when the user confirms.
    ///
    /// Navigation: Up/Down (vertical list, matching CC's `<Select>` component).
    /// Shortcuts: y = Allow, n/Esc = Deny, a = `AllowAlways`.
    pub fn handle_key(&mut self, code: KeyCode) -> Option<PermissionResponse> {
        match code {
            // Vertical navigation (CC uses Up/Down for Select)
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected < self.options.len() - 1 {
                    self.selected += 1;
                }
                None
            }
            // Confirm selection
            KeyCode::Enter => Some(self.options[self.selected].response.clone()),
            // Shortcut keys
            KeyCode::Char('y' | 'Y') => Some(PermissionResponse::Allow),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(PermissionResponse::Deny),
            KeyCode::Char('a' | 'A') => {
                // Only if AlwaysAllow is available
                if self
                    .options
                    .iter()
                    .any(|o| o.response == PermissionResponse::AllowAlways)
                {
                    Some(PermissionResponse::AllowAlways)
                } else {
                    Some(PermissionResponse::Allow)
                }
            }
            _ => None,
        }
    }

    /// Currently selected option index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Render the permission card into pre-allocated lines for inline display.
    ///
    /// Returns a `Vec<Line>` that can be appended to the message flow.
    /// This is the preferred rendering path — the card appears inline in
    /// the conversation, not as an overlay.
    #[must_use]
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let w = width as usize;
        let mut lines = Vec::new();

        // ─── Top border with title (rounded, top-border only) ───
        let title = self.kind.title();
        let border_color = permission_color();

        // Build: ╭─ Title ─────────────────────╮
        let title_segment = format!(" {title} ");
        let remaining = w.saturating_sub(2 + title_segment.len()); // 2 for ╭ and ╮
        let right_border = "─".repeat(remaining);

        lines.push(Line::from(vec![
            Span::styled("╭─", Style::default().fg(border_color)),
            Span::styled(
                title_segment,
                Style::default()
                    .fg(label_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(right_border, Style::default().fg(border_color)),
        ]));

        // ─── Content area (varies by kind) ───
        let content_lines = self.render_content(w);
        lines.extend(content_lines);

        // ─── Blank line before options ───
        lines.push(Line::default());

        // ─── Options (vertical select list) ───
        for (i, opt) in self.options.iter().enumerate() {
            let is_selected = i == self.selected;
            let prefix = if is_selected { "  ▸ " } else { "    " };
            let label_style = if is_selected {
                Style::default()
                    .fg(selected_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(label_color())
            };

            let mut spans = vec![
                Span::styled(prefix, label_style),
                Span::styled(opt.label.clone(), label_style),
            ];

            if let Some(hint) = opt.hint {
                spans.push(Span::styled(
                    format!("  ({hint})"),
                    Style::default().fg(muted_color()),
                ));
            }

            lines.push(Line::from(spans));
        }

        // ─── Footer hint ───
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Esc to deny",
            Style::default().fg(muted_color()),
        )));

        lines
    }

    /// Render the per-tool-type content section.
    fn render_content(&self, _width: usize) -> Vec<Line<'static>> {
        let dim = Style::default().fg(muted_color());
        let normal = Style::default().fg(body_color());
        let emphasis = Style::default()
            .fg(label_color())
            .add_modifier(Modifier::BOLD);

        match &self.kind {
            PermissionKind::Bash {
                command,
                description,
            } => {
                let mut lines = vec![Line::from(vec![
                    Span::styled("  ", dim),
                    Span::styled(command.clone(), Style::default().fg(label_color())),
                ])];
                if let Some(desc) = description
                    && !desc.is_empty()
                {
                    lines.push(Line::from(Span::styled(format!("  {desc}"), dim)));
                }
                lines
            }
            PermissionKind::FileEdit { path } | PermissionKind::NotebookEdit { path } => {
                vec![Line::from(vec![
                    Span::styled("  ", dim),
                    Span::styled(path.clone(), normal),
                ])]
            }
            PermissionKind::WebFetch { url } => {
                vec![Line::from(vec![
                    Span::styled("  ", dim),
                    Span::styled(url.clone(), normal),
                ])]
            }
            PermissionKind::FileWrite { path, file_exists } => {
                let verb = if *file_exists { "overwrite" } else { "create" };
                vec![Line::from(vec![
                    Span::styled(format!("  Do you want to {verb} "), dim),
                    Span::styled(path.clone(), emphasis),
                    Span::styled("?", dim),
                ])]
            }
            PermissionKind::Generic {
                tool_name,
                input_summary,
            } => {
                let mut lines = vec![Line::from(vec![
                    Span::styled("  ", dim),
                    Span::styled(tool_name.clone(), emphasis),
                ])];
                // Truncate summary to 3 lines (CC: truncateToLines(text, 3))
                let summary_lines: Vec<&str> = input_summary.lines().collect();
                let show = summary_lines.len().min(3);
                for line in &summary_lines[..show] {
                    lines.push(Line::from(Span::styled(format!("  {line}"), dim)));
                }
                if summary_lines.len() > 3 {
                    lines.push(Line::from(Span::styled("  ...", dim)));
                }
                lines
            }
        }
    }
}

/// Render the permission card as a ratatui `Widget`.
///
/// This is used when rendering the card in a fixed area (e.g., at the bottom
/// of the content region). For inline message flow rendering, use `render_lines()`.
impl Widget for &PermissionCard {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 20 {
            return;
        }

        let border_color = permission_color();

        // Top-border-only block — rounded corners, only the top edge is drawn
        // so the card reads as "attached to what's below".
        let block = Block::default()
            .title(format!(" {} ", self.kind.title()))
            .title_style(
                Style::default()
                    .fg(label_color())
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::TOP)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        Widget::render(block, area, buf);

        if inner.height < 3 || inner.width < 10 {
            return;
        }

        // Split inner: content + spacer + options + footer
        let option_count = self.options.len() as u16;
        let chunks = Layout::vertical([
            Constraint::Min(1),               // content
            Constraint::Length(1),            // spacer
            Constraint::Length(option_count), // options
            Constraint::Length(1),            // footer hint
        ])
        .split(inner);

        // Content
        let content_lines = self.render_content(inner.width as usize);
        for (i, line) in content_lines.iter().enumerate() {
            if i >= chunks[0].height as usize {
                break;
            }
            Widget::render(
                line.clone(),
                Rect {
                    x: chunks[0].x,
                    y: chunks[0].y + i as u16,
                    width: chunks[0].width,
                    height: 1,
                },
                buf,
            );
        }

        // Options (vertical select)
        for (i, opt) in self.options.iter().enumerate() {
            if i >= chunks[2].height as usize {
                break;
            }
            let y = chunks[2].y + i as u16;
            let is_selected = i == self.selected;
            let prefix = if is_selected { " ▸ " } else { "   " };
            let label_style = if is_selected {
                Style::default()
                    .fg(selected_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(label_color())
            };

            let mut spans = vec![
                Span::styled(prefix, label_style),
                Span::styled(&opt.label, label_style),
            ];
            if let Some(hint) = opt.hint {
                spans.push(Span::styled(
                    format!("  ({hint})"),
                    Style::default().fg(muted_color()),
                ));
            }

            Widget::render(
                Line::from(spans),
                Rect {
                    x: chunks[2].x,
                    y,
                    width: chunks[2].width,
                    height: 1,
                },
                buf,
            );
        }

        // Footer hint
        let hint = Paragraph::new("Esc to deny")
            .style(Style::default().fg(muted_color()))
            .wrap(Wrap { trim: true });
        Widget::render(
            hint,
            Rect {
                x: chunks[3].x + 1,
                y: chunks[3].y,
                width: chunks[3].width.saturating_sub(1),
                height: 1,
            },
            buf,
        );
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Classify tool name into a `PermissionKind`.
///
/// Maps CC's `PermissionRequest.tsx` switch-case routing.
/// Matches both canonical names (`"Bash"`) and lowercase variants (`"bash"`).
fn classify_permission_kind(tool_name: &str, input_summary: &str) -> PermissionKind {
    let lower = tool_name.to_ascii_lowercase();
    match lower.as_str() {
        "bash" => PermissionKind::Bash {
            command: input_summary.to_string(),
            description: None,
        },
        "edit" => PermissionKind::FileEdit {
            path: input_summary.to_string(),
        },
        "write" => PermissionKind::FileWrite {
            path: input_summary.to_string(),
            file_exists: false,
        },
        "notebookedit" | "notebook_edit" => PermissionKind::NotebookEdit {
            path: input_summary.to_string(),
        },
        name if name.contains("fetch") || name.contains("web") => PermissionKind::WebFetch {
            url: input_summary.to_string(),
        },
        _ => PermissionKind::Generic {
            tool_name: tool_name.to_string(),
            input_summary: input_summary.to_string(),
        },
    }
}

/// Build the option list for a permission kind.
///
/// CC options per tool type:
/// - `Bash`: Yes (y) / Yes, don't ask again (a) / No (n)
/// - `FileEdit`: Yes (y) / No (n)
/// - `FileWrite`: Yes (y) / No (n)
/// - `WebFetch`: Yes (y) / Yes, don't ask again for domain (a) / No (n)
/// - `Generic`: Yes (y) / Yes, don't ask again (a) / No (n)
fn build_options(kind: &PermissionKind) -> Vec<PermissionOption> {
    match kind {
        PermissionKind::Bash { .. } => vec![
            PermissionOption {
                label: "Yes".to_string(),
                hint: Some('y'),
                response: PermissionResponse::Allow,
            },
            PermissionOption {
                label: "Yes, and don't ask again".to_string(),
                hint: Some('a'),
                response: PermissionResponse::AllowAlways,
            },
            PermissionOption {
                label: "No".to_string(),
                hint: Some('n'),
                response: PermissionResponse::Deny,
            },
        ],
        PermissionKind::FileEdit { .. }
        | PermissionKind::FileWrite { .. }
        | PermissionKind::NotebookEdit { .. } => vec![
            PermissionOption {
                label: "Yes".to_string(),
                hint: Some('y'),
                response: PermissionResponse::Allow,
            },
            PermissionOption {
                label: "No".to_string(),
                hint: Some('n'),
                response: PermissionResponse::Deny,
            },
        ],
        PermissionKind::WebFetch { url } => {
            // Extract domain for "don't ask again" label
            let domain = extract_domain(url);
            vec![
                PermissionOption {
                    label: "Yes".to_string(),
                    hint: Some('y'),
                    response: PermissionResponse::Allow,
                },
                PermissionOption {
                    label: format!("Yes, don't ask again for {domain}"),
                    hint: Some('a'),
                    response: PermissionResponse::AllowAlways,
                },
                PermissionOption {
                    label: "No".to_string(),
                    hint: Some('n'),
                    response: PermissionResponse::Deny,
                },
            ]
        }
        PermissionKind::Generic { tool_name, .. } => vec![
            PermissionOption {
                label: "Yes".to_string(),
                hint: Some('y'),
                response: PermissionResponse::Allow,
            },
            PermissionOption {
                label: format!("Yes, don't ask again for {tool_name}"),
                hint: Some('a'),
                response: PermissionResponse::AllowAlways,
            },
            PermissionOption {
                label: "No".to_string(),
                hint: Some('n'),
                response: PermissionResponse::Deny,
            },
        ],
    }
}

/// Extract domain from a URL for display.
fn extract_domain(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bash_card() -> PermissionCard {
        PermissionCard::from_event("bash", "rm -rf /tmp/cache", "req_1".into())
    }

    fn edit_card() -> PermissionCard {
        PermissionCard::from_event("edit", "src/main.rs", "req_2".into())
    }

    fn generic_card() -> PermissionCard {
        PermissionCard::from_event("mcp_tool", "some input data", "req_3".into())
    }

    #[test]
    fn bash_card_has_three_options() {
        let card = bash_card();
        assert_eq!(card.options.len(), 3);
        assert!(matches!(card.kind, PermissionKind::Bash { .. }));
        assert_eq!(card.kind.title(), "Bash command");
    }

    #[test]
    fn edit_card_has_two_options() {
        let card = edit_card();
        assert_eq!(card.options.len(), 2);
        assert!(matches!(card.kind, PermissionKind::FileEdit { .. }));
        assert_eq!(card.kind.title(), "Edit file");
    }

    #[test]
    fn generic_card_has_three_options() {
        let card = generic_card();
        assert_eq!(card.options.len(), 3);
        assert!(matches!(card.kind, PermissionKind::Generic { .. }));
        assert_eq!(card.kind.title(), "Tool use");
    }

    #[test]
    fn navigate_up_down() {
        let mut card = bash_card();
        assert_eq!(card.selected(), 0);

        card.handle_key(KeyCode::Down);
        assert_eq!(card.selected(), 1);

        card.handle_key(KeyCode::Down);
        assert_eq!(card.selected(), 2);

        // Clamp at end
        card.handle_key(KeyCode::Down);
        assert_eq!(card.selected(), 2);

        card.handle_key(KeyCode::Up);
        assert_eq!(card.selected(), 1);

        // Clamp at start
        card.handle_key(KeyCode::Up);
        card.handle_key(KeyCode::Up);
        assert_eq!(card.selected(), 0);
    }

    #[test]
    fn enter_confirms_selected() {
        let mut card = bash_card();
        assert_eq!(
            card.handle_key(KeyCode::Enter),
            Some(PermissionResponse::Allow)
        );

        card.handle_key(KeyCode::Down);
        assert_eq!(
            card.handle_key(KeyCode::Enter),
            Some(PermissionResponse::AllowAlways)
        );

        card.handle_key(KeyCode::Down);
        assert_eq!(
            card.handle_key(KeyCode::Enter),
            Some(PermissionResponse::Deny)
        );
    }

    #[test]
    fn shortcut_y_allows() {
        let mut card = bash_card();
        assert_eq!(
            card.handle_key(KeyCode::Char('y')),
            Some(PermissionResponse::Allow)
        );
    }

    #[test]
    fn shortcut_n_denies() {
        let mut card = bash_card();
        assert_eq!(
            card.handle_key(KeyCode::Char('n')),
            Some(PermissionResponse::Deny)
        );
    }

    #[test]
    fn esc_denies() {
        let mut card = bash_card();
        assert_eq!(
            card.handle_key(KeyCode::Esc),
            Some(PermissionResponse::Deny)
        );
    }

    #[test]
    fn shortcut_a_always_allows() {
        let mut card = bash_card();
        assert_eq!(
            card.handle_key(KeyCode::Char('a')),
            Some(PermissionResponse::AllowAlways)
        );
    }

    #[test]
    fn shortcut_a_falls_back_when_no_always_option() {
        let mut card = edit_card();
        // Edit only has Yes/No, no AlwaysAllow
        assert_eq!(
            card.handle_key(KeyCode::Char('a')),
            Some(PermissionResponse::Allow)
        );
    }

    #[test]
    fn vim_navigation() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Char('j'));
        assert_eq!(card.selected(), 1);
        card.handle_key(KeyCode::Char('k'));
        assert_eq!(card.selected(), 0);
    }

    #[test]
    fn unknown_key_returns_none() {
        let mut card = bash_card();
        assert_eq!(card.handle_key(KeyCode::F(1)), None);
        assert_eq!(card.handle_key(KeyCode::Tab), None);
    }

    #[test]
    fn write_card_uses_overwrite_title() {
        let card = PermissionCard::from_event("write", "output.txt", "req_w".into());
        // Default: file_exists = false → "Create file"
        assert_eq!(card.kind.title(), "Create file");
    }

    #[test]
    fn web_fetch_detection() {
        let card =
            PermissionCard::from_event("web_fetch", "https://example.com/api", "req_f".into());
        assert!(matches!(card.kind, PermissionKind::WebFetch { .. }));
        assert_eq!(card.kind.title(), "Fetch");
    }

    #[test]
    fn extract_domain_works() {
        assert_eq!(extract_domain("https://example.com/path"), "example.com");
        assert_eq!(extract_domain("http://api.test.io/v1/data"), "api.test.io");
        assert_eq!(extract_domain("no-scheme"), "no-scheme");
    }

    #[test]
    fn render_lines_produces_output() {
        let card = bash_card();
        let lines = card.render_lines(80);
        assert!(lines.len() >= 6); // border + content + spacer + 3 options + spacer + hint

        // First line should contain the title
        let first_text: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(first_text.contains("Bash command"));
    }

    #[test]
    fn widget_render_does_not_panic() {
        let card = bash_card();
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        Widget::render(&card, area, &mut buf);
    }

    #[test]
    fn widget_render_tiny_area_does_not_panic() {
        let card = bash_card();
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        Widget::render(&card, area, &mut buf);
    }

    #[test]
    fn widget_render_contains_title() {
        let card = bash_card();
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        Widget::render(&card, area, &mut buf);

        let buf_ref = &buf;
        let all_text: String = (0..area.height)
            .flat_map(|y| {
                (0..area.width).map(move |x| buf_ref.cell((x, y)).unwrap().symbol().to_string())
            })
            .collect();
        assert!(all_text.contains("Bash command"));
    }

    #[test]
    fn notebook_edit_detected() {
        let card = PermissionCard::from_event("notebook_edit", "analysis.ipynb", "req_n".into());
        assert!(matches!(card.kind, PermissionKind::NotebookEdit { .. }));
        assert_eq!(card.kind.title(), "Edit notebook");
    }
}
