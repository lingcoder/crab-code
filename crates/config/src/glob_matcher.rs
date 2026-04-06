//! High-performance glob matching for permission patterns.
//!
//! Supports `*` (any chars), `**` (any path segments), `?` (single char),
//! `[abc]` (character class), and `{a,b}` (alternation).
//!
//! [`GlobMatcher`] compiles a single pattern; [`GlobSet`] matches against
//! multiple patterns simultaneously.

use serde::{Deserialize, Serialize};

// ── GlobMatcher ────────────────────────────────────────────────────

/// A compiled glob pattern for efficient repeated matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobMatcher {
    pattern: String,
    /// Expanded alternatives (from `{a,b}` syntax). If no braces, contains
    /// just the original pattern.
    alternatives: Vec<String>,
}

impl GlobMatcher {
    /// Compile a glob pattern.
    #[must_use]
    pub fn new(pattern: &str) -> Self {
        let alternatives = expand_braces(pattern);
        Self {
            pattern: pattern.to_owned(),
            alternatives,
        }
    }

    /// The original pattern string.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Test whether `input` matches this glob.
    #[must_use]
    pub fn is_match(&self, input: &str) -> bool {
        self.alternatives.iter().any(|alt| glob_match(alt, input))
    }
}

// ── GlobSet ────────────────────────────────────────────────────────

/// A set of glob patterns — matches if **any** pattern matches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobSet {
    matchers: Vec<GlobMatcher>,
}

impl GlobSet {
    /// Create an empty glob set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            matchers: Vec::new(),
        }
    }

    /// Add a glob pattern.
    pub fn add(&mut self, pattern: &str) {
        self.matchers.push(GlobMatcher::new(pattern));
    }

    /// Create from a slice of patterns.
    #[must_use]
    pub fn from_patterns(patterns: &[&str]) -> Self {
        let matchers = patterns.iter().map(|p| GlobMatcher::new(p)).collect();
        Self { matchers }
    }

    /// Test whether `input` matches **any** pattern in the set.
    #[must_use]
    pub fn is_match(&self, input: &str) -> bool {
        self.matchers.iter().any(|m| m.is_match(input))
    }

    /// Return all patterns that match `input`.
    #[must_use]
    pub fn matching_patterns(&self, input: &str) -> Vec<&str> {
        self.matchers
            .iter()
            .filter(|m| m.is_match(input))
            .map(GlobMatcher::pattern)
            .collect()
    }

    /// Number of patterns.
    #[must_use]
    pub fn len(&self) -> usize {
        self.matchers.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.matchers.is_empty()
    }
}

// ── Core matching ──────────────────────────────────────────────────

/// Match a single glob alternative (no `{…}` braces) against `input`.
fn glob_match(pattern: &str, input: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let inp: Vec<char> = input.chars().collect();
    do_match(&pat, 0, &inp, 0)
}

/// Recursive matcher with backtracking for `*` and `**`.
fn do_match(pat: &[char], mut pi: usize, inp: &[char], mut ii: usize) -> bool {
    let (mut star_pat, mut star_inp) = (usize::MAX, usize::MAX);

    while ii < inp.len() {
        if pi < pat.len() && pat[pi] == '[' {
            // Character class
            if let Some((matched, end)) = match_char_class(pat, pi, inp[ii])
                && matched
            {
                pi = end;
                ii += 1;
                continue;
            }
            // No match in char class — try backtrack
            if star_pat != usize::MAX {
                pi = star_pat + 1;
                star_inp += 1;
                ii = star_inp;
                continue;
            }
            return false;
        }

        if pi + 1 < pat.len() && pat[pi] == '*' && pat[pi + 1] == '*' {
            // `**` matches any number of path segments
            star_pat = pi;
            star_inp = ii;
            pi += 2;
            // Skip optional separator after **
            if pi < pat.len() && (pat[pi] == '/' || pat[pi] == '\\') {
                pi += 1;
            }
            continue;
        }

        if pi < pat.len() && pat[pi] == '*' {
            // `*` matches any chars except path separators (for simple tool names
            // there are no separators, so this effectively matches everything)
            star_pat = pi;
            star_inp = ii;
            pi += 1;
            continue;
        }

        if pi < pat.len() && pat[pi] == '?' {
            pi += 1;
            ii += 1;
            continue;
        }

        if pi < pat.len() && pat[pi] == inp[ii] {
            pi += 1;
            ii += 1;
            continue;
        }

        // Mismatch — backtrack to last `*` or `**`
        if star_pat != usize::MAX {
            pi = star_pat + 1;
            // For ** skip the second *
            if star_pat + 1 < pat.len() && pat[star_pat + 1] == '*' {
                pi = star_pat + 2;
                if pi < pat.len() && (pat[pi] == '/' || pat[pi] == '\\') {
                    pi += 1;
                }
            }
            star_inp += 1;
            ii = star_inp;
            continue;
        }

        return false;
    }

    // Consume trailing `*` or `**` in pattern
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }

    pi == pat.len()
}

