//! MCP server/tool browser overlay — three-level drill-down navigation.
//!
//! `ServerList` → `ToolList` → `ToolDetail`.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub tool_count: usize,
    pub tools: Vec<McpToolInfo>,
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    ServerList,
    ToolList,
    ToolDetail,
}

// ---------------------------------------------------------------------------
// McpBrowserOverlay
// ---------------------------------------------------------------------------

pub struct McpBrowserOverlay {
    servers: Vec<McpServerInfo>,
    selected_server: usize,
    selected_tool: usize,
    detail_scroll: usize,
    view: View,
}

impl McpBrowserOverlay {
    #[must_use]
    pub fn new(servers: Vec<McpServerInfo>) -> Self {
        Self {
            servers,
            selected_server: 0,
            selected_tool: 0,
            detail_scroll: 0,
            view: View::ServerList,
        }
    }

    fn current_server(&self) -> Option<&McpServerInfo> {
        self.servers.get(self.selected_server)
    }

    fn current_tool(&self) -> Option<&McpToolInfo> {
        self.current_server()
            .and_then(|s| s.tools.get(self.selected_tool))
    }

    fn enter_tool_list(&mut self) {
        if self.current_server().is_some() {
            self.view = View::ToolList;
            self.selected_tool = 0;
        }
    }

    fn enter_tool_detail(&mut self) {
        if self.current_tool().is_some() {
            self.view = View::ToolDetail;
            self.detail_scroll = 0;
        }
    }

    fn go_back(&mut self) -> OverlayAction {
        match self.view {
            View::ServerList => OverlayAction::Dismiss,
            View::ToolList => {
                self.view = View::ServerList;
                OverlayAction::Consumed
            }
            View::ToolDetail => {
                self.view = View::ToolList;
                OverlayAction::Consumed
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl Renderable for McpBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 20 {
            return;
        }

        Widget::render(Clear, area, buf);

        let title = match self.view {
            View::ServerList => format!(
                " MCP Servers ({}) ",
                self.servers.len()
            ),
            View::ToolList => {
                let name = self
                    .current_server()
                    .map_or("???", |s| s.name.as_str());
                format!(" {name} — Tools ")
            }
            View::ToolDetail => {
                let name = self
                    .current_tool()
                    .map_or("???", |t| t.name.as_str());
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
            View::ServerList => self.render_server_list(inner, buf),
            View::ToolList => self.render_tool_list(inner, buf),
            View::ToolDetail => self.render_tool_detail(inner, buf),
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0 // fullscreen
    }
}

impl McpBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render_server_list(&self, area: Rect, buf: &mut Buffer) {
        if self.servers.is_empty() {
            let msg = Line::from(Span::styled(
                "  No MCP servers configured.",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(
                msg,
                Rect { x: area.x, y: area.y, width: area.width, height: 1 },
                buf,
            );
            return;
        }

        for (i, server) in self.servers.iter().enumerate() {
            if i as u16 >= area.height.saturating_sub(1) {
                break;
            }
            let is_selected = i == self.selected_server;
            let prefix = if is_selected { "▸ " } else { "  " };
            let name_style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let line = Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(server.name.clone(), name_style),
                Span::styled(
                    format!(" ({} tools)", server.tool_count),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            Widget::render(
                line,
                Rect { x: area.x, y: area.y + i as u16, width: area.width, height: 1 },
                buf,
            );
        }

        render_footer(area, buf, " ↑↓/jk navigate  Enter view  q/Esc close");
    }

    #[allow(clippy::cast_possible_truncation)]
    fn render_tool_list(&self, area: Rect, buf: &mut Buffer) {
        let Some(server) = self.current_server() else {
            return;
        };

        if server.tools.is_empty() {
            let msg = Line::from(Span::styled(
                "  No tools available.",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(
                msg,
                Rect { x: area.x, y: area.y, width: area.width, height: 1 },
                buf,
            );
            return;
        }

        let max_name_len = server.tools.iter().map(|t| t.name.len()).max().unwrap_or(0);

        for (i, tool) in server.tools.iter().enumerate() {
            if i as u16 >= area.height.saturating_sub(1) {
                break;
            }
            let is_selected = i == self.selected_tool;
            let prefix = if is_selected { "▸ " } else { "  " };
            let name_style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let padded_name = format!("{:<width$}", tool.name, width = max_name_len);
            let desc_width = (area.width as usize)
                .saturating_sub(prefix.len() + max_name_len + 5);
            let desc: String = tool
                .description
                .chars()
                .take(desc_width)
                .collect();

            let line = Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(padded_name, name_style),
                Span::styled(" — ", Style::default().fg(Color::DarkGray)),
                Span::styled(desc, Style::default().fg(Color::Gray)),
            ]);
            Widget::render(
                line,
                Rect { x: area.x, y: area.y + i as u16, width: area.width, height: 1 },
                buf,
            );
        }

        render_footer(area, buf, " ↑↓/jk navigate  Enter detail  Backspace/Esc back");
    }

    #[allow(clippy::cast_possible_truncation)]
    fn render_tool_detail(&self, area: Rect, buf: &mut Buffer) {
        let Some(tool) = self.current_tool() else {
            return;
        };

        let mut lines: Vec<Line<'static>> = Vec::new();

        lines.push(Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                tool.name.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(Line::default());

        lines.push(Line::from(Span::styled(
            "Description:",
            Style::default().fg(Color::DarkGray),
        )));
        for desc_line in tool.description.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {desc_line}"),
                Style::default().fg(Color::White),
            )));
        }

        lines.push(Line::default());

        lines.push(Line::from(Span::styled(
            "Input Schema:",
            Style::default().fg(Color::DarkGray),
        )));
        let schema_str = serde_json::to_string_pretty(&tool.input_schema)
            .unwrap_or_else(|_| tool.input_schema.to_string());
        for schema_line in schema_str.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {schema_line}"),
                Style::default().fg(Color::Gray),
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
                Rect { x: area.x, y: area.y + row as u16, width: area.width, height: 1 },
                buf,
            );
        }

        render_footer(area, buf, " ↑↓/jk scroll  Backspace/Esc back  q close");
    }
}

#[allow(clippy::cast_possible_truncation)]
fn render_footer(area: Rect, buf: &mut Buffer, text: &str) {
    if area.height > 1 {
        let hint = Line::from(Span::styled(
            text,
            Style::default().fg(Color::DarkGray),
        ));
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

impl Overlay for McpBrowserOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match self.view {
            View::ServerList => self.handle_server_list_key(key),
            View::ToolList => self.handle_tool_list_key(key),
            View::ToolDetail => self.handle_tool_detail_key(key),
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::ScrollBox]
    }

    fn name(&self) -> &'static str {
        "mcp_browser"
    }
}

