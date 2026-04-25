//! `.gitignore` auto-maintenance for crab's project-local files.
//!
//! Crab writes / consumes a few files that are intentionally **per-checkout
//! only** — they must never end up in commits. Today:
//!
//! - `.crab/config.local.toml` (user secrets / per-checkout overrides)
//! - `AGENTS.local.md` (project memory the user wants to keep private)
//!
//! Whenever crab touches one of these paths (writing the config, or
//! observing an AGENTS.local.md while loading memory), it calls the
//! corresponding helper here to make sure Git ignores the file. The check
//! is two-layered to avoid duplicating entries for users who already have
//! global rules:
//!
//! 1. Run `git check-ignore --quiet <path>` — succeeds (exit 0) when Git
//!    already ignores the file via any project-level or global rule.
//! 2. Inspect the user's global gitignore (`core.excludesfile` or
//!    `~/.config/git/ignore`) for a matching entry.
//!
//! Only when both checks fail does crab append the corresponding line to
//! the project's `.gitignore`. Repeated calls are idempotent.

use std::path::{Path, PathBuf};
use std::process::Command;

use crab_core::{Error, Result};

/// `.gitignore` line for the project-local config file.
const CONFIG_LOCAL_ENTRY: &str = "/.crab/config.local.toml";
/// `.gitignore` line for the project-local AGENTS memory file.
const AGENTS_LOCAL_ENTRY: &str = "/AGENTS.local.md";

/// Ensure `path` is ignored by Git, appending `entry` to the project
/// `.gitignore` if needed. `match_filename` returns true for any existing
/// gitignore line that already covers the same target file (allowing
/// tolerant matching for e.g. `**/config.local.toml`).
fn ensure_ignored(
    path: &Path,
    entry: &'static str,
    match_filename: fn(&str) -> bool,
) -> Result<()> {
    if already_ignored_by_git(path) {
        return Ok(());
    }
    if global_gitignore_covers(match_filename) {
        return Ok(());
    }
    let Some(repo_root) = find_repo_root(path) else {
        // Not inside a Git checkout — no `.gitignore` to maintain.
        return Ok(());
    };
    append_to_project_gitignore(&repo_root.join(".gitignore"), entry)
}

/// Ensure that `local_config_path` is ignored by Git in its enclosing repo.
///
/// Idempotent. Returns `Ok(())` on success or when the path is not inside a
/// Git repository (no `.gitignore` to maintain).
pub fn ensure_local_config_ignored(local_config_path: &Path) -> Result<()> {
    ensure_ignored(
        local_config_path,
        CONFIG_LOCAL_ENTRY,
        matches_local_config_entry,
    )
}

/// Ensure that `agents_md_path` (typically `<project>/AGENTS.local.md`) is
/// ignored by Git. Same idempotency / no-op-outside-git semantics as
/// [`ensure_local_config_ignored`].
pub fn ensure_local_agents_md_ignored(agents_md_path: &Path) -> Result<()> {
    ensure_ignored(
        agents_md_path,
        AGENTS_LOCAL_ENTRY,
        matches_local_agents_md_entry,
    )
}

/// True when `git check-ignore --quiet <path>` exits 0, meaning Git already
/// ignores the file via *any* rule (project, global, or system).
fn already_ignored_by_git(path: &Path) -> bool {
    let output = Command::new("git")
        .args(["check-ignore", "--quiet"])
        .arg(path)
        .output();
    matches!(output, Ok(o) if o.status.success())
}

/// True when the user's global gitignore lists an entry matching the target
/// file (per `match_line`). Tolerant of `**/x` style entries — see the
/// `matches_*_entry` helpers.
fn global_gitignore_covers(match_line: fn(&str) -> bool) -> bool {
    let Some(path) = global_gitignore_path() else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    content.lines().any(match_line)
}

/// Locate the user's global gitignore file: prefer `git config core.excludesfile`,
/// fall back to `~/.config/git/ignore` (the documented default).
fn global_gitignore_path() -> Option<PathBuf> {
    if let Ok(out) = Command::new("git")
        .args(["config", "--global", "--get", "core.excludesfile"])
        .output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(expand_user(&s));
        }
    }
    let home = crab_core::common::utils::path::home_dir();
    Some(home.join(".config").join("git").join("ignore"))
}

/// Expand a leading `~/` to the user's home directory.
fn expand_user(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        crab_core::common::utils::path::home_dir().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Skip comments, blank lines, and negation rules — they shouldn't count as
/// "covers this file" matches.
fn is_meaningful_gitignore_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
        None
    } else {
        Some(trimmed)
    }
}

/// Match a single gitignore line against `config.local.toml`. Tolerant of
/// `config.local.toml`, `/.crab/config.local.toml`, `**/config.local.toml`, etc.
fn matches_local_config_entry(line: &str) -> bool {
    is_meaningful_gitignore_line(line).is_some_and(|s| s.ends_with("config.local.toml"))
}

