//! Shortcut hint bar — displays context-sensitive keyboard shortcuts
//! at the bottom or side of the TUI.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// A single key binding hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    /// Key display string (e.g. "Ctrl+C", "Esc", "j/k").
    pub key: String,
    /// Description of what the key does.
    pub description: String,
}

impl KeyBinding {
    /// Create a new key binding.
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
        }
    }
}

/// A named group of key bindings.
#[derive(Debug, Clone)]
pub struct BindingGroup {
    /// Group name (e.g. "Navigation", "Editing").
    pub name: String,
    /// Bindings in this group.
    pub bindings: Vec<KeyBinding>,
}

impl BindingGroup {
    /// Create a new group.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bindings: Vec::new(),
        }
    }

    /// Add a binding.
    pub fn add(&mut self, key: impl Into<String>, desc: impl Into<String>) {
        self.bindings.push(KeyBinding::new(key, desc));
    }

    /// Builder: add a binding and return self.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, desc: impl Into<String>) -> Self {
        self.add(key, desc);
        self
    }
}

/// UI mode that determines which shortcuts are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintMode {
    Normal,
    Insert,
    Visual,
    Command,
    Search,
    Dialog,
}

impl std::fmt::Display for HintMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "NORMAL"),
            Self::Insert => write!(f, "INSERT"),
            Self::Visual => write!(f, "VISUAL"),
            Self::Command => write!(f, "COMMAND"),
            Self::Search => write!(f, "SEARCH"),
            Self::Dialog => write!(f, "DIALOG"),
        }
    }
}

/// Display style for the hint bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintStyle {
    /// Single-line compact display: "key:desc | key:desc".
    Compact,
    /// Multi-line expanded display with grouped sections.
    Expanded,
}

// ─── ShortcutHintBar ────────────────────────────────────────────────────

/// Manages shortcut hints for different modes.
pub struct ShortcutHintBar {
    /// Current display mode.
    mode: HintMode,
    /// Display style.
    style: HintStyle,
    /// Binding groups per mode.
    mode_bindings: Vec<(HintMode, Vec<BindingGroup>)>,
}

impl ShortcutHintBar {
    /// Create a new hint bar with default bindings.
    #[must_use]
    pub fn new() -> Self {
        let mut bar = Self {
            mode: HintMode::Normal,
            style: HintStyle::Compact,
            mode_bindings: Vec::new(),
        };
        bar.setup_defaults();
        bar
    }

    /// Set the active mode.
    pub fn set_mode(&mut self, mode: HintMode) {
        self.mode = mode;
    }

    /// Get the active mode.
    #[must_use]
    pub fn mode(&self) -> HintMode {
        self.mode
    }

    /// Set the display style.
    pub fn set_style(&mut self, style: HintStyle) {
        self.style = style;
    }

    /// Get the display style.
    #[must_use]
    pub fn hint_style(&self) -> HintStyle {
        self.style
    }

    /// Register binding groups for a mode.
    pub fn set_bindings(&mut self, mode: HintMode, groups: Vec<BindingGroup>) {
        if let Some(entry) = self.mode_bindings.iter_mut().find(|(m, _)| *m == mode) {
            entry.1 = groups;
        } else {
            self.mode_bindings.push((mode, groups));
        }
    }

    /// Get binding groups for the current mode.
    #[must_use]
    pub fn current_bindings(&self) -> &[BindingGroup] {
        self.mode_bindings
            .iter()
            .find(|(m, _)| *m == self.mode)
            .map_or(&[], |(_, groups)| groups)
    }

    /// Get all bindings (flattened) for the current mode.
    #[must_use]
    pub fn all_bindings(&self) -> Vec<&KeyBinding> {
        self.current_bindings()
            .iter()
            .flat_map(|g| g.bindings.iter())
            .collect()
    }

