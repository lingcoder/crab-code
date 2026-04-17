//! Tool-filter matching — `matches_tool_filter`, `tool_name_matches_pattern`,
//! and the glob engine they share.
//!
//! Supports three filter formats:
//! - `*` — matches any tool
//! - `Bash` — plain tool name with glob metacharacters
//! - `Bash(key:pattern)` — tool-name + input-parameter-level constraint

/// Check whether a tool filter matches a tool invocation.
///
/// The `tool_input` is the JSON input object passed to the tool (used for
/// parameter-level matching like `Bash(command:git*)`).
pub fn matches_tool_filter(filter: &str, tool_name: &str, tool_input: &serde_json::Value) -> bool {
    let filter = filter.trim();

    // Wildcard: match everything
    if filter == "*" {
        return true;
    }

    // Check for Name(pattern) format
    if let Some(paren_start) = filter.find('(')
        && filter.ends_with(')')
    {
        let name_part = &filter[..paren_start];
        let pattern_part = &filter[paren_start + 1..filter.len() - 1];

        // Tool name must match
        if !glob_match(name_part, tool_name) {
            return false;
        }

        // Parse the parameter constraint: "key:pattern"
        if let Some(colon_pos) = pattern_part.find(':') {
            let key = &pattern_part[..colon_pos];
            let value_pattern = &pattern_part[colon_pos + 1..];

            // Look up the key in tool_input
            if let Some(value) = tool_input.get(key) {
                let value_str = match value {
                    serde_json::Value::String(s) => s.as_str(),
                    _ => return false,
                };
                return glob_match(value_pattern, value_str);
            }
            return false;
        }

        // No colon — just name match (weird format but handle gracefully)
        return true;
    }

    // Plain name match (may contain globs)
    glob_match(filter, tool_name)
}

pub(crate) fn tool_name_matches_pattern(pattern: &str, tool_name: &str) -> bool {
    glob_match(pattern, tool_name)
}

/// Simple glob matching supporting `*` (any chars), `?` (single char),
/// and `[abc]` (character class). Avoids pulling in globset for a small
/// pattern set used only in permission checks.
pub(crate) fn glob_match(pattern: &str, input: &str) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let input_chars: Vec<char> = input.chars().collect();
    glob_match_inner(&pat_chars, &input_chars)
}

