//! `/verify` — Verify that recent changes work correctly.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("verify")
        .description("Run tests and verification steps to confirm changes work as intended")
        .command_trigger("verify")
        .when_to_use("When the user wants to verify that recent changes are correct and working")
        .content(PROMPT)
        .build()
        .expect("built-in skill 'verify' must be valid")
}

const PROMPT: &str = "\
Run tests and verification steps to confirm changes work as intended.";
