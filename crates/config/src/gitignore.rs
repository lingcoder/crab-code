//! `.gitignore` auto-maintenance for `config.local.toml`.
//!
//! When the user first writes `config.local.toml` via `crab config set --local`,
//! the writer calls [`ensure_local_config_ignored`] to make sure the file is
//! covered by Git's ignore rules. The check is two-layered to avoid duplicating
//! entries for users who already have global rules:
//!
//! 1. Run `git check-ignore --quiet <path>` — succeeds (exit 0) when Git
//!    already ignores the file via any project-level or global rule.
//! 2. Inspect the user's global gitignore (`core.excludesfile` or
//!    `~/.config/git/ignore`) for an entry matching `config.local.toml`.
//!
//! Only when both checks fail does the loader append `/.crab/config.local.toml`
//! to the project's `.gitignore`. Repeated writes are idempotent.

use std::path::{Path, PathBuf};
use std::process::Command;

use crab_core::{Error, Result};

/// Entry written to the project `.gitignore` for the local config file.
const GITIGNORE_ENTRY: &str = "/.crab/config.local.toml";

/// Ensure that `local_config_path` is ignored by Git in its enclosing repo.
///
/// Idempotent: safe to call on every write. Returns `Ok(())` on success or
/// when the path is not inside a Git repository (no `.gitignore` to maintain).
pub fn ensure_local_config_ignored(local_config_path: &Path) -> Result<()> {
    if already_ignored_by_git(local_config_path) {
        return Ok(());
    }
    if global_gitignore_covers_local_config() {
        return Ok(());
    }
    let Some(repo_root) = find_repo_root(local_config_path) else {
        // Not inside a Git checkout — no `.gitignore` to maintain.
        return Ok(());
    };
    append_to_project_gitignore(&repo_root.join(".gitignore"))
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

/// True when the user's global gitignore lists a matching entry. We accept
/// any line that ends with `config.local.toml` (with or without leading
/// directory) to be tolerant of slight wording differences.
fn global_gitignore_covers_local_config() -> bool {
    let Some(path) = global_gitignore_path() else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    content.lines().any(matches_local_config_entry)
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

/// Match a single gitignore line against the local config filename. Comments,
/// whitespace-only lines, and negations are ignored. The check tolerates
/// either `config.local.toml` or `**/config.local.toml` style entries.
fn matches_local_config_entry(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
        return false;
    }
    trimmed.ends_with("config.local.toml")
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

/// Append `GITIGNORE_ENTRY` to `gitignore_path` if not already present.
fn append_to_project_gitignore(gitignore_path: &Path) -> Result<()> {
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

    if content.lines().any(|l| l.trim() == GITIGNORE_ENTRY) {
        return Ok(());
    }

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(GITIGNORE_ENTRY);
    content.push('\n');

    std::fs::write(gitignore_path, content).map_err(|e| {
        Error::Config(format!(
            "failed to write {}: {e}",
            gitignore_path.display()
        ))
    })
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
    fn append_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        append_to_project_gitignore(&gi).unwrap();
        let content = std::fs::read_to_string(&gi).unwrap();
        assert!(content.contains(GITIGNORE_ENTRY));
    }

    #[test]
    fn append_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        append_to_project_gitignore(&gi).unwrap();
        let after_first = std::fs::read_to_string(&gi).unwrap();
        append_to_project_gitignore(&gi).unwrap();
        let after_second = std::fs::read_to_string(&gi).unwrap();
        assert_eq!(after_first, after_second);
        assert_eq!(after_second.matches(GITIGNORE_ENTRY).count(), 1);
    }

    #[test]
    fn append_preserves_existing_content_and_adds_newline() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        std::fs::write(&gi, "target/").unwrap();
        append_to_project_gitignore(&gi).unwrap();
        let content = std::fs::read_to_string(&gi).unwrap();
        assert!(content.starts_with("target/"));
        assert!(content.contains(GITIGNORE_ENTRY));
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn append_does_nothing_when_entry_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let gi = dir.path().join(".gitignore");
        let original = format!("target/\n{GITIGNORE_ENTRY}\nnode_modules/\n");
        std::fs::write(&gi, &original).unwrap();
        append_to_project_gitignore(&gi).unwrap();
        let after = std::fs::read_to_string(&gi).unwrap();
        assert_eq!(after, original);
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
