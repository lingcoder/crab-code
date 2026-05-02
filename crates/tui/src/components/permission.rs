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
        /// `(label, color)` derived from `bash_classifier::classify_command`.
        /// Surfaces the command's risk category as a badge above the
        /// description so the user sees "dangerous" / "read-only" / etc.
        /// before granting permission.
        risk_badge: Option<(String, Color)>,
    },
    /// File edit operation.
    /// CC: `FileEditPermissionRequest` — title "Edit file", shows path + optional diff preview.
    FileEdit { path: String, diff: Option<String> },
    /// File creation or overwrite.
    /// CC: `FileWritePermissionRequest` — title "Create file" / "Overwrite file",
    /// shows before/after diff when overwriting an existing file.
    FileWrite {
        path: String,
        file_exists: bool,
        diff: Option<String>,
        content_preview: Option<String>,
    },
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
    /// Canonical tool name for this kind (used for session-level grants).
    pub fn tool_name(&self) -> &str {
        match self {
            Self::Bash { .. } => "bash",
            Self::FileEdit { .. } => "edit",
            Self::FileWrite { .. } => "write",
            Self::WebFetch { .. } => "web_fetch",
            Self::NotebookEdit { .. } => "notebook_edit",
            Self::Generic { tool_name, .. } => tool_name,
        }
    }

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
    /// Deny this execution and pass a free-text note back to the model so it
    /// can adjust its approach (e.g. "use Read instead of Bash cat").
    DenyWithFeedback(String),
    /// Allow and remember (don't ask again for this tool/prefix in this session).
    AllowAlways,
}

impl PermissionResponse {
    /// Whether this response allows the tool to run.
    #[must_use]
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow | Self::AllowAlways)
    }

    /// Free-text feedback the user supplied alongside this response, if any.
    #[must_use]
    pub fn feedback(&self) -> Option<&str> {
        match self {
            Self::DenyWithFeedback(text) => Some(text.as_str()),
            _ => None,
        }
    }
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
    /// Whether the card is currently capturing free-text feedback to attach
    /// to a deny. Toggled by Tab. While true, options/shortcuts are inert
    /// and key events go into `feedback_text`.
    pub feedback_mode: bool,
    /// Buffered feedback text the user is composing (visible in feedback mode).
    pub feedback_text: String,
}

impl PermissionCard {
    /// Create a permission card from a raw event.
    ///
    /// Classifies the tool name into the appropriate `PermissionKind` and
    /// builds the option set. The `tool_input` JSON is the source of truth
    /// for tool-specific fields (bash command, edit paths, write target,
    /// fetch URL); `input_summary` is a fallback for tools that did not
    /// emit structured input.
    pub fn from_event(
        tool_name: &str,
        input_summary: &str,
        request_id: String,
        tool_input: &serde_json::Value,
    ) -> Self {
        let kind = classify_permission_kind(tool_name, input_summary, tool_input);
        let options = build_options(&kind);
        Self {
            kind,
            request_id,
            options,
            selected: 0,
            feedback_mode: false,
            feedback_text: String::new(),
        }
    }

    /// Return (`tool_name`, summary) for a rejection message.
    pub fn rejection_summary(&self) -> (String, String) {
        match &self.kind {
            PermissionKind::Bash { command, .. } => {
                let short = if command.len() > 60 {
                    format!("{}…", &command[..60])
                } else {
                    command.clone()
                };
                ("bash".into(), format!("Run rejected ({short})"))
            }
            PermissionKind::FileEdit { path, .. } => {
                let f = path.rsplit(['/', '\\']).next().unwrap_or(path);
                ("edit".into(), format!("Edit rejected ({f})"))
            }
            PermissionKind::FileWrite { path, .. } => {
                let f = path.rsplit(['/', '\\']).next().unwrap_or(path);
                ("write".into(), format!("Write rejected ({f})"))
            }
            PermissionKind::WebFetch { url } => {
                ("web_fetch".into(), format!("Fetch rejected ({url})"))
            }
            PermissionKind::NotebookEdit { path } => {
                let f = path.rsplit(['/', '\\']).next().unwrap_or(path);
                (
                    "notebook_edit".into(),
                    format!("Notebook edit rejected ({f})"),
                )
            }
            PermissionKind::Generic {
                tool_name,
                input_summary,
            } => (
                tool_name.clone(),
                format!("{tool_name} rejected ({input_summary})"),
            ),
        }
    }

