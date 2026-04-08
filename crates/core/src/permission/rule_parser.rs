//! Permission rule AST parser.
//!
//! Maps to CCB `utils/permissions/permissionRuleParser.ts` + `shellRuleMatching.ts`.
//!
//! Parses permission rules like `"Bash(cmd:git*)"`, `"Edit(path:/src/*)"` into a
//! structured AST, and matches tool invocations against those rules. Also supports
//! bash-specific command pattern parsing for shell rule matching.

use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Error returned when a permission rule string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// The original input that failed to parse.
    pub input: String,
    /// Human-readable description of the problem.
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse rule '{}': {}", self.input, self.reason)
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Core AST types
// ---------------------------------------------------------------------------

/// A parsed permission rule with tool name and optional content matcher.
///
/// # Examples
///
/// | Input string            | Parsed                                     |
/// |-------------------------|--------------------------------------------|
/// | `"Bash"`                | tool_name=`Bash`, content=`None`           |
/// | `"Bash(command:git*)"` | tool_name=`Bash`, content=`Glob("git*")`   |
/// | `"mcp__*"`              | tool_name=`mcp__*`, content=`None`         |
/// | `"*"`                   | tool_name=`*`, content=`None`              |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Tool name or glob pattern (e.g. `"Bash"`, `"mcp__*"`, `"*"`).
    pub tool_name: String,
    /// Optional content matcher for parameter-level matching.
    pub content: Option<RuleContent>,
}

impl fmt::Display for PermissionRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.content {
            None => write!(f, "{}", self.tool_name),
            Some(content) => write!(f, "{}({})", self.tool_name, content),
        }
    }
}

/// The content portion of a rule -- a matcher applied to a tool's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleContent {
    /// Match argument value using a glob pattern (e.g. `command:git*`).
    Glob {
        /// The parameter key (e.g. `"command"`).
        key: String,
        /// The glob pattern (e.g. `"git*"`).
        pattern: String,
    },
    /// Match argument value exactly (e.g. `command=git status`).
    Exact {
        /// The parameter key.
        key: String,
        /// The exact value to match.
        value: String,
    },
    /// Match argument value against a regex (e.g. `command~/^git\s/`).
    Regex {
        /// The parameter key.
        key: String,
        /// The regex pattern string.
        pattern: String,
    },
    /// Match any invocation of the tool regardless of arguments.
    Any,
}

impl fmt::Display for RuleContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Glob { key, pattern } => write!(f, "{key}:{pattern}"),
            Self::Exact { key, value } => write!(f, "{key}={value}"),
            Self::Regex { key, pattern } => write!(f, "{key}~{pattern}"),
            Self::Any => write!(f, "*"),
        }
    }
}

/// A parsed bash command pattern for shell-specific rule matching.
///
/// E.g. `"git *"` -> command=`"git"`, `args_glob`=`Some("*")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashPattern {
    /// The base command (e.g. `"git"`, `"npm"`).
    pub command: String,
    /// Optional glob pattern for command arguments.
    pub args_glob: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing functions
// ---------------------------------------------------------------------------

/// Parse a permission rule string into a structured [`PermissionRule`].
///
/// # Supported formats
///
/// - `"*"` -- matches any tool
/// - `"ToolName"` -- exact tool name (or glob)
/// - `"ToolName(key:pattern)"` -- tool name + glob parameter match
/// - `"ToolName(key=value)"` -- tool name + exact parameter match
/// - `"ToolName(key~/regex/)"` -- tool name + regex parameter match
/// - `"ToolName(*)"` -- tool name + match-any arguments
///
/// # Errors
///
/// Returns [`ParseError`] if the input is empty or has mismatched parentheses.
pub fn parse_rule(input: &str) -> Result<PermissionRule, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError {
            input: input.to_string(),
            reason: "empty rule string".to_string(),
        });
    }

    // Check for Name(content) format
    if let Some(paren_start) = input.find('(') {
        if !input.ends_with(')') {
            return Err(ParseError {
                input: input.to_string(),
                reason: "opening '(' without closing ')'".to_string(),
            });
        }

        let tool_name = input[..paren_start].to_string();
        let content_str = &input[paren_start + 1..input.len() - 1];

        if tool_name.is_empty() {
            return Err(ParseError {
                input: input.to_string(),
                reason: "empty tool name before '('".to_string(),
            });
        }

        let content = parse_rule_content(content_str)?;

        Ok(PermissionRule {
            tool_name,
            content: Some(content),
        })
    } else {
        // Plain tool name / glob
        Ok(PermissionRule {
            tool_name: input.to_string(),
            content: None,
        })
    }
}

