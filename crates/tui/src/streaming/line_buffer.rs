/// Accumulates streaming text and splits it into committed lines at `\n` boundaries.
#[derive(Debug, Clone)]
pub struct LineBuffer {
    buffer: String,
    committed_lines: Vec<String>,
}

impl LineBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            committed_lines: Vec::new(),
        }
    }

    pub fn append(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].to_string();
            self.committed_lines.push(line);
            self.buffer = self.buffer[pos + 1..].to_string();
        }
    }

    #[must_use]
    pub fn committed(&self) -> &[String] {
        &self.committed_lines
    }

    #[must_use]
    pub fn pending(&self) -> &str {
        &self.buffer
    }

    pub fn commit_all(&mut self) {
        if !self.buffer.is_empty() {
            let remaining = std::mem::take(&mut self.buffer);
            self.committed_lines.push(remaining);
        }
    }

    #[must_use]
    pub fn full_text(&self) -> String {
        let mut text = self.committed_lines.join("\n");
        if !self.buffer.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&self.buffer);
        }
        text
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.committed_lines.len() + usize::from(!self.buffer.is_empty())
    }
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_initially() {
        let buf = LineBuffer::new();
        assert!(buf.committed().is_empty());
        assert!(buf.pending().is_empty());
    }

    #[test]
    fn single_chunk_no_newline() {
        let mut buf = LineBuffer::new();
        buf.append("hello");
        assert!(buf.committed().is_empty());
        assert_eq!(buf.pending(), "hello");
    }

    #[test]
    fn newline_commits() {
        let mut buf = LineBuffer::new();
        buf.append("line1\nline2");
        assert_eq!(buf.committed(), &["line1"]);
        assert_eq!(buf.pending(), "line2");
    }

    #[test]
    fn multiple_appends() {
        let mut buf = LineBuffer::new();
        buf.append("hel");
        buf.append("lo\nwor");
        buf.append("ld\n!");
        assert_eq!(buf.committed(), &["hello", "world"]);
        assert_eq!(buf.pending(), "!");
    }

    #[test]
    fn commit_all_flushes() {
        let mut buf = LineBuffer::new();
        buf.append("partial");
        buf.commit_all();
        assert_eq!(buf.committed(), &["partial"]);
        assert!(buf.pending().is_empty());
    }

    #[test]
    fn full_text_reconstruction() {
        let mut buf = LineBuffer::new();
        buf.append("a\nb\nc");
        assert_eq!(buf.full_text(), "a\nb\nc");
    }

    #[test]
    fn line_count() {
        let mut buf = LineBuffer::new();
        buf.append("a\nb\nc");
        assert_eq!(buf.line_count(), 3);
    }
}
