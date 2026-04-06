//! Project browser — full panel with file tree, project info, and search filter.
//!
//! Combines the file tree widget with a header (project name + git branch)
//! and a footer (file statistics), plus a search/filter input.

use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

use super::file_tree::{FileNode, FileTree, FileTreeWidget, NodeType};

// ─── Types ──────────────────────────────────────────────────────────────

/// Statistics about the project files.
#[derive(Debug, Clone, Default)]
pub struct FileStats {
    /// Total number of files.
    pub total_files: usize,
    /// Total number of directories.
    pub total_dirs: usize,
    /// Total lines across all files (if computed).
    pub total_lines: Option<usize>,
}

impl FileStats {
    /// Create new file stats.
    #[must_use]
    pub fn new(total_files: usize, total_dirs: usize) -> Self {
        Self {
            total_files,
            total_dirs,
            total_lines: None,
        }
    }

    /// Set total lines.
    #[must_use]
    pub fn with_lines(mut self, lines: usize) -> Self {
        self.total_lines = Some(lines);
        self
    }

    /// Format as a summary string.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("{} files", self.total_files),
            format!("{} dirs", self.total_dirs),
        ];
        if let Some(lines) = self.total_lines {
            parts.push(format!("{lines} lines"));
        }
        parts.join(" | ")
    }
}

/// Count files and directories in a tree recursively.
#[must_use]
pub fn count_nodes(roots: &[FileNode]) -> FileStats {
    let mut files = 0;
    let mut dirs = 0;
    count_recursive(roots, &mut files, &mut dirs);
    FileStats::new(files, dirs)
}

fn count_recursive(nodes: &[FileNode], files: &mut usize, dirs: &mut usize) {
    for node in nodes {
        match node.node_type {
            NodeType::Directory => {
                *dirs += 1;
                count_recursive(&node.children, files, dirs);
            }
            NodeType::File | NodeType::Symlink => {
                *files += 1;
            }
        }
    }
}

// ─── Fuzzy filter ───────────────────────────────────────────────────────

/// Check if a name matches a fuzzy query (case-insensitive ordered character match).
#[must_use]
pub fn fuzzy_matches(name: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let name_lower = name.to_lowercase();
    let query_lower = query.to_lowercase();
    let mut chars = query_lower.chars();
    let mut current = chars.next();
    for c in name_lower.chars() {
        if let Some(target) = current {
            if c == target {
                current = chars.next();
            }
        } else {
            break;
        }
    }
    current.is_none()
}

/// Collect paths from a tree that match a fuzzy query.
#[must_use]
pub fn filter_tree(roots: &[FileNode], query: &str) -> Vec<PathBuf> {
    let mut results = Vec::new();
    filter_recursive(roots, query, &mut results);
    results
}

fn filter_recursive(nodes: &[FileNode], query: &str, out: &mut Vec<PathBuf>) {
    for node in nodes {
        if fuzzy_matches(&node.name, query) {
            out.push(node.path.clone());
        }
        if node.node_type == NodeType::Directory {
            filter_recursive(&node.children, query, out);
        }
    }
}

// ─── ProjectBrowser ─────────────────────────────────────────────────────

/// The project browser panel state.
pub struct ProjectBrowser {
    /// Project name (usually the root directory name).
    project_name: String,
    /// Current git branch, if available.
    git_branch: Option<String>,
    /// The file tree.
    tree: FileTree,
    /// File statistics.
    stats: FileStats,
    /// Search filter query.
    filter_query: String,
    /// Whether the filter input is active.
    filter_active: bool,
    /// Filtered paths (when a filter is active).
    filtered_paths: Vec<PathBuf>,
}

impl ProjectBrowser {
    /// Create a new project browser.
    #[must_use]
    pub fn new(project_name: impl Into<String>, roots: Vec<FileNode>) -> Self {
        let stats = count_nodes(&roots);
        let tree = FileTree::new(roots);
        Self {
            project_name: project_name.into(),
            git_branch: None,
            tree,
            stats,
            filter_query: String::new(),
            filter_active: false,
            filtered_paths: Vec::new(),
        }
    }

    /// Set the git branch.
    #[must_use]
    pub fn with_git_branch(mut self, branch: impl Into<String>) -> Self {
        self.git_branch = Some(branch.into());
        self
    }

