//! Built-in skills shipped with crab-code.
//!
//! Provides a set of bundled skills that are always available without requiring
//! external skill files. Each skill is defined as an embedded string constant
//! and registered into the skill registry at startup.
//!
//! Maps to CCB `skills/bundled/` (18+ skills).

use super::skill::Skill;

// ─── Skill content definitions ─────────────────────────────────────────

/// Embedded skill prompt content.
///
/// These are string literals containing the skill prompt templates.
/// In production these will be multi-paragraph markdown instructions;
/// placeholder values are used here during the skeleton phase.
#[allow(dead_code)]
mod definitions {
    /// `/commit` — Create a well-structured git commit.
    pub const COMMIT: &str =
        "Create a git commit with a descriptive message based on staged changes.";

    /// `/review-pr` — Review a pull request for issues and improvements.
    pub const REVIEW_PR: &str =
        "Review the pull request for correctness, style, and potential issues.";

    /// `/debug` — Systematic debugging of an issue.
    pub const DEBUG: &str = "Debug the reported issue systematically using available tools.";

    /// `/loop` — Run a command repeatedly on an interval.
    pub const LOOP: &str = "Run the specified command or prompt on a recurring interval.";

    /// `/remember` — Save information to memory for future sessions.
    pub const REMEMBER: &str =
        "Save the given information to the memory system for future reference.";

    /// `/schedule` — Create or manage scheduled tasks.
    pub const SCHEDULE: &str = "Create, list, or manage scheduled tasks and cron jobs.";

    /// `/simplify` — Review and simplify changed code.
    pub const SIMPLIFY: &str =
        "Review changed code for reuse, quality, and efficiency, then fix issues.";

    /// `/stuck` — Help when the agent is stuck in a loop.
    pub const STUCK: &str = "Break out of an unproductive loop by re-evaluating the approach.";

    /// `/verify` — Verify that recent changes work correctly.
    pub const VERIFY: &str =
        "Run tests and verification steps to confirm changes work as intended.";

    /// `/update-config` — Update crab-code configuration.
    pub const UPDATE_CONFIG: &str =
        "Update crab-code settings and configuration via settings.json.";
}

// ─── Bundled skills constructor ────────────────────────────────────────

/// Get all bundled skills.
///
/// Returns a `Vec<Skill>` containing all built-in skills with their prompt
/// content and trigger definitions. These are registered into the
/// [`SkillRegistry`](super::skill::SkillRegistry) at startup before
/// user/project skills are loaded (so user skills can override them).
pub fn bundled_skills() -> Vec<Skill> {
    todo!("bundled_skills: construct Skill instances from definitions::* constants")
}

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

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_names_not_empty() {
        assert!(!BUNDLED_SKILL_NAMES.is_empty());
        assert!(BUNDLED_SKILL_NAMES.contains(&"commit"));
        assert!(BUNDLED_SKILL_NAMES.contains(&"review-pr"));
    }

    #[test]
    fn definitions_are_nonempty() {
        assert!(!definitions::COMMIT.is_empty());
        assert!(!definitions::REVIEW_PR.is_empty());
        assert!(!definitions::DEBUG.is_empty());
        assert!(!definitions::LOOP.is_empty());
        assert!(!definitions::REMEMBER.is_empty());
        assert!(!definitions::SCHEDULE.is_empty());
        assert!(!definitions::SIMPLIFY.is_empty());
        assert!(!definitions::STUCK.is_empty());
        assert!(!definitions::VERIFY.is_empty());
        assert!(!definitions::UPDATE_CONFIG.is_empty());
    }
}