    /// Render compact (single-line) hints.
    #[must_use]
    pub fn render_compact(&self, theme: &Theme) -> Line<'static> {
        let key_style = Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(theme.muted);
        let sep_style = Style::default().fg(theme.border);
        let mode_style = Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD);

        let mut spans = Vec::new();
        spans.push(Span::styled(format!(" {} ", self.mode), mode_style));
        spans.push(Span::styled("│ ", sep_style));

        let bindings = self.all_bindings();
        for (i, binding) in bindings.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", sep_style));
            }
            spans.push(Span::styled(binding.key.clone(), key_style));
            spans.push(Span::styled(
                format!(":{}", binding.description),
                desc_style,
            ));
        }

        Line::from(spans)
    }

    /// Render expanded (multi-line) hints grouped by section.
    #[must_use]
    pub fn render_expanded(&self, theme: &Theme) -> Vec<Line<'static>> {
        let key_style = Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(theme.fg);
        let group_style = Style::default()
            .fg(theme.syntax_type)
            .add_modifier(Modifier::BOLD);

        let mut lines = Vec::new();

        for group in self.current_bindings() {
            // Group header
            lines.push(Line::from(Span::styled(
                format!("  {} ", group.name),
                group_style,
            )));

            // Bindings
            for binding in &group.bindings {
                lines.push(Line::from(vec![
                    Span::styled(format!("    {:>12}", binding.key), key_style),
                    Span::styled(format!("  {}", binding.description), desc_style),
                ]));
            }
        }

        lines
    }

    fn setup_defaults(&mut self) {
        // Normal mode
        self.set_bindings(
            HintMode::Normal,
            vec![
                BindingGroup::new("Navigation")
                    .with("j/k", "Up/Down")
                    .with("h/l", "Left/Right")
                    .with("gg/G", "Top/Bottom")
                    .with("PgUp/PgDn", "Scroll"),
                BindingGroup::new("Actions")
                    .with("i", "Insert mode")
                    .with("v", "Visual mode")
                    .with(":", "Command mode")
                    .with("/", "Search")
                    .with("Enter", "Submit")
                    .with("Ctrl+C", "Quit"),
                BindingGroup::new("Sessions")
                    .with("Ctrl+N", "New session")
                    .with("Ctrl+B", "Toggle sidebar"),
            ],
        );

        // Insert mode
        self.set_bindings(
            HintMode::Insert,
            vec![
                BindingGroup::new("Editing")
                    .with("Esc", "Normal mode")
                    .with("Enter", "Submit")
                    .with("Shift+Enter", "New line")
                    .with("Tab", "Autocomplete")
                    .with("Ctrl+C", "Quit"),
            ],
        );

        // Visual mode
        self.set_bindings(
            HintMode::Visual,
            vec![
                BindingGroup::new("Selection")
                    .with("Esc", "Normal mode")
                    .with("j/k", "Extend selection")
                    .with("y", "Copy"),
            ],
        );

        // Command mode
        self.set_bindings(
            HintMode::Command,
            vec![
                BindingGroup::new("Command")
                    .with("Esc", "Cancel")
                    .with("Enter", "Execute")
                    .with("Tab", "Autocomplete"),
            ],
        );

        // Search mode
        self.set_bindings(
            HintMode::Search,
            vec![
                BindingGroup::new("Search")
                    .with("Esc", "Cancel")
                    .with("Enter", "Confirm")
                    .with("n/N", "Next/Prev match"),
            ],
        );

        // Dialog mode
        self.set_bindings(
            HintMode::Dialog,
            vec![
                BindingGroup::new("Dialog")
                    .with("y", "Accept")
                    .with("n", "Deny")
                    .with("Esc", "Dismiss"),
            ],
        );
    }
}

