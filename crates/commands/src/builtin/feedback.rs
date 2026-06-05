use crate::context::CommandContext;
use crate::types::{CommandResult, SlashCommand};

const FEEDBACK_REPO: &str = "lingcoder/crab-code";

/// Call `gh issue create` to submit feedback as a GitHub Issue.
///
/// Returns the issue URL on success, or an error message on failure.
/// In test builds this is a no-op that returns a fake URL.
fn create_github_issue(repo: &str, title: &str, body: &str) -> Result<String, String> {
    #[cfg(test)]
    {
        let _ = (repo, title, body);
        return Ok("https://github.com/test/repo/issues/1".into());
    }

    #[cfg(not(test))]
    {
        match std::process::Command::new("gh")
            .args([
                "issue", "create", "--repo", repo, "--title", title, "--body", body,
            ])
            .output()
        {
            Ok(output) if output.status.success() => {
                let url = String::from_utf8_lossy(&output.stdout);
                Ok(url.trim().to_string())
            }
            Ok(output) => {
                let err = String::from_utf8_lossy(&output.stderr);
                Err(format!("Failed to create issue: {}", err.trim()))
            }
            Err(_) => {
                Err("GitHub CLI (gh) not found. Install it from https://cli.github.com/".into())
            }
        }
    }
}

pub struct FeedbackCommand;

impl SlashCommand for FeedbackCommand {
    fn name(&self) -> &'static str {
        "feedback"
    }
    fn description(&self) -> &'static str {
        "Submit feedback via GitHub Issue"
    }
    fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult {
        let title = args.trim();
        if title.is_empty() {
            return CommandResult::Message(
                "Usage: /feedback <title>\nCreates a GitHub issue with environment info.".into(),
            );
        }
        let body = format!(
            "**Submitted via /feedback**\n\nModel: {}\nSession: {}\nWorking dir: {}",
            ctx.model,
            ctx.session_id,
            ctx.working_dir.display()
        );
        match create_github_issue(FEEDBACK_REPO, title, &body) {
            Ok(url) => CommandResult::Message(format!("Issue created: {url}")),
            Err(msg) => CommandResult::Message(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_test_ctx, test_model_and_dir};

    #[test]
    fn feedback_no_args_shows_usage() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = FeedbackCommand.execute("", &ctx) {
            assert!(text.contains("Usage:"));
            assert!(text.contains("/feedback"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn feedback_with_args_creates_issue() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = FeedbackCommand.execute("My bug report", &ctx) {
            assert!(text.contains("Issue created:"));
            assert!(text.contains("https://"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn create_github_issue_returns_url_in_test() {
        let result = create_github_issue("owner/repo", "title", "body");
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with("https://"));
    }
}
