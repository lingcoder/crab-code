//! `/commit` — Create a well-structured git commit.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("commit")
        .description("Create a git commit with a descriptive message based on staged changes")
        .command_trigger("commit")
        .when_to_use("When the user wants to commit their changes with a good message")
        .content(PROMPT)
        .build()
        .expect("built-in skill 'commit' must be valid")
}

const PROMPT: &str = "\
Create a git commit with a descriptive message based on staged changes.";
