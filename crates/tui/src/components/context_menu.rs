//! Context menu — floating menu triggered by shortcut or right-click,
//! with support for nested submenus (1 level).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// A single menu item.
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// Display label.
    pub label: String,
    /// Action identifier (returned when selected).
    pub action: String,
    /// Optional shortcut display (e.g. "Ctrl+C").
    pub shortcut: Option<String>,
    /// Whether the item is enabled (greyed out if false).
    pub enabled: bool,
    /// If true, render a separator line after this item.
    pub separator_after: bool,
    /// Nested submenu items (1 level max).
    pub submenu: Vec<Self>,
}

impl MenuItem {
    /// Create a new enabled menu item.
    pub fn new(label: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
            shortcut: None,
            enabled: true,
            separator_after: false,
            submenu: Vec::new(),
        }
    }

    /// Create a separator item.
    #[must_use]
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            action: String::new(),
            shortcut: None,
            enabled: false,
            separator_after: true,
            submenu: Vec::new(),
        }
    }

    /// Set shortcut display.
    #[must_use]
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Set enabled state.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Add a separator after this item.
    #[must_use]
    pub fn with_separator(mut self) -> Self {
        self.separator_after = true;
        self
    }

    /// Add submenu items.
    #[must_use]
    pub fn with_submenu(mut self, items: Vec<Self>) -> Self {
        self.submenu = items;
        self
    }

    /// Whether this item is a pure separator.
    #[must_use]
    pub fn is_separator(&self) -> bool {
        self.label.is_empty() && self.separator_after
    }

    /// Whether this item has a submenu.
    #[must_use]
    pub fn has_submenu(&self) -> bool {
        !self.submenu.is_empty()
    }
}

// ─── ContextMenu state ──────────────────────────────────────────────────

/// Result of a menu interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// User selected a menu item (action id).
    Selected(String),
    /// Menu was dismissed.
    Dismissed,
    /// Key consumed but no final action.
    Consumed,
}

/// The context menu state.
pub struct ContextMenu {
    /// Whether the menu is visible.
    visible: bool,
    /// Menu items.
    items: Vec<MenuItem>,
    /// Selected index in the main menu.
    selected: usize,
    /// Position where menu should appear.
    position: (u16, u16),
    /// Whether a submenu is open.
    submenu_open: bool,
    /// Selected index in the submenu.
    submenu_selected: usize,
}

