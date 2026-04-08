//! Bash command classifier for permission decisions.
//!
//! Classifies shell commands by risk level and category to support the
//! permission system's auto-allow/deny decisions. Used by `BashTool` and the
//! permission layer to determine whether a command requires user confirmation.
//!
//! Maps to Claude Code's `bashClassifier.ts` + `shellRuleMatching.ts`.
//!
//! This is NOT a `Tool` — it is a utility module consumed by `BashTool` and
//! the permission subsystem.

use std::path::PathBuf;

// ─── Classification types ───────────────────────────────────────────────

/// Classification of a bash command's risk level and type.
///
/// Categories are ordered roughly from least to most dangerous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    /// Read-only commands: `ls`, `cat`, `grep`, `find`, `git status`, etc.
    ReadOnly,
    /// Commands that write files: `echo >file`, `cp`, `mv`, `rm`, `tee`, etc.
    FileWrite,
    /// Git operations: `git add`, `git commit`, `git push`, etc.
    GitOperation,
    /// Network-accessing commands: `curl`, `wget`, `ssh`, `nc`, etc.
    NetworkAccess,
    /// Process control commands: `kill`, `pkill`, `killall`, etc.
    ProcessControl,
    /// Package managers: `npm`, `pip`, `cargo`, `apt`, `brew`, etc.
    PackageManager,
    /// Extremely dangerous commands: `rm -rf /`, `dd`, `mkfs`, `:(){:|:&};:`, etc.
    Dangerous,
    /// Could not classify — treat with caution.
    Unknown,
}

impl CommandCategory {
    /// Human-readable label for this category.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::FileWrite => "file-write",
            Self::GitOperation => "git-operation",
            Self::NetworkAccess => "network-access",
            Self::ProcessControl => "process-control",
            Self::PackageManager => "package-manager",
            Self::Dangerous => "dangerous",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for CommandCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Result of classifying a bash command.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// The category this command falls into.
    pub category: CommandCategory,
    /// Whether the command is potentially destructive (may delete or corrupt data).
    pub is_destructive: bool,
    /// File paths affected by the command (best-effort extraction).
    pub affected_paths: Vec<String>,
    /// Human-readable description of what the command does.
    pub description: String,
}

impl ClassificationResult {
    /// Whether this command is safe to auto-allow without user confirmation.
    #[must_use]
    pub fn is_safe_to_auto_allow(&self) -> bool {
        self.category == CommandCategory::ReadOnly && !self.is_destructive
    }
}

// ─── Read-only command patterns ─────────────────────────────────────────

/// Commands that are always read-only (no side effects).
const READ_ONLY_COMMANDS: &[&str] = &[
    "ls",
    "ll",
    "la",
    "dir",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "grep",
    "rg",
    "ag",
    "ack",
    "find",
    "fd",
    "locate",
    "which",
    "whereis",
    "type",
    "file",
    "stat",
    "du",
    "df",
    "free",
    "top",
    "htop",
    "ps",
    "who",
    "whoami",
    "id",
    "hostname",
    "uname",
    "date",
    "cal",
    "echo",
    "printf",
    "pwd",
    "env",
    "printenv",
    "set",
    "test",
    "true",
    "false",
    "seq",
    "sort",
    "uniq",
    "tr",
    "cut",
    "paste",
    "diff",
    "comm",
    "jq",
    "yq",
    "xxd",
    "hexdump",
    "od",
    "strings",
    "readlink",
    "realpath",
    "basename",
    "dirname",
    "sha256sum",
    "sha1sum",
    "md5sum",
    "cksum",
];

/// Git sub-commands that are read-only.
const READ_ONLY_GIT_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "tag",
    "describe",
    "rev-parse",
    "rev-list",
    "ls-files",
    "ls-tree",
    "ls-remote",
    "remote",
    "config",
    "stash list",
    "blame",
    "shortlog",
    "reflog",
    "name-rev",
    "cat-file",
];

// ─── Dangerous patterns ─────────────────────────────────────────────────

/// Commands or patterns that are always dangerous.
const DANGEROUS_COMMANDS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "dd",
    "mkfs",
    "fdisk",
    "parted",
    "wipefs",
    "shred",
    ":(){:|:&};:",
    "chmod -R 777 /",
    "chown -R",
];

// ─── Network commands ───────────────────────────────────────────────────

/// Commands that access the network.
const NETWORK_COMMANDS: &[&str] = &[
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "nc",
    "ncat",
    "netcat",
    "telnet",
    "ftp",
    "ping",
    "traceroute",
    "dig",
    "nslookup",
    "host",
    "nmap",
    "openssl",
];

