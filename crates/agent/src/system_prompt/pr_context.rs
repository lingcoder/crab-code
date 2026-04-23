//! Load context from a GitHub pull request via `gh` CLI.
//!
//! When the user passes `--from-pr <number_or_url>`, this module calls
//! `gh pr view` to fetch PR metadata and diff, then formats it for
//! injection into the system prompt.

use std::fmt::Write;
use std::process::Command;

use serde::Deserialize;

/// Parsed PR metadata from `gh pr view --json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: String,
    pub state: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub changed_files: Vec<ChangedFile>,
}

/// A single file changed in the PR.
#[derive(Debug, Clone, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
}

/// Loaded PR context ready for system prompt injection.
#[derive(Debug, Clone)]
pub struct PrContext {
    pub info: PrInfo,
    /// Abbreviated diff summary (truncated to avoid flooding the prompt).
    pub diff_summary: String,
}

/// Maximum diff lines to include in the system prompt.
const MAX_DIFF_LINES: usize = 300;

impl PrContext {
    /// Format the PR context as a system prompt section.
    pub fn format_for_prompt(&self) -> String {
        let mut out = String::with_capacity(2048);
        let _ = writeln!(out, "# Pull Request Context\n");
        let _ = writeln!(
            out,
            "PR #{}: {} ({})",
            self.info.number, self.info.title, self.info.state
        );
        if !self.info.url.is_empty() {
            let _ = writeln!(out, "URL: {}", self.info.url);
        }
        let _ = writeln!(
            out,
            "Branch: {} → {}",
            self.info.head_ref_name, self.info.base_ref_name
        );
        let _ = writeln!(out);

        if !self.info.body.is_empty() {
            let _ = writeln!(out, "## Description\n");
            let _ = writeln!(out, "{}", self.info.body);
            let _ = writeln!(out);
        }

        if !self.info.changed_files.is_empty() {
            let _ = writeln!(out, "## Changed Files\n");
            for f in &self.info.changed_files {
                let _ = writeln!(out, "- {} (+{} -{})  ", f.path, f.additions, f.deletions);
            }
            let _ = writeln!(out);
        }

        if !self.diff_summary.is_empty() {
            let _ = writeln!(out, "## Diff Summary\n");
            let _ = writeln!(out, "```diff");
            let _ = write!(out, "{}", self.diff_summary);
            let _ = writeln!(out, "```");
            let _ = writeln!(out);
        }

        out
    }
}

/// Load PR context from `gh` CLI.
///
/// `pr_ref` can be a PR number (e.g. "123") or a URL.
/// Returns `None` if `gh` is not available or the PR cannot be fetched.
pub fn load_pr_context(pr_ref: &str) -> crab_core::Result<PrContext> {
    // Fetch structured PR info
    let info = fetch_pr_info(pr_ref)?;

    // Fetch diff summary
    let diff_summary = fetch_pr_diff(pr_ref).unwrap_or_default();

    Ok(PrContext { info, diff_summary })
}