impl ContextMenu {
    /// Create a new hidden context menu.
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: false,
            items: Vec::new(),
            selected: 0,
            position: (0, 0),
            submenu_open: false,
            submenu_selected: 0,
        }
    }

    /// Show the menu at the given position with the given items.
    pub fn show(&mut self, x: u16, y: u16, items: Vec<MenuItem>) {
        self.visible = true;
        self.items = items;
        self.position = (x, y);
        self.selected = 0;
        self.submenu_open = false;
        self.submenu_selected = 0;
        self.skip_to_selectable(true);
    }

    /// Hide the menu.
    pub fn hide(&mut self) {
        self.visible = false;
        self.submenu_open = false;
    }

    /// Whether the menu is visible.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Current position.
    #[must_use]
    pub fn position(&self) -> (u16, u16) {
        self.position
    }

    /// Number of items.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Current selection index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether the submenu is open.
    #[must_use]
    pub fn is_submenu_open(&self) -> bool {
        self.submenu_open
    }

    /// Submenu selection index.
    #[must_use]
    pub fn submenu_selected(&self) -> usize {
        self.submenu_selected
    }

    /// Get the current items.
    #[must_use]
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// Move selection down, skipping separators.
    pub fn select_next(&mut self) {
        if self.submenu_open {
            if let Some(item) = self.items.get(self.selected)
                && !item.submenu.is_empty()
            {
                self.submenu_selected = (self.submenu_selected + 1) % item.submenu.len();
            }
        } else if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
            self.skip_to_selectable(true);
        }
    }

    /// Move selection up, skipping separators.
    pub fn select_prev(&mut self) {
        if self.submenu_open {
            if let Some(item) = self.items.get(self.selected)
                && !item.submenu.is_empty()
            {
                self.submenu_selected = if self.submenu_selected == 0 {
                    item.submenu.len() - 1
                } else {
                    self.submenu_selected - 1
                };
            }
        } else if !self.items.is_empty() {
            self.selected = if self.selected == 0 {
                self.items.len() - 1
            } else {
                self.selected - 1
            };
            self.skip_to_selectable(false);
        }
    }

    /// Open submenu of current item (if it has one), or confirm selection.
    #[must_use]
    pub fn confirm(&mut self) -> Option<MenuAction> {
        if self.submenu_open {
            if let Some(item) = self.items.get(self.selected)
                && let Some(sub_item) = item.submenu.get(self.submenu_selected)
                && sub_item.enabled
            {
                return Some(MenuAction::Selected(sub_item.action.clone()));
            }
            return None;
        }

        if let Some(item) = self.items.get(self.selected) {
            if item.has_submenu() {
                self.submenu_open = true;
                self.submenu_selected = 0;
                return Some(MenuAction::Consumed);
            }
            if item.enabled && !item.is_separator() {
                return Some(MenuAction::Selected(item.action.clone()));
            }
        }
        None
    }

    /// Close submenu (or dismiss menu if no submenu open).
    pub fn back(&mut self) {
        if self.submenu_open {
            self.submenu_open = false;
        } else {
            self.hide();
        }
    }

    /// Skip to next selectable item (not a separator and enabled).
    fn skip_to_selectable(&mut self, forward: bool) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len();
        for _ in 0..len {
            if let Some(item) = self.items.get(self.selected)
                && !item.is_separator()
            {
                return;
            }
            if forward {
                self.selected = (self.selected + 1) % len;
            } else {
                self.selected = if self.selected == 0 {
                    len - 1
                } else {
                    self.selected - 1
                };
            }
        }
    }

    /// Compute the menu rect, clamping to terminal bounds.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    pub fn compute_rect(&self, terminal: Rect) -> Rect {
        let width = self.menu_width().min(terminal.width as usize) as u16;
        let height = self.visible_height().min(terminal.height as usize) as u16;
        let (mut x, mut y) = self.position;

        // Clamp to terminal bounds
        if x + width > terminal.x + terminal.width {
            x = (terminal.x + terminal.width).saturating_sub(width);
        }
        if y + height > terminal.y + terminal.height {
            y = (terminal.y + terminal.height).saturating_sub(height);
        }

        Rect::new(x, y, width, height)
    }

    fn menu_width(&self) -> usize {
        let content_width = self
            .items
            .iter()
            .map(|item| {
                let label_len = item.label.len();
                let shortcut_len = item.shortcut.as_ref().map_or(0, |s| s.len() + 2);
                let submenu_indicator = if item.has_submenu() { 2 } else { 0 };
                label_len + shortcut_len + submenu_indicator
            })
            .max()
            .unwrap_or(10);
        content_width + 6 // padding + borders
    }

    fn visible_height(&self) -> usize {
        let item_count = self.items.len();
        let separator_count = self.items.iter().filter(|i| i.is_separator()).count();
        item_count - separator_count + separator_count + 2 // +2 for borders
    }
}

impl Default for ContextMenu {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Renders the context menu.
pub struct ContextMenuWidget<'a> {
    menu: &'a ContextMenu,
    theme: &'a Theme,
}

impl<'a> ContextMenuWidget<'a> {
    #[must_use]
    pub fn new(menu: &'a ContextMenu, theme: &'a Theme) -> Self {
        Self { menu, theme }
    }
}