// ─── Process control commands ───────────────────────────────────────────

/// Commands that control processes.
const PROCESS_CONTROL_COMMANDS: &[&str] = &[
    "kill",
    "pkill",
    "killall",
    "xkill",
    "reboot",
    "shutdown",
    "halt",
    "poweroff",
    "systemctl",
    "service",
];

// ─── Package manager commands ───────────────────────────────────────────

/// Package manager commands.
const PACKAGE_MANAGER_COMMANDS: &[&str] = &[
    "npm", "npx", "yarn", "pnpm", "bun", "pip", "pip3", "pipx", "cargo", "rustup", "apt",
    "apt-get", "yum", "dnf", "pacman", "brew", "gem", "go", "composer", "nuget", "dotnet",
];

// ─── File-write commands ────────────────────────────────────────────────

/// Commands that typically write files.
const FILE_WRITE_COMMANDS: &[&str] = &[
    "cp", "mv", "rm", "mkdir", "rmdir", "touch", "ln", "install", "chmod", "chown", "chgrp",
    "truncate", "tee",
];

// ─── Public API ─────────────────────────────────────────────────────────

/// Classify a bash command string by risk level and category.
///
/// Performs best-effort static analysis on the command text. Handles
/// simple commands, pipelines, and common shell patterns. Does NOT
/// execute the command or perform full shell parsing.
///
/// # Examples
///
/// ```
/// use crab_tools::builtin::bash_classifier::classify_command;
///
/// let result = classify_command("ls -la");
/// assert_eq!(result.category, crab_tools::builtin::bash_classifier::CommandCategory::ReadOnly);
/// assert!(!result.is_destructive);
/// ```
pub fn classify_command(command: &str) -> ClassificationResult {
    let trimmed = command.trim();

    if trimmed.is_empty() {
        return ClassificationResult {
            category: CommandCategory::Unknown,
            is_destructive: false,
            affected_paths: Vec::new(),
            description: "empty command".to_string(),
        };
    }

    // Check for dangerous patterns first (highest priority).
    if is_dangerous_pattern(trimmed) {
        return ClassificationResult {
            category: CommandCategory::Dangerous,
            is_destructive: true,
            affected_paths: extract_file_paths(trimmed),
            description: format!("dangerous command: {}", first_word(trimmed)),
        };
    }

    // Extract the base command (first word, ignoring env vars and prefixes).
    let base_cmd = extract_base_command(trimmed);

    // Check output redirection — promotes read-only commands to file-write.
    let has_redirect = has_output_redirect(trimmed);

    // Git commands get special handling.
    if base_cmd == "git" {
        return classify_git_command(trimmed);
    }

    // Check each category.
    if !has_redirect && is_read_only_base(base_cmd) {
        return ClassificationResult {
            category: CommandCategory::ReadOnly,
            is_destructive: false,
            affected_paths: Vec::new(),
            description: format!("read-only command: {base_cmd}"),
        };
    }

    if NETWORK_COMMANDS.contains(&base_cmd) {
        return ClassificationResult {
            category: CommandCategory::NetworkAccess,
            is_destructive: false,
            affected_paths: extract_file_paths(trimmed),
            description: format!("network command: {base_cmd}"),
        };
    }

    if PROCESS_CONTROL_COMMANDS.contains(&base_cmd) {
        return ClassificationResult {
            category: CommandCategory::ProcessControl,
            is_destructive: true,
            affected_paths: Vec::new(),
            description: format!("process control command: {base_cmd}"),
        };
    }

    if PACKAGE_MANAGER_COMMANDS.contains(&base_cmd) {
        return ClassificationResult {
            category: CommandCategory::PackageManager,
            is_destructive: false,
            affected_paths: extract_file_paths(trimmed),
            description: format!("package manager: {base_cmd}"),
        };
    }

    if FILE_WRITE_COMMANDS.contains(&base_cmd) || has_redirect {
        let is_rm = base_cmd == "rm" || base_cmd == "rmdir";
        return ClassificationResult {
            category: CommandCategory::FileWrite,
            is_destructive: is_rm,
            affected_paths: extract_file_paths(trimmed),
            description: format!("file-write command: {base_cmd}"),
        };
    }

    // Default: unknown
    ClassificationResult {
        category: CommandCategory::Unknown,
        is_destructive: false,
        affected_paths: extract_file_paths(trimmed),
        description: format!("unclassified command: {base_cmd}"),
    }
}