    /// Set file stats.
    pub fn set_stats(&mut self, stats: FileStats) {
        self.stats = stats;
    }

    /// Set the git branch at runtime.
    pub fn set_git_branch(&mut self, branch: Option<String>) {
        self.git_branch = branch;
    }

    /// Get the project name.
    #[must_use]
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// Get the git branch.
    #[must_use]
    pub fn git_branch(&self) -> Option<&str> {
        self.git_branch.as_deref()
    }

    /// Get a reference to the file tree.
    #[must_use]
    pub fn tree(&self) -> &FileTree {
        &self.tree
    }

    /// Get a mutable reference to the file tree.
    pub fn tree_mut(&mut self) -> &mut FileTree {
        &mut self.tree
    }

    /// Get file statistics.
    #[must_use]
    pub fn stats(&self) -> &FileStats {
        &self.stats
    }

    /// Whether the filter input is active.
    #[must_use]
    pub fn filter_active(&self) -> bool {
        self.filter_active
    }

    /// Get the current filter query.
    #[must_use]
    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    /// Activate the filter input.
    pub fn start_filter(&mut self) {
        self.filter_active = true;
        self.filter_query.clear();
        self.filtered_paths.clear();
    }

    /// Deactivate the filter input and clear the query.
    pub fn stop_filter(&mut self) {
        self.filter_active = false;
        self.filter_query.clear();
        self.filtered_paths.clear();
    }

    /// Type a character into the filter.
    pub fn filter_type_char(&mut self, ch: char) {
        self.filter_query.push(ch);
        self.update_filter();
    }

    /// Delete the last character from the filter.
    pub fn filter_backspace(&mut self) {
        self.filter_query.pop();
        self.update_filter();
    }

    /// Update filtered results based on the current query.
    fn update_filter(&mut self) {
        if self.filter_query.is_empty() {
            self.filtered_paths.clear();
        } else {
            self.filtered_paths = filter_tree(self.tree.roots(), &self.filter_query);
        }
    }

    /// Get filtered paths.
    #[must_use]
    pub fn filtered_paths(&self) -> &[PathBuf] {
        &self.filtered_paths
    }

    /// Update the tree roots and recompute stats.
    pub fn set_roots(&mut self, roots: Vec<FileNode>) {
        self.stats = count_nodes(&roots);
        self.tree.set_roots(roots);
    }

    /// Navigate up in the tree.
    pub fn select_prev(&mut self) {
        self.tree.select_prev();
    }

    /// Navigate down in the tree.
    pub fn select_next(&mut self) {
        self.tree.select_next();
    }

    /// Toggle expand/collapse on selected directory.
    pub fn toggle_expand(&mut self) {
        self.tree.toggle_expand();
    }

    /// Confirm the selected entry. Returns file path if a file was selected.
    #[must_use]
    pub fn confirm(&mut self) -> Option<PathBuf> {
        self.tree.confirm()
    }

    /// Get the selected file path.
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        self.tree.selected_path()
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the project browser panel.
pub struct ProjectBrowserWidget<'a> {
    browser: &'a ProjectBrowser,
    theme: &'a Theme,
}

impl<'a> ProjectBrowserWidget<'a> {
    #[must_use]
    pub fn new(browser: &'a ProjectBrowser, theme: &'a Theme) -> Self {
        Self { browser, theme }
    }
}

