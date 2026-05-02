//! `/simplify` — Review and simplify changed code.

use crate::builder::SkillBuilder;
use crate::types::Skill;

pub fn skill() -> Skill {
    SkillBuilder::new("simplify")
        .description(
            "Review changed code for reuse, quality, and efficiency, then fix any issues found",
        )
        .command_trigger("simplify")
        .when_to_use("When the user wants to clean up, simplify, or improve recently changed code")
        .content(PROMPT)
        .build()
        .expect("built-in skill 'simplify' must be valid")
}

const PROMPT: &str = "\
Review the user's recent changes through five review lenses, then fix what \
matters. Be substantive — do not invent issues to look thorough.

# Step 1: Identify the changed code

Run `git diff HEAD` to see uncommitted changes. If that is empty, run \
`git diff --cached` for staged changes. If both are empty, ask the user which \
files or commits to review.

# Step 2: Review through five lenses, in order

Apply each lens to the diff. For each finding note the file and line.

1. **Reuse & DRY** — Is this duplicating an existing utility, helper, or \
constant? Can two near-identical blocks collapse into one function? Is there \
already a crate or stdlib API for this?
2. **Clarity & Readability** — Are names accurate and unambiguous? Can a \
nested branch flatten with early returns? Is a clever one-liner harder to read \
than three plain ones? Are magic numbers literal where a named constant would \
explain intent?
3. **Correctness & Robustness** — Are errors propagated rather than swallowed? \
Are edge cases (empty input, overflow, concurrent access, partial failure) \
handled or knowingly ignored? Are invariants asserted at the boundary?
4. **Performance & Efficiency** — Unnecessary allocations, clones, or \
collects in hot paths? Quadratic loops where a hash-map lookup would do? IO \
inside a tight loop that could batch?
5. **Idiomatic Style** — Does this follow the project's conventions and the \
language's idioms? In Rust: `is_some_and` over `.map().unwrap_or(false)`, \
`?` over manual match-on-error, iterator chains over indexed loops where \
clearer.

# Step 3: Fix what matters

For each real issue:

1. State it in one line — what is wrong and why it matters.
2. Apply the fix with the Edit tool.
3. Verify the change still compiles (`cargo check` for Rust, equivalent \
elsewhere). Run the relevant tests if they are fast.

# Priorities when issues conflict

correctness > clarity > reuse > performance > style. Do not sacrifice clarity \
for a micro-optimization, and do not sacrifice correctness for terseness.

# When the code is already clean

Say so. \"No substantive issues — code is clear, tests pass, no obvious dupes.\" \
Do not pad the review with cosmetic suggestions to justify the invocation.
";