/// Check if a command is read-only (no side effects, no file writes).
///
/// Convenience wrapper around `classify_command` for simple checks.
#[must_use]
pub fn is_read_only_command(command: &str) -> bool {
    classify_command(command).category == CommandCategory::ReadOnly
}

/// Extract file paths referenced in a command (best-effort).
///
/// Looks for tokens that look like file paths (starting with `/`, `./`, `../`,
/// `~`, or containing path separators). Does not resolve or validate paths.
pub fn extract_file_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();

    for token in shell_tokenize(command) {
        // Skip flags and option arguments.
        if token.starts_with('-') {
            continue;
        }
        // Check if it looks like a file path.
        if looks_like_path(token) {
            paths.push(token.to_string());
        }
    }

    paths
}

// ─── Internal helpers ───────────────────────────────────────────────────

/// Extract the base command name from a shell command line.
///
/// Handles:
/// - Leading environment variables (`FOO=bar cmd`)
/// - `sudo` / `env` prefixes
/// - Pipelines (returns the first command)
fn extract_base_command(command: &str) -> &str {
    let cmd = command.trim();

    // Strip leading env assignments (e.g., "FOO=bar BAZ=qux cmd").
    let mut rest = cmd;
    for token in cmd.split_whitespace() {
        if token.contains('=') && !token.starts_with('-') {
            rest = cmd[token.len()..].trim_start();
        } else {
            break;
        }
    }

    // Take only the first pipeline segment.
    let segment = rest.split('|').next().unwrap_or(rest).trim();

    // Strip sudo/env prefix.
    let segment = strip_prefix_commands(segment);

    first_word(segment)
}

/// Strip common prefix commands like `sudo`, `env`, `nice`, `nohup`.
///
/// Handles flags that take values (e.g., `sudo -u root cmd`): each flag
/// starting with `-` consumes both the flag token and its value token.
fn strip_prefix_commands(cmd: &str) -> &str {
    let prefixes = ["sudo", "env", "nice", "nohup", "time", "strace"];
    // Flags for sudo/env that consume a following argument.
    let flags_with_value = ["-u", "-g", "-C", "-D", "-i", "-s"];
    let mut rest = cmd;
    loop {
        let word = first_word(rest);
        if prefixes.contains(&word) {
            rest = rest[word.len()..].trim_start();
            // Skip flags after sudo/env (e.g., "sudo -u root cmd")
            while rest.starts_with('-') {
                let flag = first_word(rest);
                rest = rest[flag.len()..].trim_start();
                // If this flag takes a value argument, skip that too.
                if flags_with_value.contains(&flag) && !rest.is_empty() && !rest.starts_with('-') {
                    let val = first_word(rest);
                    rest = rest[val.len()..].trim_start();
                }
            }
        } else {
            break;
        }
    }
    rest
}

/// Get the first whitespace-delimited word.
fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// Check if the base command is in the read-only list.
fn is_read_only_base(cmd: &str) -> bool {
    READ_ONLY_COMMANDS.contains(&cmd)
}

/// Check if a command matches known dangerous patterns.
fn is_dangerous_pattern(command: &str) -> bool {
    let normalized = command.replace("  ", " ");
    for pattern in DANGEROUS_COMMANDS {
        // Use word-boundary-aware matching: the dangerous pattern must appear
        // such that the last character isn't followed by more path characters.
        if let Some(pos) = normalized.find(pattern) {
            let after = pos + pattern.len();
            // If the pattern ends at the string boundary, or the next char is
            // whitespace / semicolon / pipe, it's a match. But if followed by
            // more path characters (e.g., "rm -rf /tmp"), it's not the
            // dangerous "rm -rf /" pattern.
            if after >= normalized.len() {
                return true;
            }
            let next_char = normalized.as_bytes()[after];
            if next_char == b' '
                || next_char == b';'
                || next_char == b'|'
                || next_char == b'&'
                || next_char == b'\n'
            {
                return true;
            }
            // Special case: patterns that already end with specific paths
            // (like "rm -rf /*") should be exact-contains.
            if pattern.ends_with('*') || pattern.ends_with('~') {
                return true;
            }
        }
    }
    // Fork bomb detection.
    if normalized.contains("(){") && normalized.contains(":|:") {
        return true;
    }
    false
}

