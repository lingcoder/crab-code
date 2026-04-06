//! Git context collection for system prompt injection.
//!
//! Collects git repository state (branch, status, recent commits, user)
//! using the `git` CLI. Non-git directories silently return `None`.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Maximum time allowed for each git command.
const GIT_TIMEOUT: Duration = Duration::from_millis(200);

/// Collected git repository context.
#[derive(Debug, Clone)]
pub struct GitContext {
    pub branch_name: String,
    pub main_branch: String,
    pub git_user: String,
    pub is_dirty: bool,
    pub status_summary: String,
    pub recent_commits: Vec<String>,
}

impl GitContext {
    /// Collect git context from the given working directory.
    ///
    /// Returns `None` if the directory is not inside a git repository
    /// or if git is not available.
    #[must_use]
    pub fn collect(working_dir: &Path) -> Option<Self> {
        // Quick check: is this a git repo?
        let _ = run_git(working_dir, &["rev-parse", "--git-dir"])?;

        let branch_name = run_git(working_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_else(|| "unknown".to_string());

        let main_branch = detect_main_branch(working_dir).unwrap_or_else(|| branch_name.clone());

        let git_user =
            run_git(working_dir, &["config", "user.name"]).unwrap_or_else(|| "unknown".to_string());

        let status_output = run_git(working_dir, &["status", "--porcelain"]);
        let is_dirty = status_output.as_ref().is_some_and(|s| !s.trim().is_empty());
        let status_summary = if is_dirty {
            status_output.unwrap_or_default()
        } else {
            "(clean)".to_string()
        };

        let recent_commits = run_git(
            working_dir,
            &["log", "--oneline", "-n", "5", "--no-decorate"],
        )
        .map(|output| output.lines().map(String::from).collect::<Vec<_>>())
        .unwrap_or_default();

        Some(Self {
            branch_name,
            main_branch,
            git_user,
            is_dirty,
            status_summary,
            recent_commits,
        })
    }

    /// Format the git context matching Claude Code's gitStatus format.
    #[must_use]
    pub fn format(&self) -> String {
        use std::fmt::Write;

        let mut out = String::with_capacity(512);
        let _ = writeln!(
            out,
            "gitStatus: This is the git status at the start of the conversation. \
             Note that this status is a snapshot in time, and will not update during the conversation."
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "Current branch: {}", self.branch_name);
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Main branch (you will usually use this for PRs): {}",
            self.main_branch
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "Git user: {}", self.git_user);
        let _ = writeln!(out);
        let _ = writeln!(out, "Status:");
        let _ = writeln!(out, "{}", self.status_summary);
        let _ = writeln!(out);

        if !self.recent_commits.is_empty() {
            let _ = writeln!(out, "Recent commits:");
            for commit in &self.recent_commits {
                let _ = writeln!(out, "{commit}");
            }
        }

        out
    }
}

/// Run a git command with a timeout. Returns `None` on any failure.
fn run_git(working_dir: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Poll with timeout
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let output = child.wait_with_output().ok()?;
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return if text.is_empty() { None } else { Some(text) };
            }
            Ok(None) => {
                if start.elapsed() >= GIT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

/// Detect the main/default branch by checking common names.
fn detect_main_branch(working_dir: &Path) -> Option<String> {
    // Try to get from remote HEAD
    if let Some(ref_line) = run_git(
        working_dir,
        &["symbolic-ref", "refs/remotes/origin/HEAD", "--short"],
    ) {
        // Returns something like "origin/main" — strip the remote prefix
        if let Some(branch) = ref_line.strip_prefix("origin/") {
            return Some(branch.to_string());
        }
        return Some(ref_line);
    }

    // Fallback: check if main or master branch exists
    for candidate in &["main", "master"] {
        if run_git(
            working_dir,
            &["rev-parse", "--verify", &format!("refs/heads/{candidate}")],
        )
        .is_some()
        {
            return Some((*candidate).to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn non_git_dir_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = GitContext::collect(dir.path());
        assert!(ctx.is_none());
    }

    #[test]
    fn git_initialized_dir_returns_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Initialize a git repo
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(path)
            .output()
            .unwrap();

        let ctx = GitContext::collect(path);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();

        // Default branch on fresh git init (may be main or master depending on config)
        assert!(!ctx.branch_name.is_empty());
        assert_eq!(ctx.git_user, "Test User");
        // No commits yet
        assert!(ctx.recent_commits.is_empty());
        // Clean status (nothing tracked yet)
        assert!(!ctx.is_dirty);
    }

    #[test]
    fn dirty_status_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(path)
            .output()
            .unwrap();

        // Create and add a file to make repo dirty
        fs::write(path.join("test.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "test.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let ctx = GitContext::collect(path).unwrap();
        assert!(ctx.is_dirty);
        assert!(ctx.status_summary.contains("test.txt"));
    }

    #[test]
    fn recent_commits_collected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(path)
            .output()
            .unwrap();

        // Create a commit
        fs::write(path.join("file.txt"), "content").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(path)
            .output()
            .unwrap();

        let ctx = GitContext::collect(path).unwrap();
        assert_eq!(ctx.recent_commits.len(), 1);
        assert!(ctx.recent_commits[0].contains("initial commit"));
    }

    #[test]
    fn format_output_matches_expected() {
        let ctx = GitContext {
            branch_name: "main".to_string(),
            main_branch: "main".to_string(),
            git_user: "testuser".to_string(),
            is_dirty: false,
            status_summary: "(clean)".to_string(),
            recent_commits: vec!["abc1234 first commit".to_string()],
        };
        let formatted = ctx.format();
        assert!(formatted.contains("gitStatus:"));
        assert!(formatted.contains("Current branch: main"));
        assert!(formatted.contains("Main branch (you will usually use this for PRs): main"));
        assert!(formatted.contains("Git user: testuser"));
        assert!(formatted.contains("Status:"));
        assert!(formatted.contains("(clean)"));
        assert!(formatted.contains("Recent commits:"));
        assert!(formatted.contains("abc1234 first commit"));
    }
}
