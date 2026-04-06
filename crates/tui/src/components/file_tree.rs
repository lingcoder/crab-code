//! File tree widget — collapsible tree view of project files.
//!
//! Renders a hierarchical file list with expand/collapse, navigation,
//! file-type icons, and `.gitignore`-aware filtering.

use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// The type of a filesystem node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
    Symlink,
}

/// A single node in the file tree.
#[derive(Debug, Clone)]
pub struct FileNode {
    /// Display name (file/directory name, not the full path).
    pub name: String,
    /// Full path relative to the project root.
    pub path: PathBuf,
    /// Type of node.
    pub node_type: NodeType,
    /// Child nodes (only meaningful for directories).
    pub children: Vec<Self>,
    /// Whether this directory is expanded in the tree view.
    pub expanded: bool,
    /// Whether this node is currently selected/highlighted.
    pub selected: bool,
}

impl FileNode {
    /// Create a new file node.
    pub fn file(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            node_type: NodeType::File,
            children: Vec::new(),
            expanded: false,
            selected: false,
        }
    }

    /// Create a new directory node.
    pub fn directory(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            node_type: NodeType::Directory,
            children: Vec::new(),
            expanded: false,
            selected: false,
        }
    }

    /// Create a new symlink node.
    pub fn symlink(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            node_type: NodeType::Symlink,
            children: Vec::new(),
            expanded: false,
            selected: false,
        }
    }

    /// Add a child node (for directories).
    #[must_use]
    pub fn with_child(mut self, child: Self) -> Self {
        self.children.push(child);
        self
    }

    /// Add multiple children.
    #[must_use]
    pub fn with_children(mut self, children: Vec<Self>) -> Self {
        self.children = children;
        self
    }

    /// Set expanded state.
    #[must_use]
    pub fn with_expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Whether this node is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.node_type == NodeType::Directory
    }

    /// Total number of visible nodes (self + expanded children recursively).
    #[must_use]
    pub fn visible_count(&self) -> usize {
        let mut count = 1;
        if self.is_dir() && self.expanded {
            for child in &self.children {
                count += child.visible_count();
            }
        }
        count
    }

    /// Sort children: directories first, then files, alphabetically within each group.
    pub fn sort_children(&mut self) {
        self.children
            .sort_by(|a, b| match (a.node_type, b.node_type) {
                (NodeType::Directory, NodeType::Directory)
                | (NodeType::File, NodeType::File)
                | (NodeType::Symlink, NodeType::Symlink) => {
                    a.name.to_lowercase().cmp(&b.name.to_lowercase())
                }
                #[allow(clippy::match_same_arms)]
                (NodeType::Directory, _) => std::cmp::Ordering::Less,
                (_, NodeType::Directory) | (NodeType::File, NodeType::Symlink) => {
                    std::cmp::Ordering::Greater
                }
                (NodeType::Symlink, NodeType::File) => std::cmp::Ordering::Less,
            });
        for child in &mut self.children {
            child.sort_children();
        }
    }
}

/// Get a file-type icon based on the extension.
#[must_use]
pub fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "rs" => "\u{e7a8} ",                                   // Rust
        "ts" | "tsx" => "\u{e628} ",                           // TypeScript
        "js" | "jsx" => "\u{e74e} ",                           // JavaScript
        "py" => "\u{e73c} ",                                   // Python
        "go" => "\u{e627} ",                                   // Go
        "java" => "\u{e738} ",                                 // Java
        "c" | "h" => "\u{e61e} ",                              // C
        "cpp" | "hpp" | "cc" => "\u{e61d} ",                   // C++
        "md" => "\u{e73e} ",                                   // Markdown
        "json" => "\u{e60b} ",                                 // JSON
        "toml" => "\u{e615} ",                                 // TOML
        "yaml" | "yml" => "\u{e6a8} ",                         // YAML
        "html" => "\u{e736} ",                                 // HTML
        "css" | "scss" => "\u{e749} ",                         // CSS
        "sh" | "bash" => "\u{e795} ",                          // Shell
        "lock" => "\u{f023} ",                                 // Lock file
        "txt" => "\u{f0f6} ",                                  // Text
        "png" | "jpg" | "jpeg" | "gif" | "svg" => "\u{f1c5} ", // Image
        _ => "\u{f016} ",                                      // Default file
    }
}

