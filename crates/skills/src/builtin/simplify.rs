//! `/simplify` — Review and simplify changed code.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("simplify")
        .description(
            "Review changed code for reuse, quality, and efficiency, then fix any issues found",
        )
        .command_trigger("simplify")
        .when_to_use("When the user wants to clean up, simplify, or improve recently changed code")
        .content(PROMPT)
        .build()
        .expect("built-in skill 'simplify' must be valid")
}

const PROMPT: &str = "\
Review changed code for reuse, quality, and efficiency, then fix issues.";