    /// Handle a key event. Returns `Some(response)` when the user confirms.
    ///
    /// Two modes drive the dispatch table:
    ///
    /// - **Decision mode** (default). Vertical navigation with Up/Down or
    ///   j/k; Enter confirms the highlighted option; y/n/a/Esc are
    ///   shortcuts. Tab enters feedback mode without producing a response.
    /// - **Feedback mode** (`self.feedback_mode == true`). Printable chars
    ///   append to `feedback_text`, Backspace pops one character, Esc
    ///   cancels (clears the buffer and returns to decision mode), Enter
    ///   submits a [`PermissionResponse::DenyWithFeedback`]. All other
    ///   keys are inert so the input flow doesn't accidentally allow.
    pub fn handle_key(&mut self, code: KeyCode) -> Option<PermissionResponse> {
        if self.feedback_mode {
            return self.handle_feedback_key(code);
        }
        match code {
            // Tab toggles feedback mode — defer the decision until the user
            // either submits feedback (Enter) or cancels (Esc).
            KeyCode::Tab => {
                self.feedback_mode = true;
                None
            }
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

    /// Feedback-mode key handler. Splits out so `handle_key` stays scannable.
    fn handle_feedback_key(&mut self, code: KeyCode) -> Option<PermissionResponse> {
        match code {
            KeyCode::Esc => {
                self.feedback_mode = false;
                self.feedback_text.clear();
                None
            }
            KeyCode::Enter => {
                let text = std::mem::take(&mut self.feedback_text);
                self.feedback_mode = false;
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    Some(PermissionResponse::Deny)
                } else {
                    Some(PermissionResponse::DenyWithFeedback(trimmed.to_string()))
                }
            }
            KeyCode::Backspace => {
                self.feedback_text.pop();
                None
            }
            KeyCode::Char(c) => {
                self.feedback_text.push(c);
                None
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

        // ─── Blank line before options / feedback area ───
        lines.push(Line::default());

        if self.feedback_mode {
            lines.extend(self.render_feedback_area());
        } else {
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
        }

        // ─── Footer hint ───
        lines.push(Line::default());
        let hint = if self.feedback_mode {
            "  Enter to deny with feedback, Esc to cancel"
        } else {
            "  Esc to deny  ·  Tab to add feedback"
        };
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(muted_color()),
        )));

        lines
    }

    /// Render the inline feedback input area shown when `feedback_mode` is on.
    /// Two lines: a prompt with the buffered text + cursor glyph, and a hint
    /// describing how to commit or cancel.
    fn render_feedback_area(&self) -> Vec<Line<'static>> {
        let dim = Style::default().fg(muted_color());
        let label_style = Style::default()
            .fg(label_color())
            .add_modifier(Modifier::BOLD);
        let body = Style::default().fg(body_color());