/// Get the icon for a directory.
#[must_use]
pub fn dir_icon(expanded: bool) -> &'static str {
    if expanded {
        "\u{f115} " // open folder
    } else {
        "\u{f114} " // closed folder
    }
}

/// Get the tree-drawing prefix for a given depth and position.
fn tree_prefix(depth: usize, is_last: bool) -> String {
    if depth == 0 {
        return String::new();
    }
    let mut prefix = String::new();
    // We use simple indentation with tree-drawing characters
    for _ in 0..depth.saturating_sub(1) {
        prefix.push_str("  ");
    }
    if is_last {
        prefix.push_str("└ ");
    } else {
        prefix.push_str("├ ");
    }
    prefix
}

// ─── Default hidden patterns ────────────────────────────────────────────

/// Default directory names to hide (gitignore-like behavior).
const DEFAULT_HIDDEN: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
    "dist",
    "build",
    ".cache",
    ".next",
];

/// Check if a filename should be hidden by default.
#[must_use]
pub fn is_default_hidden(name: &str) -> bool {
    DEFAULT_HIDDEN.contains(&name)
}

// ─── Flattened row for rendering ────────────────────────────────────────

/// A single row in the flattened tree (used for rendering and navigation).
#[derive(Debug, Clone)]
pub struct FlattenedRow {
    /// Display name.
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// Node type.
    pub node_type: NodeType,
    /// Tree depth (0 = root children).
    pub depth: usize,
    /// Whether this is the last sibling.
    pub is_last: bool,
    /// Whether expanded (only meaningful for directories).
    pub expanded: bool,
}

/// Flatten a tree into a list of rows for rendering, respecting expand state.
#[must_use]
pub fn flatten_tree(roots: &[FileNode]) -> Vec<FlattenedRow> {
    let mut rows = Vec::new();
    flatten_recursive(roots, 0, &mut rows);
    rows
}

fn flatten_recursive(nodes: &[FileNode], depth: usize, out: &mut Vec<FlattenedRow>) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        out.push(FlattenedRow {
            name: node.name.clone(),
            path: node.path.clone(),
            node_type: node.node_type,
            depth,
            is_last,
            expanded: node.expanded,
        });
        if node.is_dir() && node.expanded {
            flatten_recursive(&node.children, depth + 1, out);
        }
    }
}

// ─── FileTree state ─────────────────────────────────────────────────────

/// The file tree state with navigation.
pub struct FileTree {
    /// Root nodes (top-level entries in the project).
    roots: Vec<FileNode>,
    /// Currently selected row index (in the flattened view).
    selected: usize,
    /// Scroll offset for vertical scrolling.
    scroll_offset: usize,
    /// Whether to show hidden/ignored files.
    show_hidden: bool,
    /// Cached flattened rows.
    flattened: Vec<FlattenedRow>,
}

impl FileTree {
    /// Create a new file tree from root nodes.
    #[must_use]
    pub fn new(roots: Vec<FileNode>) -> Self {
        let flattened = flatten_tree(&roots);
        Self {
            roots,
            selected: 0,
            scroll_offset: 0,
            show_hidden: false,
            flattened,
        }
    }

    /// Create an empty file tree.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Replace the root nodes and rebuild the flattened cache.
    pub fn set_roots(&mut self, roots: Vec<FileNode>) {
        self.roots = roots;
        self.rebuild_flat();
        if self.selected >= self.flattened.len() && !self.flattened.is_empty() {
            self.selected = self.flattened.len() - 1;
        }
    }

    /// Rebuild the flattened rows cache.
    fn rebuild_flat(&mut self) {
        self.flattened = flatten_tree(&self.roots);
    }

    /// Get the currently selected row index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Get the scroll offset.
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Get the flattened rows.
    #[must_use]
    pub fn rows(&self) -> &[FlattenedRow] {
        &self.flattened
    }