/// Parse the content inside parentheses of a permission rule.
fn parse_rule_content(content: &str) -> Result<RuleContent, ParseError> {
    let content = content.trim();

    // Wildcard any
    if content == "*" {
        return Ok(RuleContent::Any);
    }

    // Try exact match: key=value
    if let Some(eq_pos) = content.find('=') {
        let key = content[..eq_pos].trim().to_string();
        let value = content[eq_pos + 1..].trim().to_string();
        if key.is_empty() {
            return Err(ParseError {
                input: content.to_string(),
                reason: "empty key in exact match".to_string(),
            });
        }
        return Ok(RuleContent::Exact { key, value });
    }

    // Try regex match: key~/pattern/
    if let Some(tilde_pos) = content.find('~') {
        let key = content[..tilde_pos].trim().to_string();
        let pattern = content[tilde_pos + 1..].trim().to_string();
        if key.is_empty() {
            return Err(ParseError {
                input: content.to_string(),
                reason: "empty key in regex match".to_string(),
            });
        }
        return Ok(RuleContent::Regex { key, pattern });
    }

    // Try glob match: key:pattern
    if let Some(colon_pos) = content.find(':') {
        let key = content[..colon_pos].trim().to_string();
        let pattern = content[colon_pos + 1..].trim().to_string();
        if key.is_empty() {
            return Err(ParseError {
                input: content.to_string(),
                reason: "empty key in glob match".to_string(),
            });
        }
        return Ok(RuleContent::Glob { key, pattern });
    }

    // Fallback: treat as Any with a note
    Err(ParseError {
        input: content.to_string(),
        reason: "unrecognized content format; expected 'key:pattern', 'key=value', 'key~/regex/', or '*'".to_string(),
    })
}

/// Match a tool invocation against a parsed [`PermissionRule`].
///
/// Returns `true` if the rule matches the given tool name and input arguments.
pub fn matches_rule(rule: &PermissionRule, tool_name: &str, args: &serde_json::Value) -> bool {
    // First check tool name
    if !super::glob_match(&rule.tool_name, tool_name) {
        return false;
    }

    // Then check content constraint
    match &rule.content {
        None | Some(RuleContent::Any) => true,
        Some(RuleContent::Glob { key, pattern }) => args
            .get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| super::glob_match(pattern, s)),
        Some(RuleContent::Exact { key, value }) => args
            .get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| s == value),
        Some(RuleContent::Regex { key, pattern }) => {
            // Strip optional surrounding slashes for convenience
            let pat = pattern
                .strip_prefix('/')
                .and_then(|p| p.strip_suffix('/'))
                .unwrap_or(pattern);
            let Ok(re) = regex::Regex::new(pat) else {
                return false;
            };
            args.get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| re.is_match(s))
        }
    }
}

/// Parse a bash command pattern for shell rule matching.
///
/// Splits a bash pattern string like `"git *"` into the base command and
/// an optional glob for the arguments.
///
/// # Examples
///
/// ```ignore
/// let p = parse_bash_pattern("git *")?;
/// assert_eq!(p.command, "git");
/// assert_eq!(p.args_glob, Some("*".to_string()));
/// ```
pub fn parse_bash_pattern(pattern: &str) -> Result<BashPattern, ParseError> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(ParseError {
            input: pattern.to_string(),
            reason: "empty bash pattern".to_string(),
        });
    }

    // Split on first whitespace
    if let Some(space_pos) = pattern.find(char::is_whitespace) {
        let command = pattern[..space_pos].to_string();
        let args_glob = pattern[space_pos..].trim().to_string();
        Ok(BashPattern {
            command,
            args_glob: if args_glob.is_empty() {
                None
            } else {
                Some(args_glob)
            },
        })
    } else {
        Ok(BashPattern {
            command: pattern.to_string(),
            args_glob: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wildcard_rule() {
        let rule = parse_rule("*").unwrap();
        assert_eq!(rule.tool_name, "*");
        assert!(rule.content.is_none());
    }

    #[test]
    fn parse_plain_tool_name() {
        let rule = parse_rule("Bash").unwrap();
        assert_eq!(rule.tool_name, "Bash");
        assert!(rule.content.is_none());
    }

    #[test]
    fn parse_glob_content() {
        let rule = parse_rule("Bash(command:git*)").unwrap();
        assert_eq!(rule.tool_name, "Bash");
        assert_eq!(
            rule.content,
            Some(RuleContent::Glob {
                key: "command".to_string(),
                pattern: "git*".to_string(),
            })
        );
    }

    #[test]
    fn parse_exact_content() {
        let rule = parse_rule("Edit(path=/src/main.rs)").unwrap();
        assert_eq!(rule.tool_name, "Edit");
        assert_eq!(
            rule.content,
            Some(RuleContent::Exact {
                key: "path".to_string(),
                value: "/src/main.rs".to_string(),
            })
        );
    }

    #[test]
    fn parse_any_content() {
        let rule = parse_rule("Bash(*)").unwrap();
        assert_eq!(rule.tool_name, "Bash");
        assert_eq!(rule.content, Some(RuleContent::Any));
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_rule("").is_err());
        assert!(parse_rule("  ").is_err());
    }

    #[test]
    fn parse_mismatched_parens() {
        assert!(parse_rule("Bash(command:git*").is_err());
    }

    #[test]
    fn parse_bash_pattern_with_args() {
        let p = parse_bash_pattern("git *").unwrap();
        assert_eq!(p.command, "git");
        assert_eq!(p.args_glob, Some("*".to_string()));
    }

    #[test]
    fn parse_bash_pattern_no_args() {
        let p = parse_bash_pattern("ls").unwrap();
        assert_eq!(p.command, "ls");
        assert!(p.args_glob.is_none());
    }

    #[test]
    fn parse_bash_pattern_empty_fails() {
        assert!(parse_bash_pattern("").is_err());
    }
}