        let mut spans = vec![
            Span::styled("  ", dim),
            Span::styled("Feedback: ", label_style),
            Span::styled(self.feedback_text.clone(), body),
            // Block cursor glyph — visually distinct but doesn't depend on
            // terminal cursor positioning, which is owned by the input box.
            Span::styled("▎", Style::default().fg(selected_color())),
        ];
        if self.feedback_text.is_empty() {
            spans.push(Span::styled(
                "  (type a note for the model)",
                Style::default().fg(muted_color()),
            ));
        }
        vec![Line::from(spans)]
    }

    /// Render the per-tool-type content section.
    fn render_content(&self, width: usize) -> Vec<Line<'static>> {
        let dim = Style::default().fg(muted_color());
        let normal = Style::default().fg(body_color());
        let emphasis = Style::default()
            .fg(label_color())
            .add_modifier(Modifier::BOLD);
        let code_style = Style::default().fg(Color::Cyan);

        match &self.kind {
            PermissionKind::Bash {
                command,
                description,
                risk_badge,
            } => {
                let mut lines = Vec::new();
                let cmd_lines: Vec<&str> = command.lines().collect();
                let show_count = cmd_lines.len().min(5);
                for (i, line) in cmd_lines[..show_count].iter().enumerate() {
                    let prefix = if i == 0 { "  $ " } else { "    " };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, dim),
                        Span::styled((*line).to_string(), code_style),
                    ]));
                }
                if cmd_lines.len() > 5 {
                    lines.push(Line::from(Span::styled(
                        format!("    ... ({} more lines)", cmd_lines.len() - 5),
                        dim,
                    )));
                }
                if let Some((label, color)) = risk_badge {
                    let badge_style = Style::default().fg(*color).add_modifier(Modifier::BOLD);
                    let mut spans = vec![
                        Span::styled("  [", dim),
                        Span::styled(label.clone(), badge_style),
                        Span::styled("]", dim),
                    ];
                    if let Some(desc) = description
                        && !desc.is_empty()
                    {
                        spans.push(Span::styled(format!(" {desc}"), dim));
                    }
                    lines.push(Line::from(spans));
                } else if let Some(desc) = description
                    && !desc.is_empty()
                {
                    lines.push(Line::from(Span::styled(format!("  {desc}"), dim)));
                }
                lines
            }
            PermissionKind::FileEdit { path, diff } => {
                let mut lines = vec![Line::from(vec![
                    Span::styled("  ", dim),
                    Span::styled(path.clone(), normal),
                ])];
                if let Some(diff_text) = diff {
                    lines.push(Line::default());
                    render_diff_lines(diff_text, dim, &mut lines);
                }
                lines
            }
            PermissionKind::NotebookEdit { path } => {
                vec![Line::from(vec![
                    Span::styled("  ", dim),
                    Span::styled(path.clone(), normal),
                ])]
            }
            PermissionKind::WebFetch { url } => render_parsed_url(url, width),
            PermissionKind::FileWrite {
                path,
                file_exists,
                diff,
                content_preview,
            } => {
                let verb = if *file_exists { "overwrite" } else { "create" };
                let mut lines = vec![Line::from(vec![
                    Span::styled(format!("  Do you want to {verb} "), dim),
                    Span::styled(path.clone(), emphasis),
                    Span::styled("?", dim),
                ])];
                if let Some(diff_text) = diff {
                    lines.push(Line::default());
                    render_diff_lines(diff_text, dim, &mut lines);
                } else if let Some(preview) = content_preview {
                    lines.push(Line::default());
                    let add_style = Style::default().fg(Color::Green);
                    for line in preview.lines() {
                        lines.push(Line::from(Span::styled(format!("  +{line}"), add_style)));
                    }
                }
                lines
            }
            PermissionKind::Generic {
                tool_name,
                input_summary,
            } => {
                let mut lines = Vec::new();
                if let Some((server, tool)) = parse_mcp_tool_name(tool_name) {
                    lines.push(Line::from(vec![
                        Span::styled("  ", dim),
                        Span::styled(server, Style::default().fg(Color::Magenta)),
                        Span::styled("::", dim),
                        Span::styled(tool, emphasis),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("  ", dim),
                        Span::styled(tool_name.clone(), emphasis),
                    ]));
                }
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

        // Split inner: content + spacer + (options OR feedback) + footer
        let middle_height = if self.feedback_mode {
            1u16
        } else {
            self.options.len() as u16
        };
        let chunks = Layout::vertical([
            Constraint::Min(1),                // content
            Constraint::Length(1),             // spacer
            Constraint::Length(middle_height), // options OR feedback input
            Constraint::Length(1),             // footer hint
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

        if self.feedback_mode {
            for (i, line) in self.render_feedback_area().iter().enumerate() {
                if i >= chunks[2].height as usize {
                    break;
                }
                Widget::render(
                    line.clone(),
                    Rect {
                        x: chunks[2].x,
                        y: chunks[2].y + i as u16,
                        width: chunks[2].width,
                        height: 1,
                    },
                    buf,
                );
            }
        } else {
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
        }

        // Footer hint
        let hint_text = if self.feedback_mode {
            "Enter to deny with feedback, Esc to cancel"
        } else {
            "Esc to deny  ·  Tab to add feedback"
        };
        let hint = Paragraph::new(hint_text)
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
/// Pulls structured fields out of `tool_input` so the rendered card shows
/// real command / path / URL instead of the (often truncated) `input_summary`.
fn classify_permission_kind(
    tool_name: &str,
    input_summary: &str,
    tool_input: &serde_json::Value,
) -> PermissionKind {
    let lower = tool_name.to_ascii_lowercase();
    match lower.as_str() {
        "bash" => {
            let command = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or(input_summary)
                .to_string();
            let description = tool_input
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from);
            let risk_badge = Some(classify_bash_risk(&command));
            PermissionKind::Bash {
                command,
                description,
                risk_badge,
            }
        }
        "edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or(input_summary)
                .to_string();
            let diff = compute_edit_diff(tool_input, &path);
            PermissionKind::FileEdit { path, diff }
        }
        "write" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or(input_summary)
                .to_string();
            let file_exists = std::path::Path::new(&path).exists();
            let diff = compute_write_diff(tool_input, &path);
            let content_preview = if file_exists {
                None
            } else {
                tool_input.get("content").and_then(|v| v.as_str()).map(|s| {
                    let preview_lines: Vec<&str> = s.lines().take(10).collect();
                    preview_lines.join("\n")
                })
            };
            PermissionKind::FileWrite {
                path,
                file_exists,
                diff,
                content_preview,
            }
        }
        "notebookedit" | "notebook_edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or(input_summary)
                .to_string();
            PermissionKind::NotebookEdit { path }
        }
        name if name.contains("fetch") || name.contains("web") => {
            let url = tool_input
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or(input_summary)
                .to_string();
            PermissionKind::WebFetch { url }
        }
        _ => {
            let summary = truncate_tool_input(tool_input, input_summary);
            PermissionKind::Generic {
                tool_name: tool_name.to_string(),
                input_summary: summary,
            }
        }
    }
}

