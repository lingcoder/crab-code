//! `/debug` — Systematic debugging of an issue.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("debug")
        .description("Debug the reported issue systematically using available tools")
        .command_trigger("debug")
        .when_to_use("When the user reports a bug or error and wants help debugging it")
        .argument_hint("<issue description>")
        .content(PROMPT)
        .build()
        .expect("built-in skill 'debug' must be valid")
}

const PROMPT: &str = "\
Debug the reported issue systematically using available tools.";