impl Widget for ProjectBrowserWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        // Layout: 1 line header + 1 line filter (if active) + tree body + 1 line footer
        let header_y = area.y;
        let filter_lines: u16 = u16::from(self.browser.filter_active);
        let filter_y = area.y + 1;
        let footer_y = area.y + area.height - 1;
        let tree_y = area.y + 1 + filter_lines;
        let tree_height = area.height.saturating_sub(2 + filter_lines);

        // ─── Header ───
        let branch_text = self
            .browser
            .git_branch()
            .map(|b| format!(" [{b}]"))
            .unwrap_or_default();
        let header_text = format!(" {}{}", self.browser.project_name(), branch_text);
        let header_style = Style::default()
            .fg(self.theme.heading)
            .bg(self.theme.bg)
            .add_modifier(Modifier::BOLD);

        // Clear header line
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, header_y)) {
                cell.set_char(' ');
                cell.set_style(header_style);
            }
        }
        let header_line = Line::from(Span::styled(header_text, header_style));
        Widget::render(header_line, Rect::new(area.x, header_y, area.width, 1), buf);

        // ─── Filter line ───
        if self.browser.filter_active {
            let filter_style = Style::default().fg(self.theme.fg).bg(self.theme.bg);
            let _filter_text = format!(" > {}", self.browser.filter_query());
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, filter_y)) {
                    cell.set_char(' ');
                    cell.set_style(filter_style);
                }
            }
            let filter_line = Line::from(vec![
                Span::styled(" > ", Style::default().fg(self.theme.warning)),
                Span::styled(self.browser.filter_query(), filter_style),
            ]);
            Widget::render(filter_line, Rect::new(area.x, filter_y, area.width, 1), buf);
        }

        // ─── Tree body ───
        if tree_height > 0 {
            let tree_area = Rect::new(area.x, tree_y, area.width, tree_height);
            let tree_widget = FileTreeWidget::new(self.browser.tree(), self.theme);
            Widget::render(tree_widget, tree_area, buf);
        }

        // ─── Footer ───
        let footer_text = format!(" {}", self.browser.stats().summary());
        let footer_style = Style::default().fg(self.theme.muted).bg(self.theme.bg);
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, footer_y)) {
                cell.set_char(' ');
                cell.set_style(footer_style);
            }
        }
        let footer_line = Line::from(Span::styled(footer_text, footer_style));
        Widget::render(footer_line, Rect::new(area.x, footer_y, area.width, 1), buf);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_roots() -> Vec<FileNode> {
        vec![
            FileNode::directory("src", "src")
                .with_expanded(true)
                .with_children(vec![
                    FileNode::file("main.rs", "src/main.rs"),
                    FileNode::file("lib.rs", "src/lib.rs"),
                ]),
            FileNode::file("Cargo.toml", "Cargo.toml"),
        ]
    }

    #[test]
    fn file_stats_summary() {
        let stats = FileStats::new(10, 3);
        assert_eq!(stats.summary(), "10 files | 3 dirs");
    }

    #[test]
    fn file_stats_with_lines() {
        let stats = FileStats::new(5, 2).with_lines(500);
        assert!(stats.summary().contains("500 lines"));
    }

    #[test]
    fn file_stats_default() {
        let stats = FileStats::default();
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_dirs, 0);
        assert!(stats.total_lines.is_none());
    }

    #[test]
    fn count_nodes_basic() {
        let roots = sample_roots();
        let stats = count_nodes(&roots);
        assert_eq!(stats.total_files, 3); // main.rs, lib.rs, Cargo.toml
        assert_eq!(stats.total_dirs, 1); // src
    }

    #[test]
    fn count_nodes_empty() {
        let stats = count_nodes(&[]);
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_dirs, 0);
    }

    #[test]
    fn fuzzy_matches_empty_query() {
        assert!(fuzzy_matches("anything", ""));
    }

    #[test]
    fn fuzzy_matches_exact() {
        assert!(fuzzy_matches("main.rs", "main.rs"));
    }

    #[test]
    fn fuzzy_matches_partial() {
        assert!(fuzzy_matches("main.rs", "mn"));
        assert!(fuzzy_matches("main.rs", "mrs"));
    }

    #[test]
    fn fuzzy_matches_case_insensitive() {
        assert!(fuzzy_matches("Main.rs", "main"));
        assert!(fuzzy_matches("main.rs", "MAIN"));
    }

    #[test]
    fn fuzzy_matches_no_match() {
        assert!(!fuzzy_matches("main.rs", "xyz"));
        assert!(!fuzzy_matches("main.rs", "zrs"));
    }

    #[test]
    fn filter_tree_basic() {
        let roots = sample_roots();
        let results = filter_tree(&roots, "rs");
        // main.rs, lib.rs match "rs"; "src" also matches "rs" (s, r... no, s-r-c vs r-s)
        // Actually "src" contains s,r,c and query "rs" needs r then s — s comes before r in "src" so no match
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn filter_tree_empty_query() {
        let roots = sample_roots();
        let results = filter_tree(&roots, "");
        // Everything matches empty query
        assert_eq!(results.len(), 4); // src, main.rs, lib.rs, Cargo.toml
    }

    #[test]
    fn project_browser_new() {
        let browser = ProjectBrowser::new("my-project", sample_roots());
        assert_eq!(browser.project_name(), "my-project");
        assert!(browser.git_branch().is_none());
        assert!(!browser.filter_active());
        assert_eq!(browser.stats().total_files, 3);
    }

    #[test]
    fn project_browser_with_git_branch() {
        let browser = ProjectBrowser::new("proj", sample_roots()).with_git_branch("main");
        assert_eq!(browser.git_branch(), Some("main"));
    }

    #[test]
    fn project_browser_set_git_branch() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        browser.set_git_branch(Some("develop".into()));
        assert_eq!(browser.git_branch(), Some("develop"));
        browser.set_git_branch(None);
        assert!(browser.git_branch().is_none());
    }

    #[test]
    fn project_browser_navigation() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        assert_eq!(browser.tree().selected(), 0);
        browser.select_next();
        assert_eq!(browser.tree().selected(), 1);
        browser.select_prev();
        assert_eq!(browser.tree().selected(), 0);
    }

    #[test]
    fn project_browser_toggle_expand() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        let initial = browser.tree().row_count();
        browser.toggle_expand(); // collapse src
        assert!(browser.tree().row_count() < initial);
    }

    #[test]
    fn project_browser_confirm_file() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        browser.select_next(); // main.rs
        let result = browser.confirm();
        assert_eq!(result, Some(PathBuf::from("src/main.rs")));
    }

    #[test]
    fn project_browser_filter() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        assert!(!browser.filter_active());

        browser.start_filter();
        assert!(browser.filter_active());
        assert_eq!(browser.filter_query(), "");

        browser.filter_type_char('m');
        assert_eq!(browser.filter_query(), "m");
        assert!(!browser.filtered_paths().is_empty());

        browser.filter_type_char('a');
        assert_eq!(browser.filter_query(), "ma");

        browser.filter_backspace();
        assert_eq!(browser.filter_query(), "m");

        browser.stop_filter();
        assert!(!browser.filter_active());
        assert_eq!(browser.filter_query(), "");
    }

    #[test]
    fn project_browser_set_roots() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        browser.set_roots(vec![FileNode::file("only.txt", "only.txt")]);
        assert_eq!(browser.tree().row_count(), 1);
        assert_eq!(browser.stats().total_files, 1);
    }

    #[test]
    fn project_browser_set_stats() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        browser.set_stats(FileStats::new(100, 20).with_lines(5000));
        assert_eq!(browser.stats().total_files, 100);
        assert_eq!(browser.stats().total_lines, Some(5000));
    }

    #[test]
    fn project_browser_selected_path() {
        let browser = ProjectBrowser::new("proj", sample_roots());
        assert!(browser.selected_path().is_some());
    }

    #[test]
    fn widget_renders() {
        let browser = ProjectBrowser::new("my-project", sample_roots()).with_git_branch("main");
        let theme = Theme::dark();
        let widget = ProjectBrowserWidget::new(&browser, &theme);
        let area = Rect::new(0, 0, 40, 15);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Check header contains project name
        let header: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(header.contains("my-project"));
        assert!(header.contains("[main]"));

        // Check footer contains stats
        let footer: String = (0..area.width)
            .map(|x| buf.cell((x, area.height - 1)).unwrap().symbol().to_string())
            .collect();
        assert!(footer.contains("files"));
    }

    #[test]
    fn widget_renders_with_filter() {
        let mut browser = ProjectBrowser::new("proj", sample_roots());
        browser.start_filter();
        browser.filter_type_char('m');
        let theme = Theme::dark();
        let widget = ProjectBrowserWidget::new(&browser, &theme);
        let area = Rect::new(0, 0, 40, 15);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Filter line at y=1
        let filter_row: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(filter_row.contains(">"));
    }

    #[test]
    fn widget_small_area() {
        let browser = ProjectBrowser::new("proj", sample_roots());
        let theme = Theme::dark();
        let widget = ProjectBrowserWidget::new(&browser, &theme);
        let area = Rect::new(0, 0, 5, 2);
        let mut buf = Buffer::empty(area);
        // Should not panic even with tiny area
        Widget::render(widget, area, &mut buf);
    }
}
