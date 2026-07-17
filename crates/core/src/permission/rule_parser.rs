//! Permission rule AST parser.
//!
//! Parses CC-grammar permission rules like `"Bash(git *)"`, `"Read(src/**)"`
//! into a structured AST, and matches tool invocations against those rules.
//! Also supports bash-specific command pattern parsing for shell rule matching.

use std::fmt;

use serde::{Deserialize, Serialize};

// Error types

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

// Core AST types

/// A parsed permission rule with tool name and optional content matcher.
///
/// # Examples
///
/// | Input string            | Parsed                                     |
/// |-------------------------|--------------------------------------------|
/// | `"Bash"`                | tool_name=`Bash`, content=`None`           |
/// | `"Bash(git *)"`         | tool_name=`Bash`, content=command glob     |
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
    /// Match a named argument's value using a glob pattern.
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
    /// Match a path-carrying argument (`file_path` / `path` /
    /// `notebook_path` / `pattern`) against a glob. Produced by CC-style
    /// file-tool rules like `Read(src/**)`.
    Path {
        /// The glob pattern applied to the path argument.
        pattern: String,
    },
    /// Match the host of a `url` argument (CC `WebFetch(domain:...)`).
    Domain {
        /// Domain pattern; glob syntax (`*.example.com`) is accepted.
        domain: String,
    },
    /// Match any invocation of the tool regardless of arguments.
    Any,
}

impl fmt::Display for RuleContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Glob { key, pattern } => write!(f, "{key}:{pattern}"),
            Self::Exact { key, value } => write!(f, "{key}={value}"),
            Self::Path { pattern } => write!(f, "{pattern}"),
            Self::Domain { domain } => write!(f, "domain:{domain}"),
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

// Parsing functions

/// Parse a permission rule string into a structured [`PermissionRule`].
///
/// # Supported formats (CC permission-rule grammar)
///
/// - `"*"` -- matches any tool
/// - `"ToolName"` -- exact tool name (or glob, e.g. `mcp__*`)
/// - `"Bash(git *)"` / `"Bash(npm run test:*)"` -- command patterns
/// - `"Read(src/**)"` -- path glob for file tools
/// - `"WebFetch(domain:example.com)"` -- URL host match
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

        let content = parse_rule_content(&tool_name, content_str)?;

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

/// Tools whose CC rule specifier is a command pattern.
const COMMAND_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// Tools whose CC rule specifier is a file-path glob.
const PATH_TOOLS: [&str; 7] = [
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Glob",
    "Grep",
];

/// Argument keys that may carry the target path for [`RuleContent::Path`].
pub(crate) const PATH_ARG_KEYS: [&str; 4] = ["file_path", "path", "notebook_path", "pattern"];

/// Parse the content inside parentheses of a permission rule.
///
/// The grammar is the CC permission-rule grammar, tool-aware:
///
/// - `Bash(git status)` / `Bash(git *)` / `Bash(npm run test:*)` — command
///   patterns (exact, glob, or `:*` prefix form)
/// - `Read(src/**)` — path glob applied to the tool's path argument
/// - `WebFetch(domain:example.com)` — URL host match
/// - `Skill(name)` — exact skill name
/// - `Tool(*)` — any invocation of the tool
fn parse_rule_content(tool_name: &str, content: &str) -> Result<RuleContent, ParseError> {
    let content = content.trim();

    if content == "*" {
        return Ok(RuleContent::Any);
    }

    if COMMAND_TOOLS.contains(&tool_name) {
        return Ok(parse_command_specifier(content));
    }

    if PATH_TOOLS.contains(&tool_name) {
        return Ok(RuleContent::Path {
            pattern: content.to_string(),
        });
    }

    if (tool_name == "WebFetch" || tool_name == "WebSearch")
        && let Some(domain) = content.strip_prefix("domain:")
    {
        return Ok(RuleContent::Domain {
            domain: domain.trim().to_string(),
        });
    }

    if tool_name == "Skill" {
        return Ok(RuleContent::Exact {
            key: "skill".to_string(),
            value: content.to_string(),
        });
    }

    Err(ParseError {
        input: content.to_string(),
        reason: format!("tool '{tool_name}' does not take a rule specifier"),
    })
}

/// Parse a CC command specifier for Bash-like tools.
///
/// `:*` suffix means prefix matching (`git commit:*` → any command starting
/// with `git commit`); glob characters make it a glob; otherwise exact.
fn parse_command_specifier(content: &str) -> RuleContent {
    if let Some(prefix) = content.strip_suffix(":*") {
        return RuleContent::Glob {
            key: "command".to_string(),
            pattern: format!("{}*", prefix.trim_end()),
        };
    }
    if content.contains(['*', '?']) {
        return RuleContent::Glob {
            key: "command".to_string(),
            pattern: content.to_string(),
        };
    }
    RuleContent::Exact {
        key: "command".to_string(),
        value: content.to_string(),
    }
}