impl Default for ShortcutHintBar {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Widget impl ────────────────────────────────────────────────────────

impl Widget for &ShortcutHintBar {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        // Dummy theme for Widget impl — callers should use render_compact/render_expanded
        // and render the resulting Line(s) themselves. This impl uses dark theme defaults.
        let theme = Theme::dark();
        match self.style {
            HintStyle::Compact => {
                if area.height >= 1 {
                    let line = self.render_compact(&theme);
                    let line_area = Rect::new(area.x, area.y, area.width, 1);
                    Widget::render(line, line_area, buf);
                }
            }
            HintStyle::Expanded => {
                let lines = self.render_expanded(&theme);
                for (i, line) in lines.iter().enumerate() {
                    let y = area.y + i as u16;
                    if y >= area.y + area.height {
                        break;
                    }
                    let line_area = Rect::new(area.x, y, area.width, 1);
                    Widget::render(line.clone(), line_area, buf);
                }
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── KeyBinding ──

    #[test]
    fn key_binding_new() {
        let kb = KeyBinding::new("Ctrl+C", "Quit");
        assert_eq!(kb.key, "Ctrl+C");
        assert_eq!(kb.description, "Quit");
    }

    // ── BindingGroup ──

    #[test]
    fn binding_group_new_empty() {
        let group = BindingGroup::new("Test");
        assert_eq!(group.name, "Test");
        assert!(group.bindings.is_empty());
    }

    #[test]
    fn binding_group_add() {
        let mut group = BindingGroup::new("Nav");
        group.add("j", "Down");
        group.add("k", "Up");
        assert_eq!(group.bindings.len(), 2);
    }

    #[test]
    fn binding_group_with_builder() {
        let group = BindingGroup::new("Nav")
            .with("j", "Down")
            .with("k", "Up")
            .with("h", "Left");
        assert_eq!(group.bindings.len(), 3);
        assert_eq!(group.bindings[0].key, "j");
    }

    // ── HintMode ──

    #[test]
    fn hint_mode_display() {
        assert_eq!(HintMode::Normal.to_string(), "NORMAL");
        assert_eq!(HintMode::Insert.to_string(), "INSERT");
        assert_eq!(HintMode::Visual.to_string(), "VISUAL");
        assert_eq!(HintMode::Command.to_string(), "COMMAND");
        assert_eq!(HintMode::Search.to_string(), "SEARCH");
        assert_eq!(HintMode::Dialog.to_string(), "DIALOG");
    }

    // ── ShortcutHintBar ──

    #[test]
    fn hint_bar_defaults() {
        let bar = ShortcutHintBar::new();
        assert_eq!(bar.mode(), HintMode::Normal);
        assert_eq!(bar.hint_style(), HintStyle::Compact);
    }

    #[test]
    fn hint_bar_default_trait() {
        let bar = ShortcutHintBar::default();
        assert_eq!(bar.mode(), HintMode::Normal);
    }

    #[test]
    fn hint_bar_has_normal_bindings() {
        let bar = ShortcutHintBar::new();
        let bindings = bar.current_bindings();
        assert!(!bindings.is_empty());
        assert!(bindings.iter().any(|g| g.name == "Navigation"));
    }

    #[test]
    fn hint_bar_switch_mode() {
        let mut bar = ShortcutHintBar::new();
        bar.set_mode(HintMode::Insert);
        assert_eq!(bar.mode(), HintMode::Insert);
        let bindings = bar.current_bindings();
        assert!(!bindings.is_empty());
        assert!(bindings.iter().any(|g| g.name == "Editing"));
    }

    #[test]
    fn hint_bar_all_modes_have_bindings() {
        let mut bar = ShortcutHintBar::new();
        for mode in [
            HintMode::Normal,
            HintMode::Insert,
            HintMode::Visual,
            HintMode::Command,
            HintMode::Search,
            HintMode::Dialog,
        ] {
            bar.set_mode(mode);
            assert!(
                !bar.current_bindings().is_empty(),
                "No bindings for mode {mode}"
            );
        }
    }

    #[test]
    fn hint_bar_all_bindings_flat() {
        let bar = ShortcutHintBar::new();
        let all = bar.all_bindings();
        assert!(all.len() > 5);
    }

    #[test]
    fn hint_bar_custom_bindings() {
        let mut bar = ShortcutHintBar::new();
        bar.set_bindings(
            HintMode::Normal,
            vec![BindingGroup::new("Custom").with("x", "Do X")],
        );
        let bindings = bar.current_bindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].name, "Custom");
    }

    #[test]
    fn hint_bar_set_style() {
        let mut bar = ShortcutHintBar::new();
        bar.set_style(HintStyle::Expanded);
        assert_eq!(bar.hint_style(), HintStyle::Expanded);
    }

    // ── Rendering ──

    #[test]
    fn render_compact_produces_line() {
        let bar = ShortcutHintBar::new();
        let theme = Theme::dark();
        let line = bar.render_compact(&theme);
        assert!(!line.spans.is_empty());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("NORMAL"), "Missing mode label: {text}");
    }

    #[test]
    fn render_compact_insert_mode() {
        let mut bar = ShortcutHintBar::new();
        bar.set_mode(HintMode::Insert);
        let theme = Theme::dark();
        let line = bar.render_compact(&theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("INSERT"), "Missing mode label: {text}");
        assert!(text.contains("Esc"), "Missing Esc hint: {text}");
    }

    #[test]
    fn render_expanded_produces_lines() {
        let bar = ShortcutHintBar::new();
        let theme = Theme::dark();
        let lines = bar.render_expanded(&theme);
        assert!(lines.len() > 5);
    }

    #[test]
    fn render_expanded_has_group_headers() {
        let bar = ShortcutHintBar::new();
        let theme = Theme::dark();
        let lines = bar.render_expanded(&theme);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("Navigation"), "Missing group header: {text}");
        assert!(text.contains("Actions"), "Missing group header: {text}");
    }

    // ── Widget ──

    #[test]
    fn widget_compact_renders() {
        let bar = ShortcutHintBar::new();
        let area = Rect::new(0, 0, 100, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);
    }

    #[test]
    fn widget_expanded_renders() {
        let mut bar = ShortcutHintBar::new();
        bar.set_style(HintStyle::Expanded);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);
    }

    #[test]
    fn widget_zero_height_no_panic() {
        let bar = ShortcutHintBar::new();
        let area = Rect::new(0, 0, 80, 0);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);
    }
}