fn glob_match_inner(pat: &[char], input: &[char]) -> bool {
    let (mut pi, mut ii) = (0, 0);
    let (mut star_pat, mut star_input) = (usize::MAX, usize::MAX);

    while ii < input.len() {
        if pi < pat.len() && pat[pi] == '[' {
            // Character class
            if let Some((matched, end)) = match_char_class(&pat[pi..], input[ii])
                && matched
            {
                pi += end;
                ii += 1;
                continue;
            }
            // No match in class -- fall through to star backtrack
            if star_pat != usize::MAX {
                pi = star_pat + 1;
                star_input += 1;
                ii = star_input;
                continue;
            }
            return false;
        } else if pi < pat.len() && (pat[pi] == '?' || pat[pi] == input[ii]) {
            pi += 1;
            ii += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pat = pi;
            star_input = ii;
            pi += 1;
        } else if star_pat != usize::MAX {
            pi = star_pat + 1;
            star_input += 1;
            ii = star_input;
        } else {
            return false;
        }
    }

    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

/// Match a `[abc]` or `[a-z]` character class. Returns (matched, consumed count) or `None` if malformed.
fn match_char_class(pat: &[char], ch: char) -> Option<(bool, usize)> {
    if pat.is_empty() || pat[0] != '[' {
        return None;
    }
    let mut i = 1;
    let mut matched = false;
    while i < pat.len() && pat[i] != ']' {
        if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
            if ch >= pat[i] && ch <= pat[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if ch == pat[i] {
                matched = true;
            }
            i += 1;
        }
    }
    if i < pat.len() && pat[i] == ']' {
        Some((matched, i + 1))
    } else {
        None // Malformed: no closing bracket
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_match_wildcard_patterns() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("f*r", "foobar"));
        assert!(!glob_match("foo*", "barfoo"));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("bash", "bash"));
        assert!(!glob_match("bash", "Bash"));
        assert!(!glob_match("bash", "bash_exec"));
    }

    #[test]
    fn glob_match_empty_pattern() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "nonempty"));
    }

    #[test]
    fn glob_match_multiple_stars() {
        assert!(glob_match("*foo*bar*", "xxxfooYYYbarZZZ"));
        assert!(!glob_match("*foo*bar*", "xxxbarYYYfooZZZ"));
    }

    #[test]
    fn glob_match_char_class_range() {
        assert!(glob_match("[a-z]_tool", "m_tool"));
        assert!(!glob_match("[a-z]_tool", "M_tool"));
        assert!(!glob_match("[a-z]_tool", "1_tool"));
    }

    #[test]
    fn glob_match_malformed_char_class() {
        assert!(!glob_match("[abc", "a"));
    }

    #[test]
    fn glob_match_star_at_start_and_end() {
        assert!(glob_match("*bash*", "bash"));
        assert!(glob_match("*bash*", "my_bash_tool"));
        assert!(!glob_match("*bash*", "ba_sh"));
    }

    #[test]
    fn glob_match_question_mark_only() {
        assert!(glob_match("?", "a"));
        assert!(!glob_match("?", "ab"));
        assert!(!glob_match("?", ""));
    }

    #[test]
    fn glob_match_complex_pattern() {
        assert!(glob_match("mcp__*__[a-z]*", "mcp__server__tool_name"));
        assert!(!glob_match("mcp__*__[a-z]*", "mcp__server__9tool"));
    }

    #[test]
    fn glob_match_consecutive_stars() {
        assert!(glob_match("**", "anything"));
        assert!(glob_match("a**b", "aXXXb"));
    }

    #[test]
    fn glob_match_char_class_single_char() {
        assert!(glob_match("[x]", "x"));
        assert!(!glob_match("[x]", "y"));
    }

    #[test]
    fn tool_filter_wildcard_matches_any() {
        let input = serde_json::json!({"command": "echo hello"});
        assert!(matches_tool_filter("*", "bash", &input));
        assert!(matches_tool_filter("*", "read", &input));
    }

    #[test]
    fn tool_filter_exact_name() {
        let input = serde_json::json!({});
        assert!(matches_tool_filter("bash", "bash", &input));
        assert!(!matches_tool_filter("bash", "read", &input));
        assert!(matches_tool_filter("Edit", "Edit", &input));
    }

    #[test]
    fn tool_filter_name_glob() {
        let input = serde_json::json!({});
        assert!(matches_tool_filter("mcp__*", "mcp__playwright", &input));
        assert!(!matches_tool_filter("mcp__*", "bash", &input));
    }

    #[test]
    fn tool_filter_name_with_param_pattern() {
        let input = serde_json::json!({"command": "git status"});
        assert!(matches_tool_filter("Bash(command:git*)", "Bash", &input));
        assert!(matches_tool_filter("Bash(command:git *)", "Bash", &input));

        let other = serde_json::json!({"command": "rm -rf /"});
        assert!(!matches_tool_filter("Bash(command:git*)", "Bash", &other));
    }

    #[test]
    fn tool_filter_param_wrong_tool_name() {
        let input = serde_json::json!({"command": "git log"});
        assert!(!matches_tool_filter("Bash(command:git*)", "read", &input));
    }

    #[test]
    fn tool_filter_param_missing_key() {
        let input = serde_json::json!({"file_path": "/tmp/foo"});
        assert!(!matches_tool_filter("Bash(command:git*)", "Bash", &input));
    }

    #[test]
    fn tool_filter_param_non_string_value() {
        let input = serde_json::json!({"command": 42});
        assert!(!matches_tool_filter("Bash(command:git*)", "Bash", &input));
    }

    #[test]
    fn tool_filter_case_sensitive() {
        let input = serde_json::json!({});
        assert!(!matches_tool_filter("bash", "Bash", &input));
        assert!(matches_tool_filter("Bash", "Bash", &input));
    }
}
