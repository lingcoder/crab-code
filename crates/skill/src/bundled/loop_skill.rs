//! `/loop` — Run a command repeatedly on an interval.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("loop")
        .description("Run a prompt or slash command on a recurring interval")
        .command_trigger("loop")
        .when_to_use("When the user wants to set up a recurring task, poll for status, or run something repeatedly")
        .argument_hint("[interval] <prompt>")
        .content(PROMPT)
        .build()
        .expect("bundled skill 'loop' must be valid")
}

const PROMPT: &str = "\
Run the specified command or prompt on a recurring interval.";
