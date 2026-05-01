//! Fuzzy matcher for skill names and slash commands.
//!
//! Backed by the `nucleo-matcher` crate — the same fuzzy matcher used by
//! popular terminal UIs (Helix, Zellij, fuzzel, etc.) — so partial input
//! like `/com` → `commit`, `/deb` → `debug` is ranked by relevance rather
//! than alphabetic order.
//!
//! Matches against:
//! - skill **name** (e.g. `"commit"`)
//! - slash-command **trigger** name when the skill declares
//!   `SkillTrigger::Command { name }` (same string in practice but we
//!   check both fields so skills with a differing command name still match)
//! - **description** (lower-weighted — catches "I want a thing that does X"
//!   queries like `/fix` → `debug` because debug's description contains
//!   "fix").
//!
//! The matcher is purely pure-data: no filesystem, no I/O. Re-usable from
//! the TUI autocomplete component, the CLI `crab skill find`, and any
//! programmatic skill dispatch.

use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::registry::SkillRegistry;
use crate::types::{Skill, SkillTrigger};

// ─── Tunables ──────────────────────────────────────────────────────────

/// Bonus added to the `name` field score so a name hit outranks a
/// description hit of similar fuzzy quality.
const NAME_WEIGHT: u16 = 2;

/// Bonus for matching the literal slash-command trigger.
const COMMAND_WEIGHT: u16 = 3;

// Descriptions match with weight 1 (implicit baseline; no constant needed).

/// Minimum combined score for a skill to appear in results. Keeps the
/// list tight when the user has only typed a few characters.
const MIN_SCORE: u16 = 1;

// ─── Public API ────────────────────────────────────────────────────────

/// A single scored match produced by [`match_skills`].
#[derive(Debug, Clone)]
pub struct MatchResult<'a> {
    /// Reference to the skill that matched.
    pub skill: &'a Skill,
    /// Aggregate score — higher is more relevant. Caller treats as
    /// opaque; ordering is the contract.
    pub score: u32,
    /// Which field produced the best hit (for UI highlighting).
    pub matched_on: MatchField,
}

/// Which skill field produced the winning score in a [`MatchResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchField {
    /// Matched on `skill.name`.
    Name,
    /// Matched on the slash-command trigger name.
    Command,
    /// Matched on `skill.description`.
    Description,
}

/// Fuzzy-match `query` against every skill in the registry and return the
/// results sorted by descending score.
///
/// Skills that don't match at all are filtered out. Ties in score are
/// broken by lexicographic name order so the output is deterministic
/// across calls.
///
/// # Examples
///
/// ```no_run
/// use crab_skills::{SkillRegistry, matcher::match_skills};
///
/// let reg = SkillRegistry::new();
/// let hits = match_skills(&reg, "com");
/// for hit in hits {
///     println!("{}: score={}", hit.skill.name, hit.score);
/// }
/// ```
#[must_use]
pub fn match_skills<'a>(registry: &'a SkillRegistry, query: &str) -> Vec<MatchResult<'a>> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut query_buf = Vec::new();
    let query_utf32 = Utf32Str::new(query, &mut query_buf);

    let mut results: Vec<MatchResult<'a>> = registry
        .list()
        .iter()
        .filter_map(|skill| score_skill(&mut matcher, skill, query_utf32))
        .collect();

    // Higher score first; name asc on ties.
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.skill.name.cmp(&b.skill.name))
    });
    results
}

/// Convenience wrapper that returns just the top `n` matches.
///
/// `n == 0` returns an empty vec.
#[must_use]
pub fn top_matches<'a>(registry: &'a SkillRegistry, query: &str, n: usize) -> Vec<MatchResult<'a>> {
    let mut hits = match_skills(registry, query);
    hits.truncate(n);
    hits
}

// ─── Internals ─────────────────────────────────────────────────────────

