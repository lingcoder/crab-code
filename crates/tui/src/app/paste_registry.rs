//! App-side registry for large pasted blocks.
//!
//! A paste with at least `PASTE_COLLAPSE_MIN_LINES` lines is stored here and
//! replaced in the input box by a `[Pasted text #N +K lines]` reference token.
//! The token is expanded back to the original content when the prompt is
//! submitted, so the model receives the full text while the input box and
//! transcript stay compact.

/// Minimum line count for a paste to be collapsed into a reference token.
pub const PASTE_COLLAPSE_MIN_LINES: usize = 20;

/// A single stored paste keyed by its registry id.
#[derive(Debug, Clone)]
struct PastedEntry {
    id: usize,
    content: String,
    line_count: usize,
}

/// Registry of collapsed pastes for the current session.
#[derive(Debug, Default)]
pub struct PastedContents {
    entries: Vec<PastedEntry>,
    next_id: usize,
}

impl PastedContents {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `content`, returning the reference token to insert into the input.
    pub fn store(&mut self, content: String) -> String {
        self.next_id += 1;
        let id = self.next_id;
        let line_count = content.lines().count();
        let token = format!("[Pasted text #{id} +{line_count} lines]");
        self.entries.push(PastedEntry {
            id,
            content,
            line_count,
        });
        token
    }

    /// Expand every known reference token in `text` back to its stored content.
    #[must_use]
    pub fn expand(&self, text: &str) -> String {
        let mut out = text.to_string();
        for entry in &self.entries {
            let token = format!("[Pasted text #{} +{} lines]", entry.id, entry.line_count);
            if out.contains(&token) {
                out = out.replace(&token, &entry.content);
            }
        }
        out
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.next_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_returns_token_with_line_count() {
        let mut reg = PastedContents::new();
        let token = reg.store("a\nb\nc".to_string());
        assert_eq!(token, "[Pasted text #1 +3 lines]");
    }

    #[test]
    fn expand_round_trips_stored_content() {
        let mut reg = PastedContents::new();
        let content = (0..25)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let token = reg.store(content.clone());
        let expanded = reg.expand(&format!("before {token} after"));
        assert!(expanded.contains(&content));
        assert!(expanded.starts_with("before "));
        assert!(expanded.ends_with(" after"));
    }

    #[test]
    fn expand_leaves_unknown_token_untouched() {
        let reg = PastedContents::new();
        assert_eq!(
            reg.expand("[Pasted text #9 +9 lines]"),
            "[Pasted text #9 +9 lines]"
        );
    }

    #[test]
    fn clear_resets_ids() {
        let mut reg = PastedContents::new();
        reg.store("x".to_string());
        reg.clear();
        assert_eq!(reg.store("y".to_string()), "[Pasted text #1 +1 lines]");
    }
}