impl McpBrowserOverlay {
    fn handle_server_list_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_server = self.selected_server.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.servers.is_empty() {
                    self.selected_server =
                        (self.selected_server + 1).min(self.servers.len() - 1);
                }
                OverlayAction::Consumed
            }
            KeyCode::Enter => {
                self.enter_tool_list();
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn handle_tool_list_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => self.go_back(),
            KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_tool = self.selected_tool.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = self.current_server().map_or(0, |s| s.tools.len());
                if len > 0 {
                    self.selected_tool = (self.selected_tool + 1).min(len - 1);
                }
                OverlayAction::Consumed
            }
            KeyCode::Enter => {
                self.enter_tool_detail();
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn handle_tool_detail_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => self.go_back(),
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

    fn sample_servers() -> Vec<McpServerInfo> {
        vec![
            McpServerInfo {
                name: "filesystem".into(),
                tool_count: 2,
                tools: vec![
                    McpToolInfo {
                        name: "read_file".into(),
                        description: "Read a file from disk".into(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" }
                            }
                        }),
                    },
                    McpToolInfo {
                        name: "write_file".into(),
                        description: "Write content to a file".into(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "content": { "type": "string" }
                            }
                        }),
                    },
                ],
            },
            McpServerInfo {
                name: "github".into(),
                tool_count: 1,
                tools: vec![McpToolInfo {
                    name: "create_pr".into(),
                    description: "Create a pull request".into(),
                    input_schema: serde_json::json!({}),
                }],
            },
        ]
    }

    // --- Navigation state ---

    #[test]
    fn starts_in_server_list() {
        let ov = McpBrowserOverlay::new(sample_servers());
        assert_eq!(ov.view, View::ServerList);
    }

    #[test]
    fn enter_drills_into_tool_list() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::ToolList);
    }

    #[test]
    fn enter_from_tool_list_drills_into_detail() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter)); // server list → tool list
        ov.handle_key(key(KeyCode::Enter)); // tool list → tool detail
        assert_eq!(ov.view, View::ToolDetail);
    }

    #[test]
    fn esc_from_detail_returns_to_tool_list() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::ToolDetail);
        ov.handle_key(key(KeyCode::Esc));
        assert_eq!(ov.view, View::ToolList);
    }

    #[test]
    fn esc_from_tool_list_returns_to_server_list() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::ToolList);
        ov.handle_key(key(KeyCode::Esc));
        assert_eq!(ov.view, View::ServerList);
    }

    #[test]
    fn esc_from_server_list_dismisses() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        assert!(matches!(
            ov.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn q_always_dismisses() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter)); // tool list
        assert!(matches!(
            ov.handle_key(key(KeyCode::Char('q'))),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn backspace_goes_back() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Backspace));
        assert_eq!(ov.view, View::ServerList);
    }

    // --- Server navigation ---

    #[test]
    fn navigate_servers() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected_server, 1);
        ov.handle_key(key(KeyCode::Down)); // clamped
        assert_eq!(ov.selected_server, 1);
        ov.handle_key(key(KeyCode::Up));
        assert_eq!(ov.selected_server, 0);
    }

    // --- Tool navigation ---

    #[test]
    fn navigate_tools() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter)); // tool list for filesystem (2 tools)
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected_tool, 1);
        ov.handle_key(key(KeyCode::Down)); // clamped
        assert_eq!(ov.selected_tool, 1);
        ov.handle_key(key(KeyCode::Up));
        assert_eq!(ov.selected_tool, 0);
    }

    // --- Detail scroll ---

    #[test]
    fn detail_scroll() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Char('j')));
        assert_eq!(ov.detail_scroll, 1);
        ov.handle_key(key(KeyCode::Char('k')));
        assert_eq!(ov.detail_scroll, 0);
    }

    // --- Empty servers ---

    #[test]
    fn empty_servers_no_panic() {
        let mut ov = McpBrowserOverlay::new(vec![]);
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::ServerList); // stays
    }

    // --- Render ---

    #[test]
    fn render_server_list_no_panic() {
        let ov = McpBrowserOverlay::new(sample_servers());
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_tool_list_no_panic() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_tool_detail_no_panic() {
        let mut ov = McpBrowserOverlay::new(sample_servers());
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Enter));
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_empty_no_panic() {
        let ov = McpBrowserOverlay::new(vec![]);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_tiny_area_no_panic() {
        let ov = McpBrowserOverlay::new(sample_servers());
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn overlay_name() {
        let ov = McpBrowserOverlay::new(vec![]);
        assert_eq!(ov.name(), "mcp_browser");
    }

    #[test]
    fn overlay_context() {
        let ov = McpBrowserOverlay::new(vec![]);
        assert_eq!(ov.contexts(), vec![KeyContext::ScrollBox]);
    }
}
