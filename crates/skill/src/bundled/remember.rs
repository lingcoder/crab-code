//! `/remember` — Save information to memory for future sessions.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("remember")
        .description("Save the given information to the memory system for future reference")
        .command_trigger("remember")
        .when_to_use("When the user explicitly asks to remember something for future sessions")
        .argument_hint("<what to remember>")
        .content(PROMPT)
        .build()
        .expect("bundled skill 'remember' must be valid")
}

const PROMPT: &str = "\
Save the given information to the memory system for future reference.";
