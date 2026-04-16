//! Environment variable expansion in MCP configs.
//!
//! Resolves `${VAR}` patterns in configuration strings, allowing users to
//! reference environment variables in MCP server arguments, URLs, and
//! other configuration values without hardcoding secrets.
//!
//! Supports:
//! - `${VAR}` — expand to the value of `VAR`, empty string if unset
//! - `${VAR:-default}` — expand to `VAR` or `default` if unset

/// Expand `${VAR}` and `${VAR:-default}` patterns in a string.
///
/// Unresolved variables (not set, no default) expand to an empty string.
/// Literal `$` can be escaped as `$$`.
pub fn expand_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' {
            if i + 1 < chars.len() && chars[i + 1] == '$' {
                // Escaped $$ → literal $
                result.push('$');
                i += 2;
            } else if i + 1 < chars.len() && chars[i + 1] == '{' {
                // ${VAR} or ${VAR:-default}
                if let Some(close) = input[i + 2..].find('}') {
                    let inner = &input[i + 2..i + 2 + close];
                    let (var_name, default_val) = if let Some(sep) = inner.find(":-") {
                        (&inner[..sep], Some(&inner[sep + 2..]))
                    } else {
                        (inner, None)
                    };

                    match std::env::var(var_name) {
                        Ok(val) => result.push_str(&val),
                        Err(_) => {
                            if let Some(def) = default_val {
                                result.push_str(def);
                            }
                        }
                    }

                    i += 2 + close + 1; // skip past }
                } else {
                    // No closing } — keep literal
                    result.push(chars[i]);
                    i += 1;
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Expand environment variables in each element of an argument list.
pub fn expand_env_in_args(args: &[String]) -> Vec<String> {
    args.iter().map(|a| expand_env_vars(a)).collect()
}

/// Expand environment variables in a map of key-value pairs (e.g., env block).
pub fn expand_env_in_map<S: ::std::hash::BuildHasher>(
    map: &std::collections::HashMap<String, String, S>,
) -> std::collections::HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), expand_env_vars(v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_string_unchanged() {
        assert_eq!(expand_env_vars("no variables here"), "no variables here");
    }

    #[test]
    fn expand_path_var_exists() {
        // PATH is always set on all platforms
        let result = expand_env_vars("${PATH}");
        assert!(!result.is_empty());
    }

    #[test]
    fn expand_missing_var_empty() {
        assert_eq!(expand_env_vars("${CRAB_XYZZY_NONEXISTENT_12345}"), "");
    }

    #[test]
    fn expand_with_default() {
        assert_eq!(
            expand_env_vars("${CRAB_XYZZY_NO_EXIST:-fallback}"),
            "fallback"
        );
    }

    #[test]
    fn escaped_dollar() {
        assert_eq!(expand_env_vars("$$HOME"), "$HOME");
    }

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)]
    fn mixed_text_and_default() {
        assert_eq!(
            expand_env_vars("http://localhost:${CRAB_NOPORT:-8080}/api"),
            "http://localhost:8080/api",
        );
    }

    #[test]
    fn expand_args_with_defaults() {
        let args = vec!["--flag=${CRAB_NOARG:-val}".into(), "plain".into()];
        let expanded = expand_env_in_args(&args);
        assert_eq!(expanded[0], "--flag=val");
        assert_eq!(expanded[1], "plain");
    }

    #[test]
    fn expand_map_with_defaults() {
        let mut map = std::collections::HashMap::new();
        map.insert("key".into(), "${CRAB_NOMAP:-expanded}".into());
        let result = expand_env_in_map(&map);
        assert_eq!(result["key"], "expanded");
    }

    #[test]
    fn unclosed_brace_kept_literal() {
        assert_eq!(expand_env_vars("${UNCLOSED"), "${UNCLOSED");
    }
}