/// Match a `[abc]` or `[!abc]` character class at position `pi`.
///
/// Returns `(matched, end_index)` where `end_index` is the position after `]`.
fn match_char_class(pat: &[char], pi: usize, ch: char) -> Option<(bool, usize)> {
    if pi >= pat.len() || pat[pi] != '[' {
        return None;
    }

    let mut i = pi + 1;
    let negated = i < pat.len() && (pat[i] == '!' || pat[i] == '^');
    if negated {
        i += 1;
    }

    let mut matched = false;
    while i < pat.len() && pat[i] != ']' {
        // Range: a-z
        if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
            let lo = pat[i];
            let hi = pat[i + 2];
            if ch >= lo && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if pat[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }

    if i < pat.len() && pat[i] == ']' {
        if negated {
            matched = !matched;
        }
        Some((matched, i + 1))
    } else {
        None // Unclosed bracket — treat as literal (no match)
    }
}

// ── Brace expansion ────────────────────────────────────────────────

/// Expand `{a,b,c}` alternatives into a list of plain patterns.
///
/// Handles one level of braces. Nested braces are not expanded.
fn expand_braces(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();

    // Find first `{`
    let Some(open) = chars.iter().position(|&c| c == '{') else {
        return vec![pattern.to_owned()];
    };

    // Find matching `}`
    let mut depth = 0;
    let mut close = None;
    for (i, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }

    let Some(close) = close else {
        return vec![pattern.to_owned()]; // Unmatched brace
    };

    // Split alternatives by `,` (only at depth 1)
    let inner = &chars[open + 1..close];
    let mut alternatives = Vec::new();
    let mut current = String::new();
    let mut d = 0;
    for &c in inner {
        match c {
            '{' => {
                d += 1;
                current.push(c);
            }
            '}' => {
                d -= 1;
                current.push(c);
            }
            ',' if d == 0 => {
                alternatives.push(current.clone());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    alternatives.push(current);

    let prefix: String = chars[..open].iter().collect();
    let suffix: String = chars[close + 1..].iter().collect();

    alternatives
        .into_iter()
        .map(|alt| format!("{prefix}{alt}{suffix}"))
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic patterns ─────────────────────────────────────

    #[test]
    fn exact_match() {
        let m = GlobMatcher::new("bash");
        assert!(m.is_match("bash"));
        assert!(!m.is_match("read"));
    }

    #[test]
    fn star_match() {
        let m = GlobMatcher::new("mcp__*");
        assert!(m.is_match("mcp__playwright"));
        assert!(m.is_match("mcp__"));
        assert!(!m.is_match("bash"));
    }

    #[test]
    fn star_middle() {
        let m = GlobMatcher::new("pre*suf");
        assert!(m.is_match("pre_mid_suf"));
        assert!(m.is_match("presuf"));
        assert!(!m.is_match("prefix"));
    }

    #[test]
    fn question_mark() {
        let m = GlobMatcher::new("tool_?");
        assert!(m.is_match("tool_a"));
        assert!(!m.is_match("tool_ab"));
        assert!(!m.is_match("tool_"));
    }

    #[test]
    fn double_star() {
        let m = GlobMatcher::new("src/**/*.rs");
        assert!(m.is_match("src/main.rs"));
        assert!(m.is_match("src/lib/mod.rs"));
        assert!(!m.is_match("tests/test.rs"));
    }

    // ── Character classes ──────────────────────────────────

    #[test]
    fn char_class_list() {
        let m = GlobMatcher::new("[abc]_tool");
        assert!(m.is_match("a_tool"));
        assert!(m.is_match("b_tool"));
        assert!(!m.is_match("d_tool"));
    }

    #[test]
    fn char_class_range() {
        let m = GlobMatcher::new("[a-z]_tool");
        assert!(m.is_match("m_tool"));
        assert!(!m.is_match("A_tool"));
    }

    #[test]
    fn char_class_negated() {
        let m = GlobMatcher::new("[!0-9]x");
        assert!(m.is_match("ax"));
        assert!(!m.is_match("1x"));
    }

    #[test]
    fn char_class_caret_negation() {
        let m = GlobMatcher::new("[^abc]");
        assert!(m.is_match("d"));
        assert!(!m.is_match("a"));
    }

    // ── Brace expansion ────────────────────────────────────

    #[test]
    fn brace_expansion() {
        let m = GlobMatcher::new("{read,write}_file");
        assert!(m.is_match("read_file"));
        assert!(m.is_match("write_file"));
        assert!(!m.is_match("edit_file"));
    }

    #[test]
    fn brace_with_star() {
        let m = GlobMatcher::new("mcp__{a,b}*");
        assert!(m.is_match("mcp__alpha"));
        assert!(m.is_match("mcp__beta"));
        assert!(!m.is_match("mcp__click"));
    }

    #[test]
    fn no_braces() {
        let expanded = expand_braces("simple");
        assert_eq!(expanded, vec!["simple"]);
    }

    #[test]
    fn unmatched_brace() {
        let expanded = expand_braces("{unclosed");
        assert_eq!(expanded, vec!["{unclosed"]);
    }

    #[test]
    fn three_alternatives() {
        let expanded = expand_braces("x{a,b,c}y");
        assert_eq!(expanded, vec!["xay", "xby", "xcy"]);
    }

    // ── GlobSet ────────────────────────────────────────────

    #[test]
    fn globset_empty() {
        let gs = GlobSet::new();
        assert!(gs.is_empty());
        assert!(!gs.is_match("anything"));
    }

    #[test]
    fn globset_add_and_match() {
        let mut gs = GlobSet::new();
        gs.add("bash");
        gs.add("mcp__*");
        assert!(gs.is_match("bash"));
        assert!(gs.is_match("mcp__tool"));
        assert!(!gs.is_match("read"));
    }

    #[test]
    fn globset_from_patterns() {
        let gs = GlobSet::from_patterns(&["*.rs", "*.toml"]);
        assert_eq!(gs.len(), 2);
        assert!(gs.is_match("main.rs"));
        assert!(gs.is_match("Cargo.toml"));
        assert!(!gs.is_match("readme.md"));
    }

    #[test]
    fn globset_matching_patterns() {
        let gs = GlobSet::from_patterns(&["bash", "mcp__*", "*_tool"]);
        let matches = gs.matching_patterns("mcp__tool");
        assert!(matches.contains(&"mcp__*"));
        assert!(!matches.contains(&"bash"));
    }

    // ── GlobMatcher serde ──────────────────────────────────

    #[test]
    fn matcher_serde_roundtrip() {
        let m = GlobMatcher::new("mcp__*");
        let json = serde_json::to_string(&m).unwrap();
        let back: GlobMatcher = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pattern(), "mcp__*");
        assert!(back.is_match("mcp__test"));
    }

    #[test]
    fn globset_serde_roundtrip() {
        let gs = GlobSet::from_patterns(&["a*", "b*"]);
        let json = serde_json::to_string(&gs).unwrap();
        let back: GlobSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 2);
        assert!(back.is_match("abc"));
    }

    // ── Edge cases ─────────────────────────────────────────

    #[test]
    fn empty_pattern() {
        let m = GlobMatcher::new("");
        assert!(m.is_match(""));
        assert!(!m.is_match("a"));
    }

    #[test]
    fn star_only() {
        let m = GlobMatcher::new("*");
        assert!(m.is_match("anything"));
        assert!(m.is_match(""));
    }

    #[test]
    fn double_star_only() {
        let m = GlobMatcher::new("**");
        assert!(m.is_match("any/path/here"));
        assert!(m.is_match(""));
    }

    #[test]
    fn pattern_accessor() {
        let m = GlobMatcher::new("test_*");
        assert_eq!(m.pattern(), "test_*");
    }
}