/// Classify a git command by its sub-command.
fn classify_git_command(command: &str) -> ClassificationResult {
    let parts: Vec<&str> = command.split_whitespace().collect();

    // Find the git sub-command (skip "git" and any global flags like -C).
    let mut sub_idx = 1;
    while sub_idx < parts.len() && parts[sub_idx].starts_with('-') {
        sub_idx += 1;
        // Skip flag argument if it's a flag that takes a value.
        if sub_idx > 1
            && (parts[sub_idx - 1] == "-C" || parts[sub_idx - 1] == "-c")
            && sub_idx < parts.len()
        {
            sub_idx += 1;
        }
    }

    let sub_cmd = parts.get(sub_idx).copied().unwrap_or("");

    if READ_ONLY_GIT_SUBCOMMANDS.contains(&sub_cmd) {
        ClassificationResult {
            category: CommandCategory::ReadOnly,
            is_destructive: false,
            affected_paths: Vec::new(),
            description: format!("git read-only: git {sub_cmd}"),
        }
    } else {
        let is_destructive = matches!(
            sub_cmd,
            "push" | "reset" | "clean" | "checkout" | "rebase" | "merge"
        );
        ClassificationResult {
            category: CommandCategory::GitOperation,
            is_destructive,
            affected_paths: extract_file_paths(command),
            description: format!("git operation: git {sub_cmd}"),
        }
    }
}

/// Check if the command has output redirection (`>`, `>>`, `2>`).
fn has_output_redirect(command: &str) -> bool {
    // Simple heuristic: look for > that isn't inside quotes.
    // This is an approximation — full shell parsing would be needed for accuracy.
    let mut in_single = false;
    let mut in_double = false;
    let mut prev = ' ';

    for ch in command.chars() {
        match ch {
            '\'' if !in_double && prev != '\\' => in_single = !in_single,
            '"' if !in_single && prev != '\\' => in_double = !in_double,
            '>' if !in_single && !in_double => return true,
            _ => {}
        }
        prev = ch;
    }
    false
}

/// Simple shell tokenizer — splits on whitespace, respecting quotes.
///
/// This is a best-effort tokenizer, not a full shell parser.
fn shell_tokenize(command: &str) -> Vec<&str> {
    // Simple split on whitespace, skipping shell operators.
    command
        .split_whitespace()
        .filter(|t| {
            !t.starts_with('-')
                && !t.starts_with('|')
                && !t.starts_with('&')
                && !t.starts_with(';')
                && !t.contains('=')
                && *t != ">"
                && *t != ">>"
                && *t != "2>"
                && *t != "2>>"
                && *t != "<"
                && *t != "&&"
                && *t != "||"
        })
        .collect()
}