/// Append colored diff lines to an output buffer. No line cap — shows
/// the complete unified diff, matching CCB behavior.
fn render_diff_lines(diff_text: &str, dim: Style, out: &mut Vec<Line<'static>>) {
    for line in diff_text.lines() {
        let style = if line.starts_with('+') {
            Style::default().fg(Color::Green)
        } else if line.starts_with('-') {
            Style::default().fg(Color::Red)
        } else {
            dim
        };
        out.push(Line::from(Span::styled(format!("  {line}"), style)));
    }
}

/// Build a unified diff of a pending `Edit` tool call by replaying the
/// `old_string` → `new_string` substitution against the file on disk.
/// Returns `None` if the file is unreadable or the substitution does not match.
fn compute_edit_diff(tool_input: &serde_json::Value, path: &str) -> Option<String> {
    let old_str = tool_input.get("old_string")?.as_str()?;
    let new_str = tool_input.get("new_string")?.as_str()?;
    let content = std::fs::read_to_string(path).ok()?;
    let replaced = content.replacen(old_str, new_str, 1);
    if replaced == content {
        return None;
    }
    let label = std::path::Path::new(path).file_name()?.to_str()?;
    Some(
        similar::TextDiff::from_lines(&content, &replaced)
            .unified_diff()
            .context_radius(3)
            .header(label, label)
            .to_string(),
    )
}

/// Build a unified diff of a pending `Write` tool call by comparing the
/// existing file on disk with the new `content` from tool input.
/// Returns `None` if the file doesn't exist or `content` is absent.
fn compute_write_diff(tool_input: &serde_json::Value, path: &str) -> Option<String> {
    let new_content = tool_input.get("content")?.as_str()?;
    let old_content = std::fs::read_to_string(path).ok()?;
    if old_content == new_content {
        return None;
    }
    let label = std::path::Path::new(path).file_name()?.to_str()?;
    Some(
        similar::TextDiff::from_lines(&old_content, new_content)
            .unified_diff()
            .context_radius(3)
            .header(label, label)
            .to_string(),
    )
}

/// Render `tool_input` as a compact JSON summary, falling back to
/// `fallback` when the input is null or empty. Truncates beyond 200 chars
/// so the generic permission card stays readable.
fn truncate_tool_input(tool_input: &serde_json::Value, fallback: &str) -> String {
    if tool_input.is_null()
        || tool_input
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
    {
        return fallback.to_string();
    }
    let s = serde_json::to_string_pretty(tool_input).unwrap_or_default();
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s
    }
}

/// Build the option list for a permission kind.
///
/// Options per tool type (wording matches the "Yes, and don't ask again…"
/// phrasing used throughout the upstream permission dialogs):
/// - `Bash`: Yes (y) / Yes, and don't ask again (a) / No (n)
/// - `FileEdit`: Yes (y) / No (n)
/// - `FileWrite`: Yes (y) / No (n)
/// - `WebFetch`: Yes (y) / Yes, and don't ask again for `{domain}` (a) / No (n)
/// - `Generic`: Yes (y) / Yes, and don't ask again for `{tool_name}` (a) / No (n)
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
                    label: format!("Yes, and don't ask again for {domain}"),
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
                label: format!("Yes, and don't ask again for {tool_name}"),
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

/// Render a URL with structured display: scheme dimmed, domain bold, path muted.
fn render_parsed_url(url: &str, _width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(muted_color());
    let bold = Style::default()
        .fg(label_color())
        .add_modifier(Modifier::BOLD);

    let (scheme, rest) = if let Some(after) = url.strip_prefix("https://") {
        ("https://", after)
    } else if let Some(after) = url.strip_prefix("http://") {
        ("http://", after)
    } else {
        return vec![Line::from(vec![
            Span::styled("  ", dim),
            Span::styled(url.to_string(), Style::default().fg(body_color())),
        ])];
    };

    let (domain, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));

    vec![Line::from(vec![
        Span::styled("  ", dim),
        Span::styled(scheme.to_string(), dim),
        Span::styled(domain.to_string(), bold),
        Span::styled(path.to_string(), dim),
    ])]
}

/// Parse an MCP-style tool name (`mcp__server__tool`) into `(server, tool)`.
fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
    let stripped = name.strip_prefix("mcp__")?;
    let idx = stripped.find("__")?;
    let server = &stripped[..idx];
    let tool = &stripped[idx + 2..];
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

