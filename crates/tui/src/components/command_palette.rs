//! Command palette — modal popup for discovering and executing commands,
//! similar to VS Code's Ctrl+Shift+P.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// Category for grouping commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    File,
    Edit,
    View,
    Navigation,
    Tools,
    Session,
    Help,
}

impl std::fmt::Display for CommandCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File => write!(f, "File"),
            Self::Edit => write!(f, "Edit"),
            Self::View => write!(f, "View"),
            Self::Navigation => write!(f, "Navigation"),
            Self::Tools => write!(f, "Tools"),
            Self::Session => write!(f, "Session"),
            Self::Help => write!(f, "Help"),
        }
    }
}

/// A command that can be executed from the palette.
#[derive(Debug, Clone)]
pub struct Command {
    /// Unique identifier.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional description shown below the label.
    pub description: Option<String>,
    /// Optional shortcut display string (e.g. "Ctrl+N").
    pub shortcut: Option<String>,
    /// Category for grouping.
    pub category: CommandCategory,
}

impl Command {
    /// Create a new command.
    pub fn new(id: impl Into<String>, label: impl Into<String>, category: CommandCategory) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shortcut: None,
            category,
        }
    }

    /// Set description.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set shortcut display.
    #[must_use]
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

// ─── CommandRegistry ────────────────────────────────────────────────────

/// Registry of all available commands.
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Register a command.
    pub fn register(&mut self, command: Command) {
        self.commands.push(command);
    }

    /// Number of registered commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Get all commands.
    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Find a command by id.
    #[must_use]
    pub fn find_by_id(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }

    /// Get commands in a category.
    #[must_use]
    pub fn by_category(&self, category: CommandCategory) -> Vec<&Command> {
        self.commands
            .iter()
            .filter(|c| c.category == category)
            .collect()
    }

    /// Fuzzy search commands by query.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&Command> {
        if query.is_empty() {
            return self.commands.iter().collect();
        }
        let query_lower = query.to_lowercase();
        let mut results: Vec<(&Command, i32)> = self
            .commands
            .iter()
            .filter_map(|cmd| {
                let score = fuzzy_score(&cmd.label, &query_lower)
                    .or_else(|| {
                        cmd.description
                            .as_deref()
                            .and_then(|d| fuzzy_score(d, &query_lower))
                    })
                    .or_else(|| fuzzy_score(&cmd.category.to_string(), &query_lower));
                score.map(|s| (cmd, s))
            })
            .collect();
        results.sort_by(|a, b| b.1.cmp(&a.1));
        results.into_iter().map(|(cmd, _)| cmd).collect()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple fuzzy matching: returns Some(score) if all query chars appear in order.
fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    let hay_lower = haystack.to_lowercase();
    let mut score = 0i32;
    let mut hay_iter = hay_lower.chars().peekable();
    let mut last_match_idx = 0usize;
    let mut pos = 0usize;

    for needle_char in needle.chars() {
        let mut found = false;
        while let Some(&hay_char) = hay_iter.peek() {
            hay_iter.next();
            pos += 1;
            if hay_char == needle_char {
                // Consecutive match bonus
                if pos == last_match_idx + 1 {
                    score += 2;
                } else {
                    score += 1;
                }
                // Start-of-word bonus
                if pos <= 1 {
                    score += 3;
                }
                last_match_idx = pos;
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    Some(score)
}

// ─── CommandPalette state ───────────────────────────────────────────────

/// Result of processing a key in the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    /// User selected a command (by id).
    Execute(String),
    /// Palette was dismissed.
    Dismiss,
    /// Key was consumed (navigation/typing).
    Consumed,
}

/// The command palette modal state.
pub struct CommandPalette {
    /// Whether the palette is visible.
    visible: bool,
    /// Current search query.
    query: String,
    /// Filtered command indices.
    filtered: Vec<usize>,
    /// Selected index within filtered results.
    selected: usize,
    /// The registry of all commands.
    registry: CommandRegistry,
}

impl CommandPalette {
    /// Create a new palette with a registry.
    #[must_use]
    pub fn new(registry: CommandRegistry) -> Self {
        let filtered: Vec<usize> = (0..registry.len()).collect();
        Self {
            visible: false,
            query: String::new(),
            filtered,
            selected: 0,
            registry,
        }
    }

    /// Show the palette.
    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.update_filter();
    }

    /// Hide the palette.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Whether the palette is visible.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Current query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Number of filtered results.
    #[must_use]
    pub fn result_count(&self) -> usize {
        self.filtered.len()
    }

    /// Current selection index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Get the currently selected command.
    #[must_use]
    pub fn selected_command(&self) -> Option<&Command> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.registry.commands.get(i))
    }

    /// Type a character into the query.
    pub fn type_char(&mut self, c: char) {
        self.query.push(c);
        self.update_filter();
    }

    /// Delete the last character from the query.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.update_filter();
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = if self.selected == 0 {
                self.filtered.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// Confirm selection. Returns the command id if something is selected.
    #[must_use]
    pub fn confirm(&self) -> Option<String> {
        self.selected_command().map(|cmd| cmd.id.clone())
    }

    fn update_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.registry.len()).collect();
        } else {
            let results = self.registry.search(&self.query);
            self.filtered = results
                .iter()
                .filter_map(|cmd| {
                    self.registry
                        .commands
                        .iter()
                        .position(|c| std::ptr::eq(c, *cmd))
                })
                .collect();
        }
        if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
    }

    /// Get the visible (filtered) commands for rendering.
    #[must_use]
    pub fn visible_commands(&self) -> Vec<&Command> {
        self.filtered
            .iter()
            .filter_map(|&i| self.registry.commands.get(i))
            .collect()
    }
}

