//! `/stuck` — Help when the agent is stuck in a loop.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("stuck")
        .description("Break out of an unproductive loop by re-evaluating the approach")
        .command_trigger("stuck")
        .when_to_use(
            "When the user feels the conversation is going in circles or making no progress",
        )
        .content(PROMPT)
        .build()
        .expect("built-in skill 'stuck' must be valid")
}

const PROMPT: &str = "\
Break out of an unproductive loop by re-evaluating the approach.";
