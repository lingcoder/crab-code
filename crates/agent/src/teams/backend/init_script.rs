//! Initialization script generation for tmux-based sub-agents.
//!
//! Produces a shell script string that configures environment variables and
//! launches `crab --agent-mode` inside a tmux pane.

use crate::teams::backend::teammate::TeammateConfig;

/// Escape a string for safe embedding inside single-quoted shell strings.
///
/// Replaces each `'` with `'\''` (end quote, escaped quote, restart quote).
#[must_use]
#[allow(dead_code)]
pub fn shell_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Generate a shell initialization script for a teammate.
///
/// The produced script:
/// 1. Exports `CRAB_AGENT_MODE=1`, `CRAB_TEAMMATE_NAME`, and `CRAB_TEAMMATE_ROLE`
/// 2. Exports any extra environment variables from the config
/// 3. Optionally `cd`s to the specified working directory
/// 4. Launches `crab --agent-mode --role <role>`
#[must_use]
#[allow(dead_code)]
pub fn generate_init_script(config: &TeammateConfig) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(8 + config.env_vars.len());

    lines.push("#!/usr/bin/env bash".to_owned());
    lines.push("set -euo pipefail".to_owned());
    lines.push(String::new());

    // Core environment variables
    lines.push("export CRAB_AGENT_MODE='1'".to_owned());
    lines.push(format!(
        "export CRAB_TEAMMATE_NAME='{}'",
        shell_escape(&config.name)
    ));
    lines.push(format!(
        "export CRAB_TEAMMATE_ROLE='{}'",
        shell_escape(&config.role)
    ));

    // Extra environment variables from config
    for (key, value) in &config.env_vars {
        lines.push(format!(
            "export {}='{}'",
            shell_escape(key),
            shell_escape(value)
        ));
    }

    lines.push(String::new());

    // Optional working directory
    if let Some(ref dir) = config.working_dir {
        lines.push(format!("cd '{}'", shell_escape(&dir.display().to_string())));
    }

    // Launch command
    lines.push(format!(
        "exec crab --agent-mode --role '{}'",
        shell_escape(&config.role)
    ));
    lines.push(String::new());

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn basic_init_script() {
        let config = TeammateConfig::new("Alice", "reviewer");
        let script = generate_init_script(&config);

        assert!(script.contains("#!/usr/bin/env bash"));
        assert!(script.contains("export CRAB_AGENT_MODE='1'"));
        assert!(script.contains("export CRAB_TEAMMATE_NAME='Alice'"));
        assert!(script.contains("export CRAB_TEAMMATE_ROLE='reviewer'"));
        assert!(script.contains("exec crab --agent-mode --role 'reviewer'"));
        // No cd when working_dir is None
        assert!(!script.contains("cd "));
    }

    #[test]
    fn init_script_with_working_dir() {
        let config = TeammateConfig::new("Bob", "tester")
            .with_working_dir(PathBuf::from("/home/user/project"));
        let script = generate_init_script(&config);

        assert!(script.contains("cd '/home/user/project'"));
        assert!(script.contains("exec crab --agent-mode --role 'tester'"));
    }

    #[test]
    fn init_script_shell_escaping() {
        let config =
            TeammateConfig::new("Eve's agent", "test'er").with_env("MY_VAR", "it's a value");
        let script = generate_init_script(&config);

        assert!(script.contains("CRAB_TEAMMATE_NAME='Eve'\\''s agent'"));
        assert!(script.contains("CRAB_TEAMMATE_ROLE='test'\\''er'"));
        assert!(script.contains("MY_VAR='it'\\''s a value'"));
    }

    #[test]
    fn init_script_with_extra_env_vars() {
        let config = TeammateConfig::new("Alice", "reviewer")
            .with_env("RUST_LOG", "debug")
            .with_env("TOKEN", "abc123");
        let script = generate_init_script(&config);

        assert!(script.contains("export RUST_LOG='debug'"));
        assert!(script.contains("export TOKEN='abc123'"));
    }

    #[test]
    fn shell_escape_no_quotes() {
        assert_eq!(shell_escape("hello"), "hello");
    }

    #[test]
    fn shell_escape_with_quotes() {
        assert_eq!(shell_escape("it's"), "it'\\''s");
    }

    #[test]
    fn shell_escape_multiple_quotes() {
        assert_eq!(shell_escape("a'b'c"), "a'\\''b'\\''c");
    }
}