/// Map a bash command to a `(label, color)` risk badge by classifying it
/// with `crab_tools::builtin::bash_classifier::classify_command`.
///
/// Reflects both the command's category and its `is_destructive` flag so
/// that, for example, `rm -rf /tmp/x` shows as red "dangerous" even though
/// its base category is `FileWrite`.
fn classify_bash_risk(command: &str) -> (String, Color) {
    use crab_tools::builtin::bash_classifier::{CommandCategory, classify_command};

    let result = classify_command(command);
    if result.category == CommandCategory::Dangerous || result.is_destructive {
        return ("dangerous".to_string(), Color::Red);
    }
    match result.category {
        CommandCategory::ReadOnly => ("read-only".to_string(), Color::Green),
        CommandCategory::FileWrite => ("file-write".to_string(), Color::Yellow),
        CommandCategory::GitOperation => ("git".to_string(), Color::Yellow),
        CommandCategory::NetworkAccess => ("network".to_string(), Color::Yellow),
        CommandCategory::ProcessControl => ("process".to_string(), Color::Yellow),
        CommandCategory::PackageManager => ("package-mgr".to_string(), Color::Yellow),
        CommandCategory::Dangerous => ("dangerous".to_string(), Color::Red),
        CommandCategory::Unknown => ("unknown".to_string(), Color::DarkGray),
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
        let input = serde_json::json!({"command": "rm -rf /tmp/cache"});
        PermissionCard::from_event("bash", "rm -rf /tmp/cache", "req_1".into(), &input)
    }

    fn edit_card() -> PermissionCard {
        let input = serde_json::json!({
            "file_path": "src/main.rs",
            "old_string": "foo",
            "new_string": "bar",
        });
        PermissionCard::from_event("edit", "src/main.rs", "req_2".into(), &input)
    }

    fn generic_card() -> PermissionCard {
        PermissionCard::from_event(
            "mcp_tool",
            "some input data",
            "req_3".into(),
            &serde_json::Value::Null,
        )
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
        // Function keys are inert — they don't make a decision.
        assert_eq!(card.handle_key(KeyCode::F(1)), None);
        // Tab no longer returns None as a fallthrough; it now toggles
        // feedback mode (still without producing a response).
        assert!(!card.feedback_mode);
        assert_eq!(card.handle_key(KeyCode::Tab), None);
        assert!(card.feedback_mode);
    }

    // ── feedback-mode tests ──

    #[test]
    fn tab_enters_feedback_mode_without_decision() {
        let mut card = bash_card();
        assert!(!card.feedback_mode);
        let response = card.handle_key(KeyCode::Tab);
        assert_eq!(response, None);
        assert!(card.feedback_mode);
        assert!(card.feedback_text.is_empty());
    }

    #[test]
    fn typing_in_feedback_mode_buffers_chars() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Tab);
        for c in "use Read".chars() {
            assert_eq!(card.handle_key(KeyCode::Char(c)), None);
        }
        assert_eq!(card.feedback_text, "use Read");
        // Y/N/A do NOT short-circuit while in feedback mode — they're text.
        assert_eq!(card.handle_key(KeyCode::Char('y')), None);
        assert_eq!(card.feedback_text, "use Ready");
    }

    #[test]
    fn backspace_in_feedback_mode_pops_one() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Tab);
        for c in "abc".chars() {
            card.handle_key(KeyCode::Char(c));
        }
        assert_eq!(card.feedback_text, "abc");
        card.handle_key(KeyCode::Backspace);
        assert_eq!(card.feedback_text, "ab");
        // Pop past empty is a no-op, not a panic.
        for _ in 0..5 {
            card.handle_key(KeyCode::Backspace);
        }
        assert!(card.feedback_text.is_empty());
    }

    #[test]
    fn esc_in_feedback_mode_cancels_and_clears_buffer() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Tab);
        for c in "draft".chars() {
            card.handle_key(KeyCode::Char(c));
        }
        let response = card.handle_key(KeyCode::Esc);
        assert_eq!(response, None);
        assert!(!card.feedback_mode);
        assert!(card.feedback_text.is_empty());
    }

    #[test]
    fn enter_in_feedback_mode_submits_deny_with_feedback() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Tab);
        for c in "use Read instead".chars() {
            card.handle_key(KeyCode::Char(c));
        }
        let response = card.handle_key(KeyCode::Enter);
        assert_eq!(
            response,
            Some(PermissionResponse::DenyWithFeedback(
                "use Read instead".into()
            ))
        );
        assert!(!card.feedback_mode);
        assert!(card.feedback_text.is_empty());
    }

    #[test]
    fn enter_in_feedback_mode_with_empty_text_is_plain_deny() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Tab);
        let response = card.handle_key(KeyCode::Enter);
        assert_eq!(response, Some(PermissionResponse::Deny));
        assert!(!card.feedback_mode);
    }

    #[test]
    fn render_lines_in_feedback_mode_shows_input_prompt() {
        let mut card = bash_card();
        card.handle_key(KeyCode::Tab);
        for c in "tighten the diff".chars() {
            card.handle_key(KeyCode::Char(c));
        }
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("Feedback: "));
        assert!(all_text.contains("tighten the diff"));
        assert!(all_text.contains("Enter to deny with feedback"));
        // Decision-mode hint must be gone.
        assert!(!all_text.contains("Yes, and don't ask again"));
    }

    #[test]
    fn render_lines_decision_mode_hints_at_tab_for_feedback() {
        let card = bash_card();
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("Tab to add feedback"));
    }

    #[test]
    fn permission_response_helpers_are_consistent() {
        assert!(PermissionResponse::Allow.is_allow());
        assert!(PermissionResponse::AllowAlways.is_allow());
        assert!(!PermissionResponse::Deny.is_allow());
        assert!(!PermissionResponse::DenyWithFeedback("x".into()).is_allow());

        assert_eq!(PermissionResponse::Allow.feedback(), None);
        assert_eq!(PermissionResponse::Deny.feedback(), None);
        assert_eq!(
            PermissionResponse::DenyWithFeedback("note".into()).feedback(),
            Some("note")
        );
    }

    #[test]
    fn write_card_uses_overwrite_title() {
        let input = serde_json::json!({"file_path": "output.txt"});
        let card = PermissionCard::from_event("write", "output.txt", "req_w".into(), &input);
        // File doesn't exist on disk → "Create file"
        assert_eq!(card.kind.title(), "Create file");
    }

    #[test]
    fn web_fetch_detection() {
        let input = serde_json::json!({"url": "https://example.com/api"});
        let card = PermissionCard::from_event(
            "web_fetch",
            "https://example.com/api",
            "req_f".into(),
            &input,
        );
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
        let input = serde_json::json!({"file_path": "analysis.ipynb"});
        let card =
            PermissionCard::from_event("notebook_edit", "analysis.ipynb", "req_n".into(), &input);
        assert!(matches!(card.kind, PermissionKind::NotebookEdit { .. }));
        assert_eq!(card.kind.title(), "Edit notebook");
    }

    // ── Phase 1: Enhanced rendering tests ──

    #[test]
    fn bash_multiline_command_shows_dollar_prefix() {
        let cmd = "echo hello\necho world\necho done";
        let input = serde_json::json!({"command": cmd});
        let card = PermissionCard::from_event("bash", cmd, "req_ml".into(), &input);
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("$ "));
        assert!(all_text.contains("echo hello"));
    }

    #[test]
    fn bash_long_command_truncated() {
        let cmd = (0..10)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let input = serde_json::json!({"command": cmd});
        let card = PermissionCard::from_event("bash", &cmd, "req_trunc".into(), &input);
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("5 more lines"));
    }

    #[test]
    fn web_fetch_url_parsed_display() {
        let url = "https://api.example.com/v1/data?q=test";
        let input = serde_json::json!({"url": url});
        let card = PermissionCard::from_event("web_fetch", url, "req_url".into(), &input);
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("api.example.com"));
        assert!(all_text.contains("https://"));
    }

    #[test]
    fn mcp_tool_name_parsed() {
        assert_eq!(
            parse_mcp_tool_name("mcp__github__list_repos"),
            Some(("github".to_string(), "list_repos".to_string()))
        );
        assert_eq!(parse_mcp_tool_name("regular_tool"), None);
        assert_eq!(parse_mcp_tool_name("mcp____"), None);
        assert_eq!(parse_mcp_tool_name("mcp__server__"), None);
    }

    #[test]
    fn mcp_generic_card_renders_server_tool_format() {
        let card = PermissionCard::from_event(
            "mcp__myserver__do_thing",
            "some input",
            "req_mcp".into(),
            &serde_json::Value::Null,
        );
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("myserver"));
        assert!(all_text.contains("do_thing"));
    }

    #[test]
    fn file_edit_with_diff_shows_colored_lines() {
        let kind = PermissionKind::FileEdit {
            path: "src/main.rs".to_string(),
            diff: Some("-old line\n+new line\n context".to_string()),
        };
        let options = build_options(&kind);
        let card = PermissionCard {
            kind,
            request_id: "req_diff".into(),
            options,
            selected: 0,
            feedback_mode: false,
            feedback_text: String::new(),
        };
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("-old line"));
        assert!(all_text.contains("+new line"));
    }

    #[test]
    fn always_allow_labels_use_yes_and_form() {
        // All "always allow" option labels must use the canonical
        // "Yes, and don't ask again…" phrasing so the wording is
        // consistent across Bash / WebFetch / Generic cards.
        let bash = bash_card();
        let bash_always = &bash.options[1].label;
        assert!(
            bash_always.starts_with("Yes, and don't ask again"),
            "bash always-allow label: {bash_always}"
        );

        let web_input = serde_json::json!({"url": "https://api.example.com"});
        let web = PermissionCard::from_event(
            "web_fetch",
            "https://api.example.com",
            "r".into(),
            &web_input,
        );
        let web_always = &web.options[1].label;
        assert_eq!(
            web_always, "Yes, and don't ask again for api.example.com",
            "web-fetch always-allow label should include domain scope"
        );

        let generic = generic_card();
        let generic_always = &generic.options[1].label;
        assert_eq!(
            generic_always, "Yes, and don't ask again for mcp_tool",
            "generic always-allow label should include tool name scope"
        );
    }

    #[test]
    fn render_parsed_url_no_scheme_fallback() {
        let lines = render_parsed_url("just-a-hostname", 80);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("just-a-hostname"));
    }

    // ── tool_input wiring tests ──

    #[test]
    fn compute_edit_diff_generates_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();
        let path_str = path.to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "old_string": "beta",
            "new_string": "BETA",
        });
        let diff = compute_edit_diff(&input, &path_str).expect("diff should be produced");
        assert!(diff.contains("-beta"), "expected '-beta' in diff: {diff}");
        assert!(diff.contains("+BETA"), "expected '+BETA' in diff: {diff}");
    }

    #[test]
    fn compute_edit_diff_returns_none_for_missing_file() {
        let input = serde_json::json!({
            "old_string": "x",
            "new_string": "y",
        });
        let diff = compute_edit_diff(&input, "/nonexistent/path/does/not/exist.txt");
        assert!(diff.is_none());
    }

    #[test]
    fn compute_edit_diff_returns_none_when_old_string_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello\n").unwrap();
        let path_str = path.to_str().unwrap().to_string();
        let input = serde_json::json!({
            "old_string": "missing",
            "new_string": "replacement",
        });
        assert!(compute_edit_diff(&input, &path_str).is_none());
    }

    #[test]
    fn classify_with_tool_input_extracts_bash_command() {
        let input = serde_json::json!({
            "command": "echo from-json",
            "description": "say hi",
        });
        let kind = classify_permission_kind("bash", "fallback summary", &input);
        let PermissionKind::Bash {
            command,
            description,
            ..
        } = kind
        else {
            panic!("expected Bash kind");
        };
        assert_eq!(command, "echo from-json");
        assert_eq!(description.as_deref(), Some("say hi"));
    }

    #[test]
    fn bash_kind_carries_risk_badge() {
        let input = serde_json::json!({"command": "ls -la"});
        let kind = classify_permission_kind("bash", "ls -la", &input);
        let PermissionKind::Bash { risk_badge, .. } = kind else {
            panic!("expected Bash kind");
        };
        let (label, color) = risk_badge.expect("ls should produce a badge");
        assert_eq!(label, "read-only");
        assert_eq!(color, Color::Green);
    }

    #[test]
    fn bash_dangerous_command_gets_red_badge() {
        let input = serde_json::json!({"command": "rm -rf /"});
        let kind = classify_permission_kind("bash", "rm -rf /", &input);
        let PermissionKind::Bash { risk_badge, .. } = kind else {
            panic!("expected Bash kind");
        };
        let (label, color) = risk_badge.expect("dangerous cmd should produce a badge");
        assert_eq!(label, "dangerous");
        assert_eq!(color, Color::Red);
    }

    #[test]
    fn bash_destructive_rm_gets_dangerous_label_even_when_filewrite() {
        // `rm file.txt` is FileWrite category but is_destructive=true,
        // so the badge should escalate to "dangerous".
        let input = serde_json::json!({"command": "rm file.txt"});
        let kind = classify_permission_kind("bash", "rm file.txt", &input);
        let PermissionKind::Bash { risk_badge, .. } = kind else {
            panic!("expected Bash kind");
        };
        let (label, color) = risk_badge.expect("rm should produce a badge");
        assert_eq!(label, "dangerous");
        assert_eq!(color, Color::Red);
    }

    #[test]
    fn bash_render_includes_badge_label() {
        let input = serde_json::json!({"command": "git status", "description": "list changes"});
        let card = PermissionCard::from_event("bash", "git status", "req_badge".into(), &input);
        let lines = card.render_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            all_text.contains("[read-only]"),
            "expected badge in: {all_text}"
        );
        assert!(all_text.contains("list changes"));
    }

    #[test]
    fn classify_with_tool_input_extracts_edit_path() {
        let input = serde_json::json!({"file_path": "/tmp/some/file.rs"});
        let kind = classify_permission_kind("edit", "fallback", &input);
        let PermissionKind::FileEdit { path, .. } = kind else {
            panic!("expected FileEdit kind");
        };
        assert_eq!(path, "/tmp/some/file.rs");
    }

    #[test]
    fn classify_with_tool_input_detects_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        std::fs::write(&path, "content").unwrap();
        let path_str = path.to_str().unwrap().to_string();
        let input = serde_json::json!({"file_path": path_str});
        let kind = classify_permission_kind("write", "fallback", &input);
        let PermissionKind::FileWrite { file_exists, .. } = kind else {
            panic!("expected FileWrite kind");
        };
        assert!(file_exists, "file_exists should be true for existing path");
    }

    #[test]
    fn compute_write_diff_generates_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.txt");
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let path_str = path.to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "content": "line1\nLINE2\nline3\n",
        });
        let diff = compute_write_diff(&input, &path_str).expect("diff should be produced");
        assert!(diff.contains("-line2"), "expected '-line2' in diff: {diff}");
        assert!(diff.contains("+LINE2"), "expected '+LINE2' in diff: {diff}");
    }

    #[test]
    fn compute_write_diff_returns_none_for_new_file() {
        let input = serde_json::json!({
            "content": "brand new content",
        });
        assert!(compute_write_diff(&input, "/nonexistent/path.txt").is_none());
    }

    #[test]
    fn compute_write_diff_returns_none_when_content_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.txt");
        std::fs::write(&path, "unchanged\n").unwrap();
        let path_str = path.to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "content": "unchanged\n",
        });
        assert!(compute_write_diff(&input, &path_str).is_none());
    }

    #[test]
    fn classify_write_with_content_produces_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overwrite.txt");
        std::fs::write(&path, "old\n").unwrap();
        let path_str = path.to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "content": "new\n",
        });
        let kind = classify_permission_kind("write", "fallback", &input);
        let PermissionKind::FileWrite {
            file_exists, diff, ..
        } = kind
        else {
            panic!("expected FileWrite kind");
        };
        assert!(file_exists);
        assert!(diff.is_some(), "overwrite should produce a diff");
    }

    #[test]
    fn classify_write_new_file_has_content_preview() {
        let input = serde_json::json!({
            "file_path": "/nonexistent/new_file.rs",
            "content": "fn main() {\n    println!(\"hello\");\n}\n",
        });
        let kind = classify_permission_kind("write", "fallback", &input);
        let PermissionKind::FileWrite {
            file_exists,
            diff,
            content_preview,
            ..
        } = kind
        else {
            panic!("expected FileWrite kind");
        };
        assert!(!file_exists);
        assert!(diff.is_none(), "new file should have no diff");
        let preview = content_preview.expect("new file should have content preview");
        assert!(preview.contains("fn main()"));
        assert!(preview.contains("println!"));
    }

    #[test]
    fn classify_write_new_file_preview_capped_at_10_lines() {
        let long_content: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let input = serde_json::json!({
            "file_path": "/nonexistent/long.txt",
            "content": long_content,
        });
        let kind = classify_permission_kind("write", "fallback", &input);
        let PermissionKind::FileWrite {
            content_preview, ..
        } = kind
        else {
            panic!("expected FileWrite kind");
        };
        let preview = content_preview.expect("should have preview");
        let line_count = preview.lines().count();
        assert_eq!(
            line_count, 10,
            "preview should be capped at 10 lines, got {line_count}"
        );
    }

    #[test]
    fn classify_with_tool_input_extracts_fetch_url() {
        let input = serde_json::json!({"url": "https://structured.example.com/api"});
        let kind = classify_permission_kind("web_fetch", "fallback", &input);
        let PermissionKind::WebFetch { url } = kind else {
            panic!("expected WebFetch kind");
        };
        assert_eq!(url, "https://structured.example.com/api");
    }

    #[test]
    fn truncate_tool_input_null_falls_back() {
        let result = truncate_tool_input(&serde_json::Value::Null, "fallback summary");
        assert_eq!(result, "fallback summary");
    }

    #[test]
    fn truncate_tool_input_empty_object_falls_back() {
        let empty = serde_json::json!({});
        let result = truncate_tool_input(&empty, "fallback");
        assert_eq!(result, "fallback");
    }

    #[test]
    fn truncate_tool_input_long_value_truncated() {
        let big_str: String = "x".repeat(500);
        let input = serde_json::json!({"data": big_str});
        let result = truncate_tool_input(&input, "fallback");
        assert!(result.ends_with('…'), "long input should end with ellipsis");
        // The byte length of the prefix is 200; the ellipsis adds 3 more bytes (UTF-8).
        assert_eq!(result.len(), 200 + '…'.len_utf8());
    }
}
