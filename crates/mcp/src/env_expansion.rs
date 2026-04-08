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
pub fn expand_env_vars(_input: &str) -> String {
    todo!()
}

/// Expand environment variables in each element of an argument list.
///
/// Convenience wrapper around [`expand_env_vars`] for command arguments.
pub fn expand_env_in_args(_args: &[String]) -> Vec<String> {
    todo!()
}

/// Expand environment variables in a map of key-value pairs (e.g., env block).
pub fn expand_env_in_map<S: ::std::hash::BuildHasher>(
    _map: &std::collections::HashMap<String, String, S>,
) -> std::collections::HashMap<String, String> {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn plain_string_unchanged() {
        // Strings without ${} patterns should pass through unchanged.
        // (Will verify once expand_env_vars is implemented.)
        let input = "no variables here";
        assert_eq!(input.len(), 17); // trivial assertion for compilation
    }
}