impl Widget for ContextMenuWidget<'_> {
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.menu.is_visible() || self.menu.items.is_empty() {
            return;
        }

        let menu_rect = self.menu.compute_rect(area);
        if menu_rect.width < 4 || menu_rect.height < 3 {
            return;
        }

        let border_style = Style::default().fg(self.theme.border);
        let normal_style = Style::default().fg(self.theme.fg);
        let selected_style = Style::default()
            .fg(self.theme.fg)
            .bg(self.theme.border)
            .add_modifier(Modifier::BOLD);
        let disabled_style = Style::default().fg(self.theme.muted);
        let shortcut_style = Style::default().fg(self.theme.muted);
        let sep_style = Style::default().fg(self.theme.border);

        let inner_width = menu_rect.width.saturating_sub(2) as usize;

        // Top border
        let top = format!("┌{}┐", "─".repeat(inner_width));
        let top_line = Line::from(Span::styled(top, border_style));
        Widget::render(
            top_line,
            Rect::new(menu_rect.x, menu_rect.y, menu_rect.width, 1),
            buf,
        );

        // Items
        let mut row = 1u16;
        for (i, item) in self.menu.items.iter().enumerate() {
            let y = menu_rect.y + row;
            if y >= menu_rect.y + menu_rect.height - 1 {
                break;
            }

            if item.is_separator() {
                let sep = format!("├{}┤", "─".repeat(inner_width));
                Widget::render(
                    Line::from(Span::styled(sep, sep_style)),
                    Rect::new(menu_rect.x, y, menu_rect.width, 1),
                    buf,
                );
                row += 1;
                continue;
            }

            let is_selected = i == self.menu.selected() && !self.menu.is_submenu_open();
            let style = if !item.enabled {
                disabled_style
            } else if is_selected {
                selected_style
            } else {
                normal_style
            };

            let mut spans = Vec::new();
            spans.push(Span::styled("│ ", border_style));

            // Label
            let submenu_arrow = if item.has_submenu() { " >" } else { "" };
            let shortcut_text = item
                .shortcut
                .as_ref()
                .map_or(String::new(), |s| format!("  {s}"));
            let label_budget =
                inner_width.saturating_sub(2 + shortcut_text.len() + submenu_arrow.len());
            let label = if item.label.len() > label_budget {
                format!("{}...", &item.label[..label_budget.saturating_sub(3)])
            } else {
                item.label.clone()
            };
            let padding = label_budget.saturating_sub(label.len());

            spans.push(Span::styled(label, style));
            spans.push(Span::styled(" ".repeat(padding), style));

            if !shortcut_text.is_empty() {
                spans.push(Span::styled(shortcut_text, shortcut_style));
            }

            if item.has_submenu() {
                spans.push(Span::styled(submenu_arrow.to_string(), style));
            }

            spans.push(Span::styled(" │", border_style));

            Widget::render(
                Line::from(spans),
                Rect::new(menu_rect.x, y, menu_rect.width, 1),
                buf,
            );
            row += 1;
        }

        // Bottom border
        let bottom_y = menu_rect.y + row;
        if bottom_y < menu_rect.y + menu_rect.height {
            let bottom = format!("└{}┘", "─".repeat(inner_width));
            Widget::render(
                Line::from(Span::styled(bottom, border_style)),
                Rect::new(menu_rect.x, bottom_y, menu_rect.width, 1),
                buf,
            );
        }

        // Submenu rendering
        if self.menu.is_submenu_open()
            && let Some(parent) = self.menu.items.get(self.menu.selected())
            && !parent.submenu.is_empty()
        {
            let sub_x = menu_rect.x + menu_rect.width;
            let sub_y = menu_rect.y + self.menu.selected() as u16 + 1;
            render_submenu(
                &parent.submenu,
                self.menu.submenu_selected(),
                sub_x,
                sub_y,
                area,
                self.theme,
                buf,
            );
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
fn render_submenu(
    items: &[MenuItem],
    selected: usize,
    x: u16,
    y: u16,
    terminal: Rect,
    theme: &Theme,
    buf: &mut Buffer,
) {
    let border_style = Style::default().fg(theme.border);
    let normal_style = Style::default().fg(theme.fg);
    let selected_style = Style::default()
        .fg(theme.fg)
        .bg(theme.border)
        .add_modifier(Modifier::BOLD);

    let max_label = items.iter().map(|i| i.label.len()).max().unwrap_or(5);
    let width = (max_label + 6).min(terminal.width as usize) as u16;
    let height = (items.len() as u16 + 2).min(terminal.height);

    let sx = if x + width > terminal.x + terminal.width {
        terminal.x + terminal.width - width
    } else {
        x
    };
    let sy = if y + height > terminal.y + terminal.height {
        terminal.y + terminal.height - height
    } else {
        y
    };

    let inner = width.saturating_sub(2) as usize;

    // Top
    Widget::render(
        Line::from(Span::styled(
            format!("┌{}┐", "─".repeat(inner)),
            border_style,
        )),
        Rect::new(sx, sy, width, 1),
        buf,
    );

    for (i, item) in items.iter().enumerate() {
        let iy = sy + 1 + i as u16;
        if iy >= sy + height - 1 {
            break;
        }
        let style = if i == selected {
            selected_style
        } else {
            normal_style
        };
        let label_budget = inner.saturating_sub(2);
        let label = if item.label.len() > label_budget {
            format!("{}...", &item.label[..label_budget.saturating_sub(3)])
        } else {
            item.label.clone()
        };
        let pad = label_budget.saturating_sub(label.len());
        Widget::render(
            Line::from(vec![
                Span::styled("│ ", border_style),
                Span::styled(label, style),
                Span::styled(" ".repeat(pad), style),
                Span::styled(" │", border_style),
            ]),
            Rect::new(sx, iy, width, 1),
            buf,
        );
    }

    // Bottom
    let by = sy + 1 + items.len().min((height - 2) as usize) as u16;
    if by < sy + height {
        Widget::render(
            Line::from(Span::styled(
                format!("└{}┘", "─".repeat(inner)),
                border_style,
            )),
            Rect::new(sx, by, width, 1),
            buf,
        );
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_items() -> Vec<MenuItem> {
        vec![
            MenuItem::new("Copy", "copy").with_shortcut("Ctrl+C"),
            MenuItem::new("Paste", "paste").with_shortcut("Ctrl+V"),
            MenuItem::separator(),
            MenuItem::new("Delete", "delete").with_shortcut("Del"),
            MenuItem::new("More", "more").with_submenu(vec![
                MenuItem::new("Sub A", "sub_a"),
                MenuItem::new("Sub B", "sub_b"),
            ]),
        ]
    }

    // ── MenuItem ──

    #[test]
    fn menu_item_new() {
        let item = MenuItem::new("Copy", "copy");
        assert_eq!(item.label, "Copy");
        assert_eq!(item.action, "copy");
        assert!(item.enabled);
        assert!(!item.separator_after);
        assert!(!item.has_submenu());
        assert!(!item.is_separator());
    }

    #[test]
    fn menu_item_separator() {
        let sep = MenuItem::separator();
        assert!(sep.is_separator());
        assert!(!sep.enabled);
    }

    #[test]
    fn menu_item_with_shortcut() {
        let item = MenuItem::new("X", "x").with_shortcut("Ctrl+X");
        assert_eq!(item.shortcut.as_deref(), Some("Ctrl+X"));
    }

    #[test]
    fn menu_item_with_enabled() {
        let item = MenuItem::new("X", "x").with_enabled(false);
        assert!(!item.enabled);
    }

    #[test]
    fn menu_item_with_separator_after() {
        let item = MenuItem::new("X", "x").with_separator();
        assert!(item.separator_after);
        assert!(!item.is_separator()); // has label, so not a pure separator
    }

    #[test]
    fn menu_item_with_submenu() {
        let item = MenuItem::new("More", "more")
            .with_submenu(vec![MenuItem::new("A", "a"), MenuItem::new("B", "b")]);
        assert!(item.has_submenu());
        assert_eq!(item.submenu.len(), 2);
    }

    // ── ContextMenu ──

    #[test]
    fn menu_starts_hidden() {
        let menu = ContextMenu::new();
        assert!(!menu.is_visible());
        assert_eq!(menu.item_count(), 0);
    }

    #[test]
    fn menu_default() {
        let menu = ContextMenu::default();
        assert!(!menu.is_visible());
    }

    #[test]
    fn menu_show_hide() {
        let mut menu = ContextMenu::new();
        menu.show(10, 5, sample_items());
        assert!(menu.is_visible());
        assert_eq!(menu.position(), (10, 5));
        assert_eq!(menu.item_count(), 5);

        menu.hide();
        assert!(!menu.is_visible());
    }

    #[test]
    fn menu_skips_separator_on_show() {
        let mut menu = ContextMenu::new();
        // First item is a separator — should skip to next
        menu.show(
            0,
            0,
            vec![MenuItem::separator(), MenuItem::new("First", "first")],
        );
        assert_eq!(menu.selected(), 1);
    }

    #[test]
    fn menu_select_next_prev() {
        let mut menu = ContextMenu::new();
        menu.show(0, 0, sample_items());
        assert_eq!(menu.selected(), 0); // "Copy"

        menu.select_next();
        assert_eq!(menu.selected(), 1); // "Paste"

        menu.select_next();
        // Skips separator at index 2 -> lands on "Delete" at index 3
        assert_eq!(menu.selected(), 3);

        menu.select_prev();
        // Skips separator -> back to "Paste"
        assert_eq!(menu.selected(), 1);
    }

    #[test]
    fn menu_select_wraps() {
        let mut menu = ContextMenu::new();
        menu.show(0, 0, vec![MenuItem::new("A", "a"), MenuItem::new("B", "b")]);
        assert_eq!(menu.selected(), 0);

        menu.select_prev(); // wraps to end
        assert_eq!(menu.selected(), 1);

        menu.select_next(); // wraps to start
        assert_eq!(menu.selected(), 0);
    }

    #[test]
    fn menu_confirm_returns_action() {
        let mut menu = ContextMenu::new();
        menu.show(0, 0, sample_items());
        let result = menu.confirm();
        assert_eq!(result, Some(MenuAction::Selected("copy".into())));
    }

    #[test]
    fn menu_confirm_disabled_returns_none() {
        let mut menu = ContextMenu::new();
        menu.show(
            0,
            0,
            vec![MenuItem::new("Disabled", "dis").with_enabled(false)],
        );
        let result = menu.confirm();
        assert!(result.is_none());
    }

    #[test]
    fn menu_confirm_submenu_opens() {
        let mut menu = ContextMenu::new();
        menu.show(
            0,
            0,
            vec![MenuItem::new("More", "more").with_submenu(vec![MenuItem::new("Sub", "sub")])],
        );
        let result = menu.confirm();
        assert_eq!(result, Some(MenuAction::Consumed));
        assert!(menu.is_submenu_open());
    }

    #[test]
    fn menu_submenu_navigation() {
        let mut menu = ContextMenu::new();
        menu.show(
            0,
            0,
            vec![MenuItem::new("More", "more").with_submenu(vec![
                MenuItem::new("Sub A", "sub_a"),
                MenuItem::new("Sub B", "sub_b"),
            ])],
        );
        menu.confirm(); // open submenu
        assert!(menu.is_submenu_open());
        assert_eq!(menu.submenu_selected(), 0);

        menu.select_next();
        assert_eq!(menu.submenu_selected(), 1);

        menu.select_prev();
        assert_eq!(menu.submenu_selected(), 0);
    }

    #[test]
    fn menu_submenu_confirm() {
        let mut menu = ContextMenu::new();
        menu.show(
            0,
            0,
            vec![MenuItem::new("More", "more").with_submenu(vec![MenuItem::new("Sub A", "sub_a")])],
        );
        menu.confirm(); // open submenu
        let result = menu.confirm(); // select Sub A
        assert_eq!(result, Some(MenuAction::Selected("sub_a".into())));
    }

    #[test]
    fn menu_back_closes_submenu() {
        let mut menu = ContextMenu::new();
        menu.show(
            0,
            0,
            vec![MenuItem::new("More", "more").with_submenu(vec![MenuItem::new("Sub", "sub")])],
        );
        menu.confirm(); // open submenu
        assert!(menu.is_submenu_open());

        menu.back();
        assert!(!menu.is_submenu_open());
        assert!(menu.is_visible());
    }

    #[test]
    fn menu_back_hides_menu() {
        let mut menu = ContextMenu::new();
        menu.show(0, 0, sample_items());
        menu.back();
        assert!(!menu.is_visible());
    }

    // ── Position clamping ──

    #[test]
    fn compute_rect_within_bounds() {
        let mut menu = ContextMenu::new();
        menu.show(5, 5, sample_items());
        let terminal = Rect::new(0, 0, 80, 24);
        let rect = menu.compute_rect(terminal);
        assert!(rect.x + rect.width <= terminal.width);
        assert!(rect.y + rect.height <= terminal.height);
    }

    #[test]
    fn compute_rect_clamps_overflow() {
        let mut menu = ContextMenu::new();
        menu.show(75, 20, sample_items());
        let terminal = Rect::new(0, 0, 80, 24);
        let rect = menu.compute_rect(terminal);
        assert!(rect.x + rect.width <= terminal.width);
        assert!(rect.y + rect.height <= terminal.height);
    }

    // ── Widget rendering ──

    #[test]
    fn widget_hidden_no_render() {
        let menu = ContextMenu::new();
        let theme = Theme::dark();
        let widget = ContextMenuWidget::new(&menu, &theme);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn widget_renders_visible() {
        let mut menu = ContextMenu::new();
        menu.show(2, 2, sample_items());
        let theme = Theme::dark();
        let widget = ContextMenuWidget::new(&menu, &theme);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Check that border is rendered
        let cell = buf.cell((2, 2)).unwrap();
        assert_eq!(cell.symbol(), "┌");
    }

    #[test]
    fn widget_tiny_area_no_panic() {
        let mut menu = ContextMenu::new();
        menu.show(0, 0, sample_items());
        let theme = Theme::dark();
        let widget = ContextMenuWidget::new(&menu, &theme);
        let area = Rect::new(0, 0, 3, 2);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn widget_renders_submenu() {
        let mut menu = ContextMenu::new();
        menu.show(
            2,
            2,
            vec![MenuItem::new("More", "more").with_submenu(vec![MenuItem::new("Sub A", "sub_a")])],
        );
        menu.confirm(); // open submenu
        let theme = Theme::dark();
        let widget = ContextMenuWidget::new(&menu, &theme);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }
}
