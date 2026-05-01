//! `/review-pr` — Review a pull request for issues and improvements.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("review-pr")
        .description("Review a pull request for correctness, style, and potential issues")
        .command_trigger("review-pr")
        .when_to_use("When the user wants to review a PR or asks about PR quality")
        .argument_hint("[PR number or URL]")
        .content(PROMPT)
        .build()
        .expect("built-in skill 'review-pr' must be valid")
}

const PROMPT: &str = "\
Review the pull request for correctness, style, and potential issues.";
