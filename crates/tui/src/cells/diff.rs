use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiffBlock {
    pub file_path: String,
    pub hunks: Vec<DiffHunk>,
    pub collapsed: bool,
}

impl DiffBlock {
    #[must_use]
    pub fn new(file_path: impl Into<String>) -> Self {
        Self {
            file_path: file_path.into(),
            hunks: Vec::new(),
            collapsed: false,
        }
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut out = Vec::new();

        out.push(Line::from(Span::styled(
            format!("── {} ──", self.file_path),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));

        if self.collapsed {
            let total_lines: usize = self.hunks.iter().map(|h| h.lines.len()).sum();
            out.push(Line::from(Span::styled(
                format!("  ({total_lines} lines collapsed)"),
                Style::default().fg(Color::DarkGray),
            )));
            return out;
        }

        for hunk in &self.hunks {
            out.push(Line::from(Span::styled(
                hunk.header.clone(),
                Style::default().fg(Color::Blue),
            )));
            for line in &hunk.lines {
                let style = if line.starts_with('+') {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                out.push(Line::from(Span::styled(format!("  {line}"), style)));
            }
        }

        out
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    #[must_use]
    pub fn is_streaming(&self) -> bool {
        false
    }

    pub fn search_text(&self) -> String {
        let mut text = self.file_path.clone();
        for hunk in &self.hunks {
            text.push('\n');
            text.push_str(&hunk.header);
            for line in &hunk.lines {
                text.push('\n');
                text.push_str(line);
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diff() -> DiffBlock {
        DiffBlock {
            file_path: "src/main.rs".into(),
            hunks: vec![DiffHunk {
                header: "@@ -1,3 +1,4 @@".into(),
                lines: vec![
                    " fn main() {".into(),
                    "-    old_call();".into(),
                    "+    new_call();".into(),
                    "+    extra();".into(),
                    " }".into(),
                ],
            }],
            collapsed: false,
        }
    }

    #[test]
    fn renders_file_header() {
        let diff = sample_diff();
        let lines = diff.render_lines(80);
        let header: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(header.contains("src/main.rs"));
    }

    #[test]
    fn add_lines_green() {
        let diff = sample_diff();
        let lines = diff.render_lines(80);
        let add_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("+    new_call")));
        assert!(add_line.is_some());
        assert_eq!(add_line.unwrap().spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn remove_lines_red() {
        let diff = sample_diff();
        let lines = diff.render_lines(80);
        let rm_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("-    old_call")));
        assert!(rm_line.is_some());
        assert_eq!(rm_line.unwrap().spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn hunk_header_blue() {
        let diff = sample_diff();
        let lines = diff.render_lines(80);
        let hunk = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("@@")));
        assert_eq!(hunk.unwrap().spans[0].style.fg, Some(Color::Blue));
    }

    #[test]
    fn collapsed_shows_summary() {
        let mut diff = sample_diff();
        diff.collapsed = true;
        let lines = diff.render_lines(80);
        assert_eq!(lines.len(), 2);
        let text: String = lines[1].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("collapsed"));
    }
}
