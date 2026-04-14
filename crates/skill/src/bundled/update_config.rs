//! `/update-config` — Update crab-code configuration.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("update-config")
        .description("Update crab-code settings and configuration via settings.json")
        .command_trigger("update-config")
        .when_to_use("When the user wants to change crab-code settings or configuration")
        .allowed_tools(["Read"])
        .content(PROMPT)
        .build()
        .expect("bundled skill 'update-config' must be valid")
}

const PROMPT: &str = "\
Update crab-code settings and configuration via settings.json.";
