//! `/schedule` — Create or manage scheduled tasks.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("schedule")
        .description("Create, list, or manage scheduled tasks and cron jobs")
        .command_trigger("schedule")
        .when_to_use("When the user wants to create, view, or manage scheduled tasks")
        .argument_hint("[create|list|delete] <details>")
        .content(PROMPT)
        .build()
        .expect("bundled skill 'schedule' must be valid")
}

const PROMPT: &str = "\
Create, list, or manage scheduled tasks and cron jobs.";
