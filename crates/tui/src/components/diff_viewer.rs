//! Interactive diff viewer overlay — browse file diffs with two-level navigation.
//!
//! Activated from the permission card's `(d) view full diff` hint or a
//! keybinding. Two views:
//!
//! - **`FileList`**: file names with `+N -M` stats, `↑↓`/`jk` to navigate,
//!   `Enter` to drill into a file.
//! - **`Detail`**: hunk-by-hunk diff rendering with `n`/`p` to jump hunks,
//!   `Esc` to return to the file list.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Remove,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
    pub additions: usize,
    pub deletions: usize,
}

// ---------------------------------------------------------------------------
// Unified diff parser
// ---------------------------------------------------------------------------

#[must_use]
pub fn parse_unified_diff(raw: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current_path = String::new();
    let mut current_hunks: Vec<DiffHunk> = Vec::new();
    let mut current_hunk_lines: Vec<DiffLine> = Vec::new();
    let mut current_hunk_header = String::new();
    let mut adds: usize = 0;
    let mut dels: usize = 0;
    let mut in_file = false;

    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            if in_file {
                if !current_hunk_header.is_empty() || !current_hunk_lines.is_empty() {
                    current_hunks.push(DiffHunk {
                        header: current_hunk_header.clone(),
                        lines: std::mem::take(&mut current_hunk_lines),
                    });
                }
                files.push(FileDiff {
                    path: std::mem::take(&mut current_path),
                    hunks: std::mem::take(&mut current_hunks),
                    additions: adds,
                    deletions: dels,
                });
            }
            adds = 0;
            dels = 0;
            current_hunk_header.clear();
            in_file = true;

            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            current_path = parts
                .last()
                .unwrap_or(&"")
                .strip_prefix("b/")
                .unwrap_or(parts.last().unwrap_or(&""))
                .to_string();
        } else if line.starts_with("+++ b/") {
            current_path = line.strip_prefix("+++ b/").unwrap_or("").to_string();
        } else if line.starts_with("@@") {
            if !current_hunk_header.is_empty() || !current_hunk_lines.is_empty() {
                current_hunks.push(DiffHunk {
                    header: current_hunk_header.clone(),
                    lines: std::mem::take(&mut current_hunk_lines),
                });
            }
            current_hunk_header = line.to_string();
        } else if line.starts_with("--- ") || line.starts_with("+++ ") {
            // skip file header lines
        } else if let Some(rest) = line.strip_prefix('+') {
            adds += 1;
            current_hunk_lines.push(DiffLine {
                kind: DiffLineKind::Add,
                content: rest.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix('-') {
            dels += 1;
            current_hunk_lines.push(DiffLine {
                kind: DiffLineKind::Remove,
                content: rest.to_string(),
            });
        } else {
            let content = line.strip_prefix(' ').unwrap_or(line);
            current_hunk_lines.push(DiffLine {
                kind: DiffLineKind::Context,
                content: content.to_string(),
            });
        }
    }

    if in_file {
        if !current_hunk_header.is_empty() || !current_hunk_lines.is_empty() {
            current_hunks.push(DiffHunk {
                header: current_hunk_header,
                lines: current_hunk_lines,
            });
        }
        files.push(FileDiff {
            path: current_path,
            hunks: current_hunks,
            additions: adds,
            deletions: dels,
        });
    }

    files
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    FileList,
    Detail,
}

// ---------------------------------------------------------------------------
// DiffViewerOverlay
// ---------------------------------------------------------------------------

pub struct DiffViewerOverlay {
    files: Vec<FileDiff>,
    selected_file: usize,
    scroll_offset: usize,
    current_hunk: usize,
    view: View,
}

impl DiffViewerOverlay {
    #[must_use]
    pub fn new(files: Vec<FileDiff>) -> Self {
        Self {
            files,
            selected_file: 0,
            scroll_offset: 0,
            current_hunk: 0,
            view: View::FileList,
        }
    }

    #[must_use]
    pub fn from_unified_diff(raw: &str) -> Self {
        Self::new(parse_unified_diff(raw))
    }

    fn enter_detail(&mut self) {
        if !self.files.is_empty() {
            self.view = View::Detail;
            self.scroll_offset = 0;
            self.current_hunk = 0;
        }
    }

    fn exit_detail(&mut self) {
        self.view = View::FileList;
        self.scroll_offset = 0;
    }

    fn total_detail_lines(&self) -> usize {
        let Some(file) = self.files.get(self.selected_file) else {
            return 0;
        };
        let mut count = 0;
        for hunk in &file.hunks {
            count += 1; // hunk header
            count += hunk.lines.len();
            count += 1; // blank separator
        }
        count
    }

    fn hunk_line_offsets(&self) -> Vec<usize> {
        let Some(file) = self.files.get(self.selected_file) else {
            return Vec::new();
        };
        let mut offsets = Vec::new();
        let mut offset = 0;
        for hunk in &file.hunks {
            offsets.push(offset);
            offset += 1 + hunk.lines.len() + 1;
        }
        offsets
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl Renderable for DiffViewerOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 20 {
            return;
        }

        Widget::render(Clear, area, buf);

        let title = match self.view {
            View::FileList => format!(
                " Diff Viewer ({} file{}) ",
                self.files.len(),
                if self.files.len() == 1 { "" } else { "s" }
            ),
            View::Detail => {
                let name = self
                    .files
                    .get(self.selected_file)
                    .map_or("???", |f| f.path.as_str());
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
            View::FileList => self.render_file_list(inner, buf),
            View::Detail => self.render_detail(inner, buf),
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0 // fullscreen
    }
}

impl DiffViewerOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render_file_list(&self, area: Rect, buf: &mut Buffer) {
        if self.files.is_empty() {
            let msg = Line::from(Span::styled(
                "  No changes to display.",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(
                msg,
                Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
            return;
        }

        let visible = area.height as usize;
        let start = self.scroll_offset;

        for (i, file) in self.files.iter().enumerate().skip(start) {
            let row = i - start;
            if row >= visible {
                break;
            }
            let is_selected = i == self.selected_file;

            let prefix = if is_selected { "▸ " } else { "  " };
            let name_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![
                Span::styled(prefix, name_style),
                Span::styled(file.path.clone(), name_style),
                Span::styled("  ", Style::default()),
            ];

            if file.additions > 0 {
                spans.push(Span::styled(
                    format!("+{}", file.additions),
                    Style::default().fg(Color::Green),
                ));
                spans.push(Span::raw(" "));
            }
            if file.deletions > 0 {
                spans.push(Span::styled(
                    format!("-{}", file.deletions),
                    Style::default().fg(Color::Red),
                ));
            }

            Widget::render(
                Line::from(spans),
                Rect {
                    x: area.x,
                    y: area.y + row as u16,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }

        // Footer hint
        if area.height > 1 {
            let hint = Line::from(Span::styled(
                " ↑↓/jk navigate  Enter view  q/Esc close",
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

    #[allow(clippy::cast_possible_truncation)]
    fn render_detail(&self, area: Rect, buf: &mut Buffer) {
        let Some(file) = self.files.get(self.selected_file) else {
            return;
        };

        let mut lines: Vec<Line<'static>> = Vec::new();

        for (hi, hunk) in file.hunks.iter().enumerate() {
            let hunk_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
            let marker = if hi == self.current_hunk {
                "▸ "
            } else {
                "  "
            };
            lines.push(Line::from(vec![
                Span::styled(marker, hunk_style),
                Span::styled(hunk.header.clone(), hunk_style),
            ]));

            for dl in &hunk.lines {
                let (prefix, style) = match dl.kind {
                    DiffLineKind::Add => ("+", Style::default().fg(Color::Green)),
                    DiffLineKind::Remove => ("-", Style::default().fg(Color::Red)),
                    DiffLineKind::Context => (" ", Style::default().fg(Color::DarkGray)),
                };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{}", dl.content),
                    style,
                )));
            }
            lines.push(Line::default());
        }

        let visible = area.height.saturating_sub(1) as usize; // reserve footer
        let start = self.scroll_offset;

        for (i, line) in lines.iter().enumerate().skip(start) {
            let row = i - start;
            if row >= visible {
                break;
            }
            Widget::render(
                line.clone(),
                Rect {
                    x: area.x,
                    y: area.y + row as u16,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }

        if area.height > 1 {
            let hunk_count = file.hunks.len();
            let hint = Line::from(Span::styled(
                format!(
                    " ↑↓/jk scroll  n/p hunk ({}/{})  Esc back",
                    self.current_hunk + 1,
                    hunk_count
                ),
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
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

impl Overlay for DiffViewerOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match self.view {
            View::FileList => self.handle_file_list_key(key),
            View::Detail => self.handle_detail_key(key),
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::Diff]
    }

    fn name(&self) -> &'static str {
        "diff_viewer"
    }
}

impl DiffViewerOverlay {
    fn handle_file_list_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_file = self.selected_file.saturating_sub(1);
                if self.selected_file < self.scroll_offset {
                    self.scroll_offset = self.selected_file;
                }
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.files.is_empty() {
                    self.selected_file = (self.selected_file + 1).min(self.files.len() - 1);
                }
                OverlayAction::Consumed
            }
            KeyCode::Enter => {
                self.enter_detail();
                OverlayAction::Consumed
            }
            KeyCode::Char('G') => {
                if !self.files.is_empty() {
                    self.selected_file = self.files.len() - 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::Char('g') => {
                self.selected_file = 0;
                self.scroll_offset = 0;
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Backspace => {
                self.exit_detail();
                OverlayAction::Consumed
            }
            KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.total_detail_lines().saturating_sub(1);
                self.scroll_offset = (self.scroll_offset + 1).min(max);
                OverlayAction::Consumed
            }
            KeyCode::Char('n') => {
                let hunk_count = self
                    .files
                    .get(self.selected_file)
                    .map_or(0, |f| f.hunks.len());
                if hunk_count > 0 {
                    self.current_hunk = (self.current_hunk + 1).min(hunk_count - 1);
                    let offsets = self.hunk_line_offsets();
                    if let Some(&off) = offsets.get(self.current_hunk) {
                        self.scroll_offset = off;
                    }
                }
                OverlayAction::Consumed
            }
            KeyCode::Char('p') => {
                self.current_hunk = self.current_hunk.saturating_sub(1);
                let offsets = self.hunk_line_offsets();
                if let Some(&off) = offsets.get(self.current_hunk) {
                    self.scroll_offset = off;
                }
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

    const SAMPLE_UNIFIED: &str = "\
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    println!(\"extra\");
 }
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn add(a: i32, b: i32) -> i32 {
+    // fast path
     a + b
 }
@@ -10,3 +11,2 @@
 pub fn sub(a: i32, b: i32) -> i32 {
-    // slow path
     a - b
 }";

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // --- Parser tests ---

    #[test]
    fn parse_two_files() {
        let files = parse_unified_diff(SAMPLE_UNIFIED);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[1].path, "src/lib.rs");
    }

    #[test]
    fn parse_file_stats() {
        let files = parse_unified_diff(SAMPLE_UNIFIED);
        assert_eq!(files[0].additions, 2);
        assert_eq!(files[0].deletions, 1);
        assert_eq!(files[1].additions, 1);
        assert_eq!(files[1].deletions, 1);
    }

    #[test]
    fn parse_hunks() {
        let files = parse_unified_diff(SAMPLE_UNIFIED);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[1].hunks.len(), 2);
    }

    #[test]
    fn parse_empty_diff() {
        let files = parse_unified_diff("");
        assert!(files.is_empty());
    }

    #[test]
    fn hunk_lines_counted_correctly() {
        let files = parse_unified_diff(SAMPLE_UNIFIED);
        let h = &files[0].hunks[0];
        assert_eq!(h.lines.len(), 5); // context + remove + add + add + context
    }

    // --- Overlay state tests ---

    #[test]
    fn starts_in_file_list() {
        let ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        assert_eq!(ov.view, View::FileList);
        assert_eq!(ov.selected_file, 0);
    }

    #[test]
    fn enter_switches_to_detail() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::Detail);
    }

    #[test]
    fn esc_from_detail_returns_to_file_list() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::Detail);
        ov.handle_key(key(KeyCode::Esc));
        assert_eq!(ov.view, View::FileList);
    }

    #[test]
    fn q_from_file_list_dismisses() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        assert!(matches!(
            ov.handle_key(key(KeyCode::Char('q'))),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn q_from_detail_dismisses() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            ov.handle_key(key(KeyCode::Char('q'))),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn navigate_files() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected_file, 1);
        ov.handle_key(key(KeyCode::Down));
        assert_eq!(ov.selected_file, 1); // clamped
        ov.handle_key(key(KeyCode::Up));
        assert_eq!(ov.selected_file, 0);
    }

    #[test]
    fn hunk_navigation() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Down)); // select lib.rs (2 hunks)
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.current_hunk, 0);
        ov.handle_key(key(KeyCode::Char('n')));
        assert_eq!(ov.current_hunk, 1);
        ov.handle_key(key(KeyCode::Char('n'))); // clamped
        assert_eq!(ov.current_hunk, 1);
        ov.handle_key(key(KeyCode::Char('p')));
        assert_eq!(ov.current_hunk, 0);
    }

    #[test]
    fn hunk_navigation_updates_scroll() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Down));
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Char('n')));
        assert!(ov.scroll_offset > 0);
    }

    #[test]
    fn detail_scroll() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Enter));
        ov.handle_key(key(KeyCode::Char('j')));
        assert_eq!(ov.scroll_offset, 1);
        ov.handle_key(key(KeyCode::Char('k')));
        assert_eq!(ov.scroll_offset, 0);
    }

    #[test]
    fn backspace_exits_detail() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Enter));
        assert_eq!(ov.view, View::Detail);
        ov.handle_key(key(KeyCode::Backspace));
        assert_eq!(ov.view, View::FileList);
    }

    #[test]
    fn g_and_big_g_jump() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Char('G')));
        assert_eq!(ov.selected_file, 1);
        ov.handle_key(key(KeyCode::Char('g')));
        assert_eq!(ov.selected_file, 0);
    }

    #[test]
    fn empty_diff_no_panic() {
        let mut ov = DiffViewerOverlay::from_unified_diff("");
        assert!(ov.files.is_empty());
        ov.handle_key(key(KeyCode::Enter)); // should not panic
        assert_eq!(ov.view, View::FileList); // stays in list
    }

    #[test]
    fn render_no_panic() {
        let ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_detail_no_panic() {
        let mut ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        ov.handle_key(key(KeyCode::Enter));
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_empty_no_panic() {
        let ov = DiffViewerOverlay::from_unified_diff("");
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn render_tiny_area_no_panic() {
        let ov = DiffViewerOverlay::from_unified_diff(SAMPLE_UNIFIED);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        ov.render(area, &mut buf);
    }

    #[test]
    fn overlay_name() {
        let ov = DiffViewerOverlay::from_unified_diff("");
        assert_eq!(ov.name(), "diff_viewer");
    }

    #[test]
    fn overlay_context() {
        let ov = DiffViewerOverlay::from_unified_diff("");
        assert_eq!(ov.contexts(), vec![KeyContext::Diff]);
    }
}
