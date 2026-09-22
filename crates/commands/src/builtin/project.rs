use std::path::PathBuf;

use crate::context::CommandContext;
use crate::types::{CommandEffect, CommandResult, SlashCommand};

pub struct InitCommand;

impl SlashCommand for InitCommand {
    fn name(&self) -> &'static str {
        "init"
    }
    fn description(&self) -> &'static str {
        "Generate an AGENTS.md template in current directory"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Effect(CommandEffect::Init)
    }
}

pub struct AddDirCommand;

impl SlashCommand for AddDirCommand {
    fn name(&self) -> &'static str {
        "add-dir"
    }
    fn description(&self) -> &'static str {
        "Add working directory (/add-dir <path>)"
    }
    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let path = args.trim();
        if path.is_empty() {
            return CommandResult::Message("Usage: /add-dir <path>".into());
        }
        let dir = PathBuf::from(path);
        if !dir.is_dir() {
            return CommandResult::Message(format!("Not a directory: {path}"));
        }
        CommandResult::Effect(CommandEffect::AddDir(dir))
    }
}

pub struct FilesCommand;

impl SlashCommand for FilesCommand {
    fn name(&self) -> &'static str {
        "files"
    }
    fn description(&self) -> &'static str {
        "List tracked files in working directory"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandResult {
        let output = std::process::Command::new("git")
            .args(["ls-files"])
            .current_dir(ctx.working_dir)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                let count = text.lines().count();
                if count == 0 {
                    return CommandResult::Message("No tracked files.".into());
                }
                let preview: String = text.lines().take(30).collect::<Vec<_>>().join("\n");
                let suffix = if count > 30 {
                    format!("\n  ... and {} more files", count - 30)
                } else {
                    String::new()
                };
                CommandResult::Message(format!("Tracked files ({count}):\n{preview}{suffix}"))
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                CommandResult::Message(format!("git ls-files failed: {err}"))
            }
            Err(_) => CommandResult::Message("Not a git repository or git not available.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_test_ctx, test_model_and_dir};

    #[test]
    fn init_returns_effect() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        assert!(matches!(
            InitCommand.execute("", &ctx),
            CommandResult::Effect(CommandEffect::Init)
        ));
    }

    #[test]
    fn add_dir_no_args_shows_usage() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = AddDirCommand.execute("", &ctx) {
            assert!(text.contains("Usage:"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn add_dir_nonexistent_shows_error() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        if let CommandResult::Message(text) = AddDirCommand.execute("/nonexistent/path/xyz", &ctx) {
            assert!(text.contains("Not a directory"));
        } else {
            panic!("expected Message");
        }
    }

    #[test]
    fn add_dir_valid_dir_returns_effect() {
        let (model, dir) = test_model_and_dir();
        let ctx = make_test_ctx(&model, &dir);
        let tmp = std::env::temp_dir();
        let result = AddDirCommand.execute(tmp.to_str().unwrap(), &ctx);
        assert!(matches!(
            result,
            CommandResult::Effect(CommandEffect::AddDir(_))
        ));
    }
}