/// Fetch PR metadata via `gh pr view --json`.
fn fetch_pr_info(pr_ref: &str) -> crab_core::Result<PrInfo> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            pr_ref,
            "--json",
            "number,title,body,headRefName,baseRefName,state,url,files",
        ])
        .output()
        .map_err(|e| {
            crab_core::Error::Other(format!(
                "failed to run `gh pr view`: {e}. Is the GitHub CLI installed?"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crab_core::Error::Other(format!(
            "`gh pr view` failed: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // gh returns `files` as an array of objects with `path`, `additions`, `deletions`.
    // We need to map that into our ChangedFile struct.
    let raw: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| crab_core::Error::Other(format!("failed to parse gh output: {e}")))?;

    let number = raw["number"].as_u64().unwrap_or(0);
    let title = raw["title"].as_str().unwrap_or("").to_string();
    let body = raw["body"].as_str().unwrap_or("").to_string();
    let head_ref_name = raw["headRefName"].as_str().unwrap_or("").to_string();
    let base_ref_name = raw["baseRefName"].as_str().unwrap_or("").to_string();
    let state = raw["state"].as_str().unwrap_or("").to_string();
    let url = raw["url"].as_str().unwrap_or("").to_string();

    let changed_files = raw["files"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some(ChangedFile {
                        path: f["path"].as_str()?.to_string(),
                        additions: f["additions"].as_u64().unwrap_or(0) as u32,
                        deletions: f["deletions"].as_u64().unwrap_or(0) as u32,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(PrInfo {
        number,
        title,
        body,
        head_ref_name,
        base_ref_name,
        state,
        url,
        changed_files,
    })
}

/// Fetch the PR diff via `gh pr diff`, truncated to `MAX_DIFF_LINES`.
fn fetch_pr_diff(pr_ref: &str) -> Option<String> {
    let output = Command::new("gh")
        .args(["pr", "diff", pr_ref])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let diff = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() > MAX_DIFF_LINES {
        let mut truncated = lines[..MAX_DIFF_LINES].join("\n");
        let _ = write!(
            truncated,
            "\n\n... ({} more lines truncated)",
            lines.len() - MAX_DIFF_LINES
        );
        Some(truncated)
    } else {
        Some(diff.into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_info_deserialize() {
        let json = r#"{
            "number": 42,
            "title": "Fix bug",
            "body": "This fixes the bug.",
            "headRefName": "fix/bug",
            "baseRefName": "main",
            "state": "OPEN",
            "url": "https://github.com/org/repo/pull/42",
            "changedFiles": []
        }"#;
        let info: PrInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.number, 42);
        assert_eq!(info.title, "Fix bug");
        assert_eq!(info.state, "OPEN");
        assert_eq!(info.head_ref_name, "fix/bug");
    }

    #[test]
    fn changed_file_deserialize() {
        let json = r#"{"path": "src/main.rs", "additions": 10, "deletions": 3}"#;
        let f: ChangedFile = serde_json::from_str(json).unwrap();
        assert_eq!(f.path, "src/main.rs");
        assert_eq!(f.additions, 10);
        assert_eq!(f.deletions, 3);
    }

    #[test]
    fn format_for_prompt_basic() {
        let ctx = PrContext {
            info: PrInfo {
                number: 42,
                title: "Add feature X".into(),
                body: "This adds feature X.".into(),
                head_ref_name: "feature/x".into(),
                base_ref_name: "main".into(),
                state: "OPEN".into(),
                url: "https://github.com/org/repo/pull/42".into(),
                changed_files: vec![
                    ChangedFile {
                        path: "src/lib.rs".into(),
                        additions: 20,
                        deletions: 5,
                    },
                    ChangedFile {
                        path: "src/main.rs".into(),
                        additions: 3,
                        deletions: 0,
                    },
                ],
            },
            diff_summary: "+fn new_function() {\n+    todo!()\n+}".into(),
        };

        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("Pull Request Context"));
        assert!(prompt.contains("PR #42: Add feature X (OPEN)"));
        assert!(prompt.contains("feature/x → main"));
        assert!(prompt.contains("This adds feature X."));
        assert!(prompt.contains("src/lib.rs (+20 -5)"));
        assert!(prompt.contains("src/main.rs (+3 -0)"));
        assert!(prompt.contains("```diff"));
        assert!(prompt.contains("+fn new_function()"));
    }

    #[test]
    fn format_for_prompt_empty_body() {
        let ctx = PrContext {
            info: PrInfo {
                number: 1,
                title: "Quick fix".into(),
                body: String::new(),
                head_ref_name: "fix".into(),
                base_ref_name: "main".into(),
                state: "MERGED".into(),
                url: String::new(),
                changed_files: vec![],
            },
            diff_summary: String::new(),
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("PR #1: Quick fix (MERGED)"));
        assert!(!prompt.contains("Description"));
        assert!(!prompt.contains("Changed Files"));
        assert!(!prompt.contains("Diff Summary"));
    }

    #[test]
    fn format_for_prompt_no_url() {
        let ctx = PrContext {
            info: PrInfo {
                number: 5,
                title: "T".into(),
                body: "B".into(),
                head_ref_name: "h".into(),
                base_ref_name: "b".into(),
                state: "CLOSED".into(),
                url: String::new(),
                changed_files: vec![],
            },
            diff_summary: String::new(),
        };
        let prompt = ctx.format_for_prompt();
        assert!(!prompt.contains("URL:"));
    }

    // Note: load_pr_context() is not tested here because it requires `gh` CLI.
    // Integration tests for this would go in crates/agent/tests/.
}