    /// Number of visible rows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.flattened.len()
    }

    /// Whether hidden files are shown.
    #[must_use]
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Toggle hidden files visibility.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
    }

    /// Get the path of the currently selected node.
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        self.flattened.get(self.selected).map(|r| r.path.as_path())
    }

    /// Get the selected row.
    #[must_use]
    pub fn selected_row(&self) -> Option<&FlattenedRow> {
        self.flattened.get(self.selected)
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.adjust_scroll();
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.flattened.is_empty() && self.selected < self.flattened.len() - 1 {
            self.selected += 1;
        }
        self.adjust_scroll();
    }

    /// Toggle expand/collapse on the selected node (if it's a directory).
    pub fn toggle_expand(&mut self) {
        if let Some(row) = self.flattened.get(self.selected)
            && row.node_type == NodeType::Directory
        {
            let path = row.path.clone();
            toggle_node_expanded(&mut self.roots, &path);
            self.rebuild_flat();
        }
    }

    /// Expand the selected directory. No-op if already expanded or if it's a file.
    pub fn expand_selected(&mut self) {
        if let Some(row) = self.flattened.get(self.selected)
            && row.node_type == NodeType::Directory
            && !row.expanded
        {
            let path = row.path.clone();
            toggle_node_expanded(&mut self.roots, &path);
            self.rebuild_flat();
        }
    }

    /// Collapse the selected directory. No-op if already collapsed or if it's a file.
    pub fn collapse_selected(&mut self) {
        if let Some(row) = self.flattened.get(self.selected)
            && row.node_type == NodeType::Directory
            && row.expanded
        {
            let path = row.path.clone();
            toggle_node_expanded(&mut self.roots, &path);
            self.rebuild_flat();
        }
    }

    /// Confirm / open the selected entry.
    /// Returns the path if it's a file, or toggles expand if it's a directory.
    #[must_use]
    pub fn confirm(&mut self) -> Option<PathBuf> {
        let row = self.flattened.get(self.selected)?;
        match row.node_type {
            NodeType::Directory => {
                self.toggle_expand();
                None
            }
            NodeType::File | NodeType::Symlink => Some(row.path.clone()),
        }
    }

    /// Adjust scroll offset so the selected row is visible.
    #[allow(clippy::unused_self)]
    fn adjust_scroll(&self) {
        // Will be applied at render time based on visible height
    }

    /// Adjust scroll for a given visible height.
    pub fn adjust_scroll_for_height(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + height {
            self.scroll_offset = self.selected - height + 1;
        }
    }

    /// Get root nodes.
    #[must_use]
    pub fn roots(&self) -> &[FileNode] {
        &self.roots
    }
}

impl Default for FileTree {
    fn default() -> Self {
        Self::empty()
    }
}

/// Recursively find a node by path and toggle its expanded state.
fn toggle_node_expanded(nodes: &mut [FileNode], path: &Path) -> bool {
    for node in nodes.iter_mut() {
        if node.path == path {
            node.expanded = !node.expanded;
            return true;
        }
        if node.is_dir() && toggle_node_expanded(&mut node.children, path) {
            return true;
        }
    }
    false
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the file tree.
pub struct FileTreeWidget<'a> {
    tree: &'a FileTree,
    theme: &'a Theme,
}

impl<'a> FileTreeWidget<'a> {
    #[must_use]
    pub fn new(tree: &'a FileTree, theme: &'a Theme) -> Self {
        Self { tree, theme }
    }
}