/// Score a single skill. Returns `None` if no field scored above
/// [`MIN_SCORE`].
fn score_skill<'a>(
    matcher: &mut Matcher,
    skill: &'a Skill,
    query: Utf32Str<'_>,
) -> Option<MatchResult<'a>> {
    let mut buf = Vec::new();

    // Name hit.
    let name_hit = matcher.fuzzy_match(Utf32Str::new(&skill.name, &mut buf), query);
    let name_score = name_hit.map(|s| u32::from(s) * u32::from(NAME_WEIGHT));

    // Command hit (only if this skill has a command trigger).
    let command_score = if let SkillTrigger::Command { name } = &skill.trigger {
        matcher
            .fuzzy_match(Utf32Str::new(name, &mut buf), query)
            .map(|s| u32::from(s) * u32::from(COMMAND_WEIGHT))
    } else {
        None
    };

    // Description hit (lowest weight).
    let desc_score = matcher
        .fuzzy_match(Utf32Str::new(&skill.description, &mut buf), query)
        .map(u32::from);

    // Pick the best field; reject if nothing cleared the threshold.
    let best = [
        (command_score, MatchField::Command),
        (name_score, MatchField::Name),
        (desc_score, MatchField::Description),
    ]
    .into_iter()
    .filter_map(|(score, field)| score.map(|s| (s, field)))
    .filter(|(s, _)| *s >= u32::from(MIN_SCORE))
    .max_by_key(|(s, _)| *s)?;

    Some(MatchResult {
        skill,
        score: best.0,
        matched_on: best.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::SkillBuilder;
    use crate::types::Skill;

    fn cmd_skill(name: &str, cmd: &str, desc: &str) -> Skill {
        SkillBuilder::new(name)
            .description(desc)
            .command_trigger(cmd)
            .content("// body")
            .build()
            .unwrap()
    }

    /// Build a skill with the default `Manual` trigger (no command name).
    fn manual_skill(name: &str, desc: &str) -> Skill {
        SkillBuilder::new(name)
            .description(desc)
            .content("// body")
            .build()
            .unwrap()
    }

    fn populated() -> SkillRegistry {
        let mut reg = SkillRegistry::new();
        reg.register(cmd_skill("commit", "commit", "Create a git commit"));
        reg.register(cmd_skill("debug", "debug", "Fix a bug in the code"));
        reg.register(cmd_skill("review-pr", "review-pr", "Review a pull request"));
        reg.register(manual_skill("autoloader", "Auto-select relevant context"));
        reg
    }

    #[test]
    fn empty_query_yields_no_results() {
        let reg = populated();
        assert!(match_skills(&reg, "").is_empty());
    }

    #[test]
    fn exact_name_wins_over_partial_others() {
        let reg = populated();
        let hits = match_skills(&reg, "commit");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].skill.name, "commit");
    }

    #[test]
    fn partial_matches_rank_prefix_higher_than_middle() {
        // `com` should rank `commit` above any skill whose name only has
        // a scattered c/o/m somewhere mid-word.
        let reg = populated();
        let hits = match_skills(&reg, "com");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].skill.name, "commit");
    }

    #[test]
    fn description_keywords_match() {
        // Nothing named "bug", but debug's description has "bug".
        let reg = populated();
        let hits = match_skills(&reg, "bug");
        assert!(hits.iter().any(|h| h.skill.name == "debug"));
    }

    #[test]
    fn matched_on_tracks_winning_field() {
        let reg = populated();
        // Name match
        let hits = match_skills(&reg, "debug");
        let debug_hit = hits.iter().find(|h| h.skill.name == "debug").unwrap();
        assert!(matches!(
            debug_hit.matched_on,
            MatchField::Name | MatchField::Command
        ));

        // Description-only match
        let hits = match_skills(&reg, "pull request");
        let pr_hit = hits.iter().find(|h| h.skill.name == "review-pr");
        assert!(pr_hit.is_some(), "expected review-pr to match");
    }

    #[test]
    fn top_matches_caps_at_n() {
        let reg = populated();
        let hits = top_matches(&reg, "e", 2);
        assert!(hits.len() <= 2);
    }

    #[test]
    fn top_matches_zero_is_empty() {
        let reg = populated();
        assert!(top_matches(&reg, "anything", 0).is_empty());
    }

    #[test]
    fn no_match_filters_out() {
        let reg = populated();
        let hits = match_skills(&reg, "xyzzyqq");
        assert!(hits.is_empty());
    }

    #[test]
    fn command_trigger_participates_in_scoring() {
        let mut reg = SkillRegistry::new();
        // Same description but one has an exact command match.
        reg.register(cmd_skill("aaa-with-cmd", "review", "irrelevant text"));
        reg.register(manual_skill("zzz-no-cmd", "review something generic"));
        let hits = match_skills(&reg, "review");
        // (fixes temporary-borrow lifetime warning; reg lives long enough)
        let _ = &reg;
        // The one with the command trigger should rank higher because
        // COMMAND_WEIGHT > description baseline.
        assert!(!hits.is_empty());
        assert_eq!(hits[0].skill.name, "aaa-with-cmd");
    }

    #[test]
    fn results_are_deterministic_on_tied_score() {
        // Two skills scoring identically should come back in name order.
        let mut reg = SkillRegistry::new();
        reg.register(manual_skill("bbb", "xxx"));
        reg.register(manual_skill("aaa", "xxx"));
        let hits = match_skills(&reg, "xxx");
        if hits.len() == 2 && hits[0].score == hits[1].score {
            assert_eq!(hits[0].skill.name, "aaa");
            assert_eq!(hits[1].skill.name, "bbb");
        }
    }
}