// ─── Widget rendering ───────────────────────────────────────────────────

/// Renders the command palette as a centered modal.
pub struct CommandPaletteWidget<'a> {
    palette: &'a CommandPalette,
    theme: &'a Theme,
}

impl<'a> CommandPaletteWidget<'a> {
    #[must_use]
    pub fn new(palette: &'a CommandPalette, theme: &'a Theme) -> Self {
        Self { palette, theme }
    }
}

impl Widget for CommandPaletteWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.palette.is_visible() || area.width < 20 || area.height < 5 {
            return;
        }

        // Center the palette: 60% width, up to 15 rows
        let width = (area.width * 3 / 5).max(30).min(area.width - 2);
        let max_items = 10usize;
        let height = (max_items as u16 + 3).min(area.height - 2); // +3 for border+query+border
        let x = area.x + (area.width - width) / 2;
        let y = area.y + 1; // Near the top

        let border_style = Style::default().fg(self.theme.border);
        let query_style = Style::default().fg(self.theme.fg);
        let selected_style = Style::default()
            .fg(self.theme.fg)
            .bg(self.theme.border)
            .add_modifier(Modifier::BOLD);
        let normal_style = Style::default().fg(self.theme.fg);
        let shortcut_style = Style::default().fg(self.theme.muted);
        let desc_style = Style::default().fg(self.theme.muted);
        let category_style = Style::default().fg(self.theme.syntax_type);

        // Top border
        let top = format!("┌{}┐", "─".repeat(width.saturating_sub(2) as usize));
        let top_line = Line::from(Span::styled(top, border_style));
        let top_area = Rect::new(x, y, width, 1);
        Widget::render(top_line, top_area, buf);

        // Query line
        let query_text = format!(
            "│ > {:<width$}│",
            self.palette.query(),
            width = width.saturating_sub(6) as usize
        );
        let query_line = Line::from(Span::styled(query_text, query_style));
        let query_area = Rect::new(x, y + 1, width, 1);
        Widget::render(query_line, query_area, buf);

        // Separator
        let sep = format!("├{}┤", "─".repeat(width.saturating_sub(2) as usize));
        let sep_line = Line::from(Span::styled(sep, border_style));
        let sep_area = Rect::new(x, y + 2, width, 1);
        Widget::render(sep_line, sep_area, buf);

        // Command list
        let commands = self.palette.visible_commands();
        let visible_count = commands.len().min(max_items);

        for (i, cmd) in commands.iter().take(visible_count).enumerate() {
            let row_y = y + 3 + i as u16;
            if row_y >= y + height {
                break;
            }

            let is_selected = i == self.palette.selected();
            let style = if is_selected {
                selected_style
            } else {
                normal_style
            };

            let inner_width = width.saturating_sub(4) as usize;
            let mut spans = Vec::new();
            spans.push(Span::styled("│ ", border_style));

            // Category tag
            let cat_str = format!("[{}] ", cmd.category);
            let cat_len = cat_str.len();
            spans.push(Span::styled(cat_str, category_style));

            // Label
            let label_budget = inner_width.saturating_sub(cat_len);
            if let Some(ref shortcut) = cmd.shortcut {
                let sc_display = format!("  {shortcut}");
                let label_max = label_budget.saturating_sub(sc_display.len());
                let label = truncate(&cmd.label, label_max);
                let padding = label_budget.saturating_sub(label.len() + sc_display.len());
                spans.push(Span::styled(label, style));
                spans.push(Span::styled(" ".repeat(padding), style));
                spans.push(Span::styled(sc_display, shortcut_style));
            } else {
                let label = truncate(&cmd.label, label_budget);
                let padding = label_budget.saturating_sub(label.len());
                spans.push(Span::styled(label, style));
                spans.push(Span::styled(" ".repeat(padding), style));
            }

            spans.push(Span::styled(" │", border_style));

            let line = Line::from(spans);
            let line_area = Rect::new(x, row_y, width, 1);
            Widget::render(line, line_area, buf);

            // Description on next line if selected and has description
            if is_selected && let Some(ref desc) = cmd.description {
                let desc_y = row_y + 1;
                if desc_y < y + height {
                    let desc_text = truncate(desc, inner_width);
                    let desc_pad = inner_width.saturating_sub(desc_text.len());
                    let desc_line = Line::from(vec![
                        Span::styled("│ ", border_style),
                        Span::styled(format!("  {desc_text}"), desc_style),
                        Span::styled(" ".repeat(desc_pad), desc_style),
                        Span::styled(" │", border_style),
                    ]);
                    let desc_area = Rect::new(x, desc_y, width, 1);
                    Widget::render(desc_line, desc_area, buf);
                }
            }
        }

        // Bottom border
        let bottom_y = (y + 3 + visible_count as u16).min(y + height - 1);
        if bottom_y < area.y + area.height {
            let bottom = format!("└{}┘", "─".repeat(width.saturating_sub(2) as usize));
            let bottom_line = Line::from(Span::styled(bottom, border_style));
            let bottom_area = Rect::new(x, bottom_y, width, 1);
            Widget::render(bottom_line, bottom_area, buf);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 3 {
        format!("{}...", &s[..max - 3])
    } else {
        s[..max].to_string()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registry() -> CommandRegistry {
        let mut reg = CommandRegistry::new();
        reg.register(
            Command::new("new_file", "New File", CommandCategory::File)
                .with_shortcut("Ctrl+N")
                .with_description("Create a new file"),
        );
        reg.register(
            Command::new("open_file", "Open File", CommandCategory::File).with_shortcut("Ctrl+O"),
        );
        reg.register(Command::new("undo", "Undo", CommandCategory::Edit).with_shortcut("Ctrl+Z"));
        reg.register(Command::new("redo", "Redo", CommandCategory::Edit).with_shortcut("Ctrl+Y"));
        reg.register(Command::new(
            "toggle_sidebar",
            "Toggle Sidebar",
            CommandCategory::View,
        ));
        reg.register(
            Command::new("goto_line", "Go to Line", CommandCategory::Navigation)
                .with_shortcut("Ctrl+G"),
        );
        reg.register(Command::new("run_tool", "Run Tool", CommandCategory::Tools));
        reg.register(
            Command::new("new_session", "New Session", CommandCategory::Session)
                .with_shortcut("Ctrl+Shift+N"),
        );
        reg.register(Command::new(
            "show_help",
            "Show Help",
            CommandCategory::Help,
        ));
        reg
    }

    // ── CommandCategory ──

    #[test]
    fn category_display() {
        assert_eq!(CommandCategory::File.to_string(), "File");
        assert_eq!(CommandCategory::Edit.to_string(), "Edit");
        assert_eq!(CommandCategory::View.to_string(), "View");
        assert_eq!(CommandCategory::Navigation.to_string(), "Navigation");
        assert_eq!(CommandCategory::Tools.to_string(), "Tools");
        assert_eq!(CommandCategory::Session.to_string(), "Session");
        assert_eq!(CommandCategory::Help.to_string(), "Help");
    }

    // ── Command ──

    #[test]
    fn command_new() {
        let cmd = Command::new("test", "Test Command", CommandCategory::Edit);
        assert_eq!(cmd.id, "test");
        assert_eq!(cmd.label, "Test Command");
        assert_eq!(cmd.category, CommandCategory::Edit);
        assert!(cmd.description.is_none());
        assert!(cmd.shortcut.is_none());
    }

    #[test]
    fn command_with_builders() {
        let cmd = Command::new("x", "X", CommandCategory::File)
            .with_description("desc")
            .with_shortcut("Ctrl+X");
        assert_eq!(cmd.description.as_deref(), Some("desc"));
        assert_eq!(cmd.shortcut.as_deref(), Some("Ctrl+X"));
    }

    // ── CommandRegistry ──

    #[test]
    fn registry_new_empty() {
        let reg = CommandRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn registry_default_empty() {
        let reg = CommandRegistry::default();
        assert!(reg.is_empty());
    }

    #[test]
    fn registry_register_and_find() {
        let reg = sample_registry();
        assert_eq!(reg.len(), 9);
        assert!(!reg.is_empty());

        let cmd = reg.find_by_id("undo").unwrap();
        assert_eq!(cmd.label, "Undo");
    }

    #[test]
    fn registry_find_nonexistent() {
        let reg = sample_registry();
        assert!(reg.find_by_id("nonexistent").is_none());
    }

    #[test]
    fn registry_by_category() {
        let reg = sample_registry();
        let file_cmds = reg.by_category(CommandCategory::File);
        assert_eq!(file_cmds.len(), 2);
        assert!(
            file_cmds
                .iter()
                .all(|c| c.category == CommandCategory::File)
        );
    }

    #[test]
    fn registry_search_all() {
        let reg = sample_registry();
        let results = reg.search("");
        assert_eq!(results.len(), 9);
    }

    #[test]
    fn registry_search_filter() {
        let reg = sample_registry();
        let results = reg.search("file");
        assert!(results.len() >= 2); // "New File" and "Open File"
        assert!(results.iter().any(|c| c.id == "new_file"));
        assert!(results.iter().any(|c| c.id == "open_file"));
    }

    #[test]
    fn registry_search_partial() {
        let reg = sample_registry();
        let results = reg.search("und");
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "undo");
    }

    #[test]
    fn registry_search_no_match() {
        let reg = sample_registry();
        let results = reg.search("zzzzz");
        assert!(results.is_empty());
    }

    // ── fuzzy_score ──

    #[test]
    fn fuzzy_score_exact() {
        assert!(fuzzy_score("hello", "hello").is_some());
    }

    #[test]
    fn fuzzy_score_prefix() {
        let s = fuzzy_score("hello world", "hel");
        assert!(s.is_some());
    }

    #[test]
    fn fuzzy_score_scattered() {
        // h...l...p from "show help"
        let s = fuzzy_score("show help", "shp");
        assert!(s.is_some());
    }

    #[test]
    fn fuzzy_score_no_match() {
        assert!(fuzzy_score("hello", "xyz").is_none());
    }

    #[test]
    fn fuzzy_score_empty_needle() {
        // Empty needle always matches with score 0
        let s = fuzzy_score("anything", "");
        assert_eq!(s, Some(0));
    }

    // ── CommandPalette ──

    #[test]
    fn palette_starts_hidden() {
        let palette = CommandPalette::new(sample_registry());
        assert!(!palette.is_visible());
    }

    #[test]
    fn palette_show_hide() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        assert!(palette.is_visible());
        assert_eq!(palette.result_count(), 9);

        palette.hide();
        assert!(!palette.is_visible());
    }

    #[test]
    fn palette_typing_filters() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        assert_eq!(palette.result_count(), 9);

        palette.type_char('u');
        palette.type_char('n');
        palette.type_char('d');
        // Should filter to "Undo" at least
        assert!(palette.result_count() > 0);
        assert!(palette.result_count() < 9);
    }

    #[test]
    fn palette_backspace() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        palette.type_char('x');
        palette.type_char('y');
        assert_eq!(palette.query(), "xy");

        palette.backspace();
        assert_eq!(palette.query(), "x");

        palette.backspace();
        assert_eq!(palette.query(), "");
        assert_eq!(palette.result_count(), 9);
    }

    #[test]
    fn palette_navigation() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        assert_eq!(palette.selected(), 0);

        palette.select_next();
        assert_eq!(palette.selected(), 1);

        palette.select_prev();
        assert_eq!(palette.selected(), 0);

        palette.select_prev(); // wraps
        assert_eq!(palette.selected(), 8);

        palette.select_next(); // wraps
        assert_eq!(palette.selected(), 0);
    }

    #[test]
    fn palette_confirm() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        let id = palette.confirm();
        assert!(id.is_some());
    }

    #[test]
    fn palette_selected_command() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        let cmd = palette.selected_command();
        assert!(cmd.is_some());
    }

    #[test]
    fn palette_visible_commands() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        assert_eq!(palette.visible_commands().len(), 9);
    }

    #[test]
    fn palette_show_resets_state() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        palette.type_char('x');
        palette.select_next();
        palette.select_next();

        palette.show(); // reset
        assert_eq!(palette.query(), "");
        assert_eq!(palette.selected(), 0);
        assert_eq!(palette.result_count(), 9);
    }

    // ── Widget rendering ──

    #[test]
    fn widget_hidden_no_render() {
        let palette = CommandPalette::new(sample_registry());
        let theme = Theme::dark();
        let widget = CommandPaletteWidget::new(&palette, &theme);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
        // No crash; hidden palette should be no-op
    }

    #[test]
    fn widget_renders_visible() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        let theme = Theme::dark();
        let widget = CommandPaletteWidget::new(&palette, &theme);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains(">"), "Missing query prompt: {content}");
    }

    #[test]
    fn widget_tiny_area_no_panic() {
        let mut palette = CommandPalette::new(sample_registry());
        palette.show();
        let theme = Theme::dark();
        let widget = CommandPaletteWidget::new(&palette, &theme);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    // ── truncate ──

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate("hello world!", 8), "hello...");
    }

    #[test]
    fn truncate_exact() {
        assert_eq!(truncate("abc", 3), "abc");
    }
}
