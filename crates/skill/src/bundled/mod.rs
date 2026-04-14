//! Built-in skills shipped with crab-code.
//!
//! Each skill is defined in its own file and exports a `skill()` function
//! that returns a fully configured [`Skill`] instance. All bundled skills
//! use `SkillSource::Bundled` and `SkillTrigger::Command`.

mod commit;
mod debug;
mod loop_skill;
mod remember;
mod review_pr;
mod schedule;
mod simplify;
mod stuck;
mod update_config;
mod verify;

use crate::types::Skill;

/// Names of all bundled skills, for use in help text and listings.
pub const BUNDLED_SKILL_NAMES: &[&str] = &[
    "commit",
    "review-pr",
    "debug",
    "loop",
    "remember",
    "schedule",
    "simplify",
    "stuck",
    "verify",
    "update-config",
];

/// Get all bundled skills.
///
/// Returns a `Vec<Skill>` containing all built-in skills with their prompt
/// content and trigger definitions. These should be registered into the
/// [`SkillRegistry`](crate::registry::SkillRegistry) at startup before
/// user/project skills are loaded (so user skills can override them).
pub fn bundled_skills() -> Vec<Skill> {
    vec![
        commit::skill(),
        review_pr::skill(),
        debug::skill(),
        loop_skill::skill(),
        remember::skill(),
        schedule::skill(),
        simplify::skill(),
        stuck::skill(),
        verify::skill(),
        update_config::skill(),
    ]
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::types::{SkillSource, SkillTrigger};

    use super::*;

    #[test]
    fn bundled_skill_names_not_empty() {
        assert!(!BUNDLED_SKILL_NAMES.is_empty());
        assert!(BUNDLED_SKILL_NAMES.contains(&"commit"));
        assert!(BUNDLED_SKILL_NAMES.contains(&"review-pr"));
    }

    #[test]
    fn bundled_skills_count_matches_names() {
        let skills = bundled_skills();
        assert_eq!(skills.len(), BUNDLED_SKILL_NAMES.len());
    }

    #[test]
    fn all_bundled_skills_are_valid() {
        for skill in bundled_skills() {
            assert!(!skill.name.is_empty(), "skill name must not be empty");
            assert!(
                !skill.description.is_empty(),
                "skill '{}' must have a description",
                skill.name
            );
            assert!(
                !skill.content.is_empty(),
                "skill '{}' must have content",
                skill.name
            );
            assert!(
                matches!(skill.trigger, SkillTrigger::Command { .. }),
                "bundled skill '{}' must have a command trigger",
                skill.name
            );
            assert_eq!(
                skill.source,
                SkillSource::Bundled,
                "bundled skill '{}' must have Bundled source",
                skill.name
            );
            assert!(
                skill.user_invocable,
                "bundled skill '{}' must be user-invocable",
                skill.name
            );
        }
    }

    #[test]
    fn bundled_skill_names_match_actual_skills() {
        let skills = bundled_skills();
        for name in BUNDLED_SKILL_NAMES {
            assert!(
                skills.iter().any(|s| s.name == *name),
                "BUNDLED_SKILL_NAMES contains '{name}' but no skill with that name exists"
            );
        }
    }

    #[test]
    fn each_bundled_skill_has_when_to_use() {
        for skill in bundled_skills() {
            assert!(
                skill.when_to_use.is_some(),
                "bundled skill '{}' should have when_to_use set",
                skill.name
            );
        }
    }
}