/// Match a tool invocation against a parsed [`PermissionRule`].
///
/// Returns `true` if the rule matches the given tool name and input arguments.
pub fn matches_rule(rule: &PermissionRule, tool_name: &str, args: &serde_json::Value) -> bool {
    // First check tool name
    if !super::filter::glob_match(&rule.tool_name, tool_name) {
        return false;
    }

    // Then check content constraint
    match &rule.content {
        None | Some(RuleContent::Any) => true,
        Some(RuleContent::Glob { key, pattern }) => args
            .get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| super::filter::glob_match(pattern, s)),
        Some(RuleContent::Exact { key, value }) => args
            .get(key)
            .and_then(|v| v.as_str())
            .is_some_and(|s| s == value),
        Some(RuleContent::Path { pattern }) => PATH_ARG_KEYS.iter().any(|key| {
            args.get(*key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| super::filter::glob_match(pattern, s))
        }),
        Some(RuleContent::Domain { domain }) => args
            .get("url")
            .and_then(|v| v.as_str())
            .and_then(url_host)
            .is_some_and(|host| {
                super::filter::glob_match(domain, &host)
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.') || rest.is_empty())
            }),
    }
}

/// Extract the host from a URL string (`scheme://host[:port]/...`).
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host = host_port.rsplit_once(':').map_or(host_port, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host_port
        }
    });
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
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

// Tests

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
    fn parse_cc_bash_exact() {
        let rule = parse_rule("Bash(git status)").unwrap();
        assert_eq!(
            rule.content,
            Some(RuleContent::Exact {
                key: "command".into(),
                value: "git status".into(),
            })
        );
        assert!(matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "git status"})
        ));
        assert!(!matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "git push"})
        ));
    }

    #[test]
    fn parse_cc_bash_space_glob() {
        let rule = parse_rule("Bash(git *)").unwrap();
        assert!(matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "git status"})
        ));
        assert!(!matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "npm install"})
        ));
    }

    #[test]
    fn parse_cc_bash_colon_star_prefix() {
        // CC prefix form: `npm run test:*` matches commands starting with
        // "npm run test".
        let rule = parse_rule("Bash(npm run test:*)").unwrap();
        assert!(matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "npm run test:unit"})
        ));
        assert!(matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "npm run test"})
        ));
        assert!(!matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "npm run build"})
        ));
    }

    #[test]
    fn parse_cc_bash_bare_prefix_colon_star() {
        let rule = parse_rule("Bash(git:*)").unwrap();
        assert!(matches_rule(
            &rule,
            "Bash",
            &serde_json::json!({"command": "git push origin"})
        ));
    }

    #[test]
    fn parse_cc_file_path_glob() {
        let rule = parse_rule("Read(src/**)").unwrap();
        assert_eq!(
            rule.content,
            Some(RuleContent::Path {
                pattern: "src/**".into()
            })
        );
        assert!(matches_rule(
            &rule,
            "Read",
            &serde_json::json!({"file_path": "src/main.rs"})
        ));
        assert!(!matches_rule(
            &rule,
            "Read",
            &serde_json::json!({"file_path": "docs/readme.md"})
        ));
    }

    #[test]
    fn parse_cc_file_windows_path_not_keyed() {
        // A drive letter must not be mistaken for an explicit key.
        let rule = parse_rule("Read(C:/Users/**)").unwrap();
        assert!(matches!(rule.content, Some(RuleContent::Path { .. })));
    }

    #[test]
    fn parse_cc_webfetch_domain() {
        let rule = parse_rule("WebFetch(domain:example.com)").unwrap();
        assert_eq!(
            rule.content,
            Some(RuleContent::Domain {
                domain: "example.com".into()
            })
        );
        assert!(matches_rule(
            &rule,
            "WebFetch",
            &serde_json::json!({"url": "https://example.com/page"})
        ));
        assert!(matches_rule(
            &rule,
            "WebFetch",
            &serde_json::json!({"url": "https://api.example.com/v1"})
        ));
        assert!(!matches_rule(
            &rule,
            "WebFetch",
            &serde_json::json!({"url": "https://evil.com/example.com"})
        ));
    }

    #[test]
    fn parse_cc_skill_name_with_colon() {
        let rule = parse_rule("Skill(commit-commands:commit)").unwrap();
        assert_eq!(
            rule.content,
            Some(RuleContent::Exact {
                key: "skill".into(),
                value: "commit-commands:commit".into(),
            })
        );
    }

    #[test]
    fn url_host_extraction() {
        assert_eq!(
            url_host("https://example.com/x"),
            Some("example.com".into())
        );
        assert_eq!(
            url_host("https://Example.COM:8443/x?q=1"),
            Some("example.com".into())
        );
        assert_eq!(url_host("example.com/path"), Some("example.com".into()));
        assert_eq!(url_host(""), None);
    }

    #[test]
    fn parse_glob_content() {
        let rule = parse_rule("Bash(git*)").unwrap();
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
    fn parse_exact_path_content() {
        let rule = parse_rule("Edit(/src/main.rs)").unwrap();
        assert_eq!(rule.tool_name, "Edit");
        assert_eq!(
            rule.content,
            Some(RuleContent::Path {
                pattern: "/src/main.rs".to_string(),
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
        assert!(parse_rule("Bash(git *").is_err());
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