/// Match a single gitignore line against `AGENTS.local.md`. Same tolerance
/// rules as the config matcher.
fn matches_local_agents_md_entry(line: &str) -> bool {
    is_meaningful_gitignore_line(line).is_some_and(|s| s.ends_with("AGENTS.local.md"))
}

/// Walk upward from `start` looking for a `.git` directory or file (the
/// latter for worktrees and submodules) and return the enclosing repo root.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    // Anchor on the parent directory because `start` itself may not exist yet.
    if !current.is_dir() {
        current.pop();
    }
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Append `entry` to `gitignore_path` if not already present.
fn append_to_project_gitignore(gitignore_path: &Path, entry: &str) -> Result<()> {
    let mut content = match std::fs::read_to_string(gitignore_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(Error::Config(format!(
                "failed to read {}: {e}",
                gitignore_path.display()
            )));
        }
    };

    if content.lines().any(|l| l.trim() == entry) {
        return Ok(());
    }

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(entry);
    content.push('\n');

    std::fs::write(gitignore_path, content)
        .map_err(|e| Error::Config(format!("failed to write {}: {e}", gitignore_path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_local_config_entry_accepts_plain_entry() {
        assert!(matches_local_config_entry("config.local.toml"));
        assert!(matches_local_config_entry("/.crab/config.local.toml"));
        assert!(matches_local_config_entry("**/config.local.toml"));
        assert!(matches_local_config_entry("  config.local.toml  "));
    }

    #[test]
    fn matches_local_config_entry_rejects_comments_and_negations() {
        assert!(!matches_local_config_entry(""));
        assert!(!matches_local_config_entry("# config.local.toml"));
        assert!(!matches_local_config_entry("!config.local.toml"));
    }

    #[test]
    fn matches_local_config_entry_rejects_unrelated_files() {
        assert!(!matches_local_config_entry("config.toml"));
        assert!(!matches_local_config_entry("settings.local.json"));
    }

    #[test]
    fn matches_local_agents_md_entry_accepts_plain_entry() {
        assert!(matches_local_agents_md_entry("AGENTS.local.md"));
        assert!(matches_local_agents_md_entry("/AGENTS.local.md"));
        assert!(matches_local_agents_md_entry("**/AGENTS.local.md"));
    }

    #[test]
    fn matches_local_agents_md_entry_rejects_unrelated() {
        assert!(!matches_local_agents_md_entry("AGENTS.md"));
        assert!(!matches_local_agents_md_entry("# AGENTS.local.md"));
    }

    #[test]
    fn append_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        append_to_project_gitignore(&gi, CONFIG_LOCAL_ENTRY).unwrap();
        let content = std::fs::read_to_string(&gi).unwrap();
        assert!(content.contains(CONFIG_LOCAL_ENTRY));
    }

    #[test]
    fn append_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        append_to_project_gitignore(&gi, CONFIG_LOCAL_ENTRY).unwrap();
        let after_first = std::fs::read_to_string(&gi).unwrap();
        append_to_project_gitignore(&gi, CONFIG_LOCAL_ENTRY).unwrap();
        let after_second = std::fs::read_to_string(&gi).unwrap();
        assert_eq!(after_first, after_second);
        assert_eq!(after_second.matches(CONFIG_LOCAL_ENTRY).count(), 1);
    }

    #[test]
    fn append_preserves_existing_content_and_adds_newline() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        std::fs::write(&gi, "target/").unwrap();
        append_to_project_gitignore(&gi, CONFIG_LOCAL_ENTRY).unwrap();
        let content = std::fs::read_to_string(&gi).unwrap();
        assert!(content.starts_with("target/"));
        assert!(content.contains(CONFIG_LOCAL_ENTRY));
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn append_does_nothing_when_entry_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        let original = format!("target/\n{CONFIG_LOCAL_ENTRY}\nnode_modules/\n");
        std::fs::write(&gi, &original).unwrap();
        append_to_project_gitignore(&gi, CONFIG_LOCAL_ENTRY).unwrap();
        let after = std::fs::read_to_string(&gi).unwrap();
        assert_eq!(after, original);
    }

    #[test]
    fn append_two_distinct_entries() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        append_to_project_gitignore(&gi, CONFIG_LOCAL_ENTRY).unwrap();
        append_to_project_gitignore(&gi, AGENTS_LOCAL_ENTRY).unwrap();
        let content = std::fs::read_to_string(&gi).unwrap();
        assert!(content.contains(CONFIG_LOCAL_ENTRY));
        assert!(content.contains(AGENTS_LOCAL_ENTRY));
    }

    #[test]
    fn find_repo_root_returns_none_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(find_repo_root(&nested.join("config.toml")).is_none());
    }

    #[test]
    fn find_repo_root_finds_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let root = find_repo_root(&nested.join("config.toml")).unwrap();
        // Canonicalise both sides — tempdir on macOS lives behind /var → /private/var.
        assert_eq!(
            std::fs::canonicalize(&root).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }
}