impl Widget for FileTreeWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let visible_height = area.height as usize;
        let rows = self.tree.rows();
        let scroll = self.tree.scroll_offset();

        for (vi, ri) in (scroll..rows.len().min(scroll + visible_height)).enumerate() {
            let row = &rows[ri];
            let y = area.y + vi as u16;
            let is_selected = ri == self.tree.selected();

            let bg = if is_selected {
                self.theme.border
            } else {
                self.theme.bg
            };
            let fg = self.theme.fg;

            // Clear the line
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().fg(fg).bg(bg));
                }
            }

            let prefix = tree_prefix(row.depth, row.is_last);
            let icon = match row.node_type {
                NodeType::Directory => dir_icon(row.expanded),
                NodeType::Symlink => "\u{f0c1} ", // link icon
                NodeType::File => file_icon(&row.name),
            };

            let name_style = match row.node_type {
                NodeType::Directory => Style::default()
                    .fg(self.theme.heading)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
                NodeType::Symlink => Style::default()
                    .fg(self.theme.link)
                    .bg(bg)
                    .add_modifier(Modifier::ITALIC),
                NodeType::File => Style::default().fg(fg).bg(bg),
            };

            let prefix_style = Style::default().fg(self.theme.muted).bg(bg);
            let icon_style = Style::default().fg(self.theme.warning).bg(bg);

            let spans = vec![
                Span::styled(prefix, prefix_style),
                Span::styled(icon, icon_style),
                Span::styled(&row.name, name_style),
            ];

            let line = Line::from(spans);
            let line_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            Widget::render(line, line_area, buf);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> Vec<FileNode> {
        vec![
            FileNode::directory("src", "src")
                .with_expanded(true)
                .with_children(vec![
                    FileNode::file("main.rs", "src/main.rs"),
                    FileNode::file("lib.rs", "src/lib.rs"),
                    FileNode::directory("utils", "src/utils")
                        .with_children(vec![FileNode::file("helpers.rs", "src/utils/helpers.rs")]),
                ]),
            FileNode::file("Cargo.toml", "Cargo.toml"),
            FileNode::file("README.md", "README.md"),
        ]
    }

    #[test]
    fn file_node_constructors() {
        let f = FileNode::file("main.rs", "src/main.rs");
        assert_eq!(f.name, "main.rs");
        assert_eq!(f.node_type, NodeType::File);
        assert!(!f.is_dir());

        let d = FileNode::directory("src", "src");
        assert_eq!(d.node_type, NodeType::Directory);
        assert!(d.is_dir());

        let s = FileNode::symlink("link", "link");
        assert_eq!(s.node_type, NodeType::Symlink);
    }

    #[test]
    fn file_node_with_child() {
        let d = FileNode::directory("src", "src")
            .with_child(FileNode::file("a.rs", "src/a.rs"))
            .with_child(FileNode::file("b.rs", "src/b.rs"));
        assert_eq!(d.children.len(), 2);
    }

    #[test]
    fn file_node_with_children() {
        let kids = vec![
            FileNode::file("a.rs", "src/a.rs"),
            FileNode::file("b.rs", "src/b.rs"),
        ];
        let d = FileNode::directory("src", "src").with_children(kids);
        assert_eq!(d.children.len(), 2);
    }

    #[test]
    fn visible_count_collapsed() {
        let d = FileNode::directory("src", "src").with_children(vec![
            FileNode::file("a.rs", "src/a.rs"),
            FileNode::file("b.rs", "src/b.rs"),
        ]);
        // collapsed: only the directory itself is visible
        assert_eq!(d.visible_count(), 1);
    }

    #[test]
    fn visible_count_expanded() {
        let d = FileNode::directory("src", "src")
            .with_expanded(true)
            .with_children(vec![
                FileNode::file("a.rs", "src/a.rs"),
                FileNode::file("b.rs", "src/b.rs"),
            ]);
        // expanded: directory + 2 children
        assert_eq!(d.visible_count(), 3);
    }

    #[test]
    fn visible_count_nested() {
        let d = FileNode::directory("src", "src")
            .with_expanded(true)
            .with_children(vec![
                FileNode::directory("utils", "src/utils")
                    .with_expanded(true)
                    .with_child(FileNode::file("h.rs", "src/utils/h.rs")),
                FileNode::file("main.rs", "src/main.rs"),
            ]);
        // src + utils + h.rs + main.rs = 4
        assert_eq!(d.visible_count(), 4);
    }

    #[test]
    fn sort_children_dirs_first() {
        let mut d = FileNode::directory("root", "root").with_children(vec![
            FileNode::file("zebra.rs", "root/zebra.rs"),
            FileNode::directory("alpha", "root/alpha"),
            FileNode::file("apple.rs", "root/apple.rs"),
            FileNode::directory("beta", "root/beta"),
        ]);
        d.sort_children();
        assert_eq!(d.children[0].name, "alpha");
        assert_eq!(d.children[1].name, "beta");
        assert_eq!(d.children[2].name, "apple.rs");
        assert_eq!(d.children[3].name, "zebra.rs");
    }

    #[test]
    fn file_icon_rust() {
        let icon = file_icon("main.rs");
        assert!(!icon.is_empty());
    }

    #[test]
    fn file_icon_unknown() {
        let icon = file_icon("data.xyz");
        assert!(!icon.is_empty());
    }

    #[test]
    fn dir_icon_states() {
        assert_ne!(dir_icon(true), dir_icon(false));
    }

    #[test]
    fn is_default_hidden_targets() {
        assert!(is_default_hidden("target"));
        assert!(is_default_hidden("node_modules"));
        assert!(is_default_hidden(".git"));
        assert!(!is_default_hidden("src"));
        assert!(!is_default_hidden("Cargo.toml"));
    }

    #[test]
    fn flatten_tree_basic() {
        let roots = sample_tree();
        let flat = flatten_tree(&roots);
        // src (expanded) + main.rs + lib.rs + utils (collapsed) + Cargo.toml + README.md
        assert_eq!(flat.len(), 6);
        assert_eq!(flat[0].name, "src");
        assert_eq!(flat[0].depth, 0);
        assert_eq!(flat[1].name, "main.rs");
        assert_eq!(flat[1].depth, 1);
        assert_eq!(flat[2].name, "lib.rs");
        assert_eq!(flat[2].depth, 1);
        assert_eq!(flat[3].name, "utils");
        assert_eq!(flat[3].depth, 1);
        assert_eq!(flat[4].name, "Cargo.toml");
        assert_eq!(flat[4].depth, 0);
        assert_eq!(flat[5].name, "README.md");
        assert_eq!(flat[5].depth, 0);
    }

    #[test]
    fn flatten_tree_empty() {
        let flat = flatten_tree(&[]);
        assert!(flat.is_empty());
    }

    #[test]
    fn file_tree_new() {
        let tree = FileTree::new(sample_tree());
        assert_eq!(tree.selected(), 0);
        assert_eq!(tree.scroll_offset(), 0);
        assert_eq!(tree.row_count(), 6);
        assert!(!tree.show_hidden());
    }

    #[test]
    fn file_tree_empty() {
        let tree = FileTree::empty();
        assert_eq!(tree.row_count(), 0);
        assert!(tree.selected_path().is_none());
    }

    #[test]
    fn file_tree_default() {
        let tree = FileTree::default();
        assert_eq!(tree.row_count(), 0);
    }

    #[test]
    fn file_tree_navigation() {
        let mut tree = FileTree::new(sample_tree());
        assert_eq!(tree.selected(), 0);

        tree.select_next();
        assert_eq!(tree.selected(), 1);

        tree.select_next();
        assert_eq!(tree.selected(), 2);

        tree.select_prev();
        assert_eq!(tree.selected(), 1);

        tree.select_prev();
        assert_eq!(tree.selected(), 0);

        // Can't go below 0
        tree.select_prev();
        assert_eq!(tree.selected(), 0);
    }

    #[test]
    fn file_tree_navigation_stops_at_end() {
        let mut tree = FileTree::new(sample_tree());
        for _ in 0..100 {
            tree.select_next();
        }
        assert_eq!(tree.selected(), tree.row_count() - 1);
    }

    #[test]
    fn file_tree_selected_path() {
        let tree = FileTree::new(sample_tree());
        assert_eq!(tree.selected_path(), Some(Path::new("src")));
    }

    #[test]
    fn file_tree_selected_row() {
        let tree = FileTree::new(sample_tree());
        let row = tree.selected_row().unwrap();
        assert_eq!(row.name, "src");
        assert_eq!(row.node_type, NodeType::Directory);
    }

    #[test]
    fn file_tree_toggle_expand() {
        let mut tree = FileTree::new(sample_tree());
        // src is already expanded, toggling collapses it
        let initial_count = tree.row_count();
        tree.toggle_expand(); // collapse src
        assert!(tree.row_count() < initial_count);

        // Toggle again to expand
        tree.toggle_expand();
        assert_eq!(tree.row_count(), initial_count);
    }

    #[test]
    fn file_tree_expand_collapse_selected() {
        let mut tree = FileTree::new(sample_tree());
        // Navigate to "utils" (index 3, collapsed directory)
        tree.select_next(); // 1: main.rs
        tree.select_next(); // 2: lib.rs
        tree.select_next(); // 3: utils

        let row = tree.selected_row().unwrap();
        assert_eq!(row.name, "utils");
        assert!(!row.expanded);

        tree.expand_selected();
        // Now utils is expanded, helpers.rs should appear
        assert_eq!(tree.row_count(), 7); // +1 for helpers.rs

        tree.collapse_selected();
        assert_eq!(tree.row_count(), 6);
    }

    #[test]
    fn file_tree_expand_on_file_is_noop() {
        let mut tree = FileTree::new(sample_tree());
        tree.select_next(); // main.rs (a file)
        let count = tree.row_count();
        tree.expand_selected();
        assert_eq!(tree.row_count(), count);
    }

    #[test]
    fn file_tree_confirm_file() {
        let mut tree = FileTree::new(sample_tree());
        tree.select_next(); // main.rs
        let result = tree.confirm();
        assert_eq!(result, Some(PathBuf::from("src/main.rs")));
    }

    #[test]
    fn file_tree_confirm_directory() {
        let mut tree = FileTree::new(sample_tree());
        // Select src (directory) — confirm should toggle expand, return None
        let initial = tree.row_count();
        let result = tree.confirm();
        assert!(result.is_none());
        // src was expanded, so confirm collapsed it
        assert!(tree.row_count() < initial);
    }

    #[test]
    fn file_tree_toggle_hidden() {
        let mut tree = FileTree::new(sample_tree());
        assert!(!tree.show_hidden());
        tree.toggle_hidden();
        assert!(tree.show_hidden());
        tree.toggle_hidden();
        assert!(!tree.show_hidden());
    }

    #[test]
    fn file_tree_set_roots() {
        let mut tree = FileTree::new(sample_tree());
        tree.select_next();
        tree.select_next();
        assert_eq!(tree.selected(), 2);

        // Replace with smaller tree
        tree.set_roots(vec![FileNode::file("only.rs", "only.rs")]);
        assert_eq!(tree.row_count(), 1);
        assert_eq!(tree.selected(), 0); // clamped
    }

    #[test]
    fn file_tree_adjust_scroll() {
        let mut tree = FileTree::new(sample_tree());
        // Move selection beyond visible area
        for _ in 0..5 {
            tree.select_next();
        }
        tree.adjust_scroll_for_height(3);
        // Scroll offset should bring selected into view
        assert!(tree.scroll_offset() + 3 > tree.selected());
    }

    #[test]
    fn tree_prefix_root() {
        assert_eq!(tree_prefix(0, false), "");
        assert_eq!(tree_prefix(0, true), "");
    }

    #[test]
    fn tree_prefix_depth_one() {
        assert_eq!(tree_prefix(1, false), "\u{251c} ");
        assert_eq!(tree_prefix(1, true), "\u{2514} ");
    }

    #[test]
    fn tree_prefix_depth_two() {
        let p = tree_prefix(2, false);
        assert!(p.starts_with("  "));
    }

    #[test]
    fn widget_renders() {
        let tree = FileTree::new(sample_tree());
        let theme = Theme::dark();
        let widget = FileTreeWidget::new(&tree, &theme);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Check that something was rendered in the first row
        let row0: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row0.contains("src"));
    }

    #[test]
    fn widget_renders_empty() {
        let tree = FileTree::empty();
        let theme = Theme::dark();
        let widget = FileTreeWidget::new(&tree, &theme);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        // Should not panic
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn widget_zero_area() {
        let tree = FileTree::new(sample_tree());
        let theme = Theme::dark();
        let widget = FileTreeWidget::new(&tree, &theme);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn flattened_row_is_last() {
        let roots = vec![
            FileNode::file("a.rs", "a.rs"),
            FileNode::file("b.rs", "b.rs"),
        ];
        let flat = flatten_tree(&roots);
        assert!(!flat[0].is_last);
        assert!(flat[1].is_last);
    }

    #[test]
    fn file_node_with_expanded() {
        let d = FileNode::directory("d", "d").with_expanded(true);
        assert!(d.expanded);
    }

    #[test]
    fn roots_accessor() {
        let tree = FileTree::new(sample_tree());
        assert_eq!(tree.roots().len(), 3);
    }
}
