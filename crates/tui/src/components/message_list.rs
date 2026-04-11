//! Message list component — renders structured conversation messages
//! with scroll support.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::components::text_utils::strip_trailing_tool_json;
use crate::traits::Renderable;

/// Terra cotta color.
const CRAB_COLOR: Color = Color::Rgb(218, 119, 86);

/// Renders the structured message list with scroll offset.
pub struct MessageList<'a> {
    pub messages: &'a [ChatMessage],
    pub scroll_offset: usize,
}

impl Renderable for MessageList<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        render_messages(self.messages, self.scroll_offset, area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // Flex item — takes all available space
        0
    }
}

/// Render structured messages list — each `ChatMessage` gets its own visual treatment.
#[allow(clippy::cast_possible_truncation)]
pub fn render_messages(
    messages: &[ChatMessage],
    scroll_offset: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 {
        return;
    }

    let theme = crate::theme::Theme::dark();
    let highlighter = crate::components::syntax::SyntaxHighlighter::new();
    let md_renderer = crate::components::markdown::MarkdownRenderer::new(&theme, &highlighter);

    let mut rendered_lines: Vec<Line<'static>> = Vec::new();

    for msg in messages {
        match msg {
            ChatMessage::User { text } => {
                rendered_lines.push(Line::from(vec![
                    Span::styled(
                        "❯ ",
                        Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text.clone(), Style::default().fg(Color::White)),
                ]));
                rendered_lines.push(Line::default());
            }
            ChatMessage::Assistant { text } => {
                if text.is_empty() {
                    continue;
                }
                let clean_text = strip_trailing_tool_json(text);
                if clean_text.is_empty() {
                    continue;
                }
                let md_lines = md_renderer.render(&clean_text);
                if let Some(first) = md_lines.first() {
                    let mut spans = vec![Span::styled("● ", Style::default().fg(CRAB_COLOR))];
                    spans.extend(first.spans.iter().cloned());
                    rendered_lines.push(Line::from(spans));
                    rendered_lines.extend(md_lines.into_iter().skip(1));
                }
                rendered_lines.push(Line::default());
            }
            ChatMessage::ToolUse { name, summary } => {
                let label = summary.as_deref().unwrap_or(name);
                rendered_lines.push(Line::from(Span::styled(
                    format!("● {label}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            ChatMessage::ToolResult {
                output,
                is_error,
                display,
                ..
            } => {
                if let Some(display) = display {
                    // Use tool-customized rendering
                    use crab_core::tool::ToolDisplayStyle as DS;
                    for dl in &display.lines {
                        let style = match dl.style {
                            Some(DS::Error | DS::DiffRemove) => Style::default().fg(Color::Red),
                            Some(DS::DiffAdd) => Style::default().fg(Color::Green),
                            Some(DS::DiffContext | DS::Muted) => {
                                Style::default().fg(Color::DarkGray)
                            }
                            Some(DS::Highlight) => Style::default().fg(Color::Cyan),
                            _ => Style::default().fg(Color::Gray),
                        };
                        rendered_lines
                            .push(Line::from(Span::styled(format!("  {}", dl.text), style)));
                    }
                } else {
                    // Default rendering: plain text, 10-line truncation
                    let style = if *is_error {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let lines: Vec<&str> = output.lines().collect();
                    let show = lines.len().min(10);
                    for line in &lines[..show] {
                        rendered_lines.push(Line::from(Span::styled(format!("  {line}"), style)));
                    }
                    if lines.len() > 10 {
                        rendered_lines.push(Line::from(Span::styled(
                            format!("  ... ({} more lines)", lines.len() - 10),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                rendered_lines.push(Line::default());
            }
            ChatMessage::System { text } => {
                rendered_lines.push(Line::from(Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }

    // Scroll and render visible lines (auto-follow bottom)
    let visible = area.height as usize;
    let end = rendered_lines.len().saturating_sub(scroll_offset);
    let start = end.saturating_sub(visible);

    for (i, line) in rendered_lines
        .iter()
        .skip(start)
        .take(visible.min(end.saturating_sub(start)))
        .enumerate()
    {
        let y = area.y + i as u16;
        Widget::render(
            line.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_list_desired_height() {
        let ml = MessageList {
            messages: &[],
            scroll_offset: 0,
        };
        assert_eq!(ml.desired_height(80), 0);
    }

    #[test]
    fn render_empty_messages() {
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        render_messages(&[], 0, area, &mut buf);
    }

    #[test]
    fn render_user_message() {
        let msgs = vec![ChatMessage::User {
            text: "hello".into(),
        }];
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        render_messages(&msgs, 0, area, &mut buf);
    }
}