/// Check if a token looks like a file path.
fn looks_like_path(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('~')
        || (token.contains('/') && !token.contains("://"))
        || (token.contains('\\') && !token.starts_with('-'))
        || PathBuf::from(token).extension().is_some()
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_command ────────────────────────────────────────────

    #[test]
    fn classify_empty() {
        let r = classify_command("");
        assert_eq!(r.category, CommandCategory::Unknown);
        assert!(!r.is_destructive);
    }

    #[test]
    fn classify_ls() {
        let r = classify_command("ls -la");
        assert_eq!(r.category, CommandCategory::ReadOnly);
        assert!(!r.is_destructive);
    }

    #[test]
    fn classify_cat() {
        let r = classify_command("cat /etc/hosts");
        assert_eq!(r.category, CommandCategory::ReadOnly);
    }

    #[test]
    fn classify_grep() {
        let r = classify_command("grep -r 'TODO' src/");
        assert_eq!(r.category, CommandCategory::ReadOnly);
    }

    #[test]
    fn classify_git_status() {
        let r = classify_command("git status");
        assert_eq!(r.category, CommandCategory::ReadOnly);
    }

    #[test]
    fn classify_git_log() {
        let r = classify_command("git log --oneline -10");
        assert_eq!(r.category, CommandCategory::ReadOnly);
    }

    #[test]
    fn classify_git_commit() {
        let r = classify_command("git commit -m 'fix bug'");
        assert_eq!(r.category, CommandCategory::GitOperation);
    }

    #[test]
    fn classify_git_push() {
        let r = classify_command("git push origin main");
        assert_eq!(r.category, CommandCategory::GitOperation);
        assert!(r.is_destructive);
    }

    #[test]
    fn classify_rm() {
        let r = classify_command("rm file.txt");
        assert_eq!(r.category, CommandCategory::FileWrite);
        assert!(r.is_destructive);
    }

    #[test]
    fn classify_rm_rf_root() {
        let r = classify_command("rm -rf /");
        assert_eq!(r.category, CommandCategory::Dangerous);
        assert!(r.is_destructive);
    }

    #[test]
    fn classify_curl() {
        let r = classify_command("curl https://example.com");
        assert_eq!(r.category, CommandCategory::NetworkAccess);
    }

    #[test]
    fn classify_npm_install() {
        let r = classify_command("npm install express");
        assert_eq!(r.category, CommandCategory::PackageManager);
    }

    #[test]
    fn classify_cargo_build() {
        let r = classify_command("cargo build");
        assert_eq!(r.category, CommandCategory::PackageManager);
    }

    #[test]
    fn classify_kill() {
        let r = classify_command("kill -9 1234");
        assert_eq!(r.category, CommandCategory::ProcessControl);
        assert!(r.is_destructive);
    }

    #[test]
    fn classify_cp() {
        let r = classify_command("cp src.txt dst.txt");
        assert_eq!(r.category, CommandCategory::FileWrite);
        assert!(!r.is_destructive); // cp is not destructive in itself
    }

    #[test]
    fn classify_redirect_promotes_to_write() {
        let r = classify_command("echo hello > output.txt");
        assert_eq!(r.category, CommandCategory::FileWrite);
    }

    #[test]
    fn classify_with_sudo() {
        let r = classify_command("sudo rm -rf /tmp/junk");
        assert_eq!(r.category, CommandCategory::FileWrite);
        assert!(r.is_destructive);
    }

    #[test]
    fn classify_with_env_vars() {
        let r = classify_command("FOO=bar ls -l");
        assert_eq!(r.category, CommandCategory::ReadOnly);
    }

    #[test]
    fn classify_pipeline_first_cmd() {
        let r = classify_command("cat file.txt | grep pattern");
        assert_eq!(r.category, CommandCategory::ReadOnly);
    }

    #[test]
    fn classify_dd_dangerous() {
        let r = classify_command("dd if=/dev/zero of=/dev/sda");
        assert_eq!(r.category, CommandCategory::Dangerous);
    }

    // ── is_read_only_command ───────────────────────────────────────

    #[test]
    fn read_only_ls() {
        assert!(is_read_only_command("ls"));
    }

    #[test]
    fn read_only_git_status() {
        assert!(is_read_only_command("git status"));
    }

    #[test]
    fn not_read_only_rm() {
        assert!(!is_read_only_command("rm file.txt"));
    }

    #[test]
    fn not_read_only_redirect() {
        assert!(!is_read_only_command("echo hello > file.txt"));
    }

    // ── extract_file_paths ─────────────────────────────────────────

    #[test]
    fn extract_paths_basic() {
        let paths = extract_file_paths("cp /src/file.txt /dst/file.txt");
        assert!(paths.contains(&"/src/file.txt".to_string()));
        assert!(paths.contains(&"/dst/file.txt".to_string()));
    }

    #[test]
    fn extract_paths_relative() {
        let paths = extract_file_paths("cat ./README.md");
        assert!(paths.contains(&"./README.md".to_string()));
    }

    #[test]
    fn extract_paths_empty() {
        let paths = extract_file_paths("ls");
        assert!(paths.is_empty());
    }

    // ── Helpers ────────────────────────────────────────────────────

    #[test]
    fn first_word_simple() {
        assert_eq!(first_word("hello world"), "hello");
    }

    #[test]
    fn first_word_empty() {
        assert_eq!(first_word(""), "");
    }

    #[test]
    fn category_display() {
        assert_eq!(CommandCategory::ReadOnly.to_string(), "read-only");
        assert_eq!(CommandCategory::Dangerous.to_string(), "dangerous");
        assert_eq!(CommandCategory::Unknown.to_string(), "unknown");
    }

    #[test]
    fn classification_result_safe_to_auto_allow() {
        let safe = ClassificationResult {
            category: CommandCategory::ReadOnly,
            is_destructive: false,
            affected_paths: Vec::new(),
            description: "safe".into(),
        };
        assert!(safe.is_safe_to_auto_allow());

        let risky = ClassificationResult {
            category: CommandCategory::FileWrite,
            is_destructive: false,
            affected_paths: Vec::new(),
            description: "risky".into(),
        };
        assert!(!risky.is_safe_to_auto_allow());
    }

    #[test]
    fn has_output_redirect_basic() {
        assert!(has_output_redirect("echo hi > file"));
        assert!(has_output_redirect("cmd >> log.txt"));
        assert!(!has_output_redirect("echo hi"));
    }

    #[test]
    fn extract_base_command_simple() {
        assert_eq!(extract_base_command("ls -la"), "ls");
    }

    #[test]
    fn extract_base_command_with_env() {
        assert_eq!(extract_base_command("FOO=bar cmd --flag"), "cmd");
    }

    #[test]
    fn extract_base_command_with_sudo() {
        assert_eq!(extract_base_command("sudo -u root cat /etc/shadow"), "cat");
    }
}
