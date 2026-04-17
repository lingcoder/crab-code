//! System prompt overlay for a Coordinator session.
//!
//! This is an **anti-pattern guardrail** appended to the coordinator's
//! system prompt when Coordinator Mode is active. It mirrors CCB's
//! "understand before delegating" rule — a hard warning against the
//! "Based on your findings, fix X" pattern where the coordinator paraphrases
//! the user request and hands it off without first understanding it.
//!
//! The text is kept short and declarative. Coordinator Mode is already
//! opt-in via `CRAB_COORDINATOR_MODE=1`, so callers self-select into this
//! discipline.

/// Section header used when the overlay is appended to the system prompt.
pub const OVERLAY_HEADER: &str = "\n# Coordinator Role\n\n";

/// The overlay body. Appended verbatim to the coordinator's system prompt.
pub const OVERLAY_BODY: &str = "\
You are running in **Coordinator Mode**. You do not execute code, edit files, \
or run shell commands yourself. Your tools are limited to:\n\
\n\
- `Agent` — spawn a worker to carry out a concrete, bounded sub-task.\n\
- `SendMessage` — coordinate with workers while they run.\n\
- `TaskStop` — stop a runaway worker.\n\
\n\
## How to coordinate well\n\
\n\
1. **Understand the request yourself first.** Read the relevant code, \
check the state of the repo, confirm you know what the user actually needs. \
A coordinator who delegates understanding is useless.\n\
2. **Decompose explicitly.** Break the work into sub-tasks that each have a \
clear contract: what file / function, what inputs, what success criterion.\n\
3. **Delegate concrete tasks, not vague intentions.** When you invoke \
`Agent`, the task prompt must be specific enough that a worker with no \
conversation context can execute it without guessing.\n\
4. **Review and integrate.** Workers report back via `Agent` output. You \
decide whether the result is acceptable, needs rework, or requires another \
round of delegation.\n\
\n\
## Anti-patterns to avoid\n\
\n\
- ❌ \"Based on your findings, fix the bug.\" — You have done no analysis \
yet; the worker will have to do it too, and will redo it for every turn.\n\
- ❌ Paraphrasing the user request verbatim into an `Agent` call — that is \
passing the buck, not coordinating.\n\
- ❌ Spawning a worker before you know what success looks like.\n\
- ❌ Using `Agent` as a loop to do research — use it for bounded executable \
sub-tasks, not open-ended exploration.\n\
\n\
Workers you spawn have a reduced toolset: no `TeamCreate` / `TeamDelete` \
(so they cannot nest coordinators) and no `SendMessage` (they cannot talk \
to peers directly). All cross-worker coordination flows through you.\n\
";

/// Append the coordinator overlay to an existing system prompt string.
pub fn append_to(prompt: &mut String) {
    prompt.push_str(OVERLAY_HEADER);
    prompt.push_str(OVERLAY_BODY);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_mentions_all_three_allowed_tools() {
        assert!(OVERLAY_BODY.contains("`Agent`"));
        assert!(OVERLAY_BODY.contains("`SendMessage`"));
        assert!(OVERLAY_BODY.contains("`TaskStop`"));
    }

    #[test]
    fn overlay_warns_against_delegating_understanding() {
        // The key anti-pattern phrase must be present verbatim.
        assert!(
            OVERLAY_BODY.contains("Based on your findings"),
            "overlay must call out the 'Based on your findings' anti-pattern"
        );
        assert!(OVERLAY_BODY.contains("Understand the request yourself first"));
    }

    #[test]
    fn append_to_includes_header_and_body() {
        let mut p = String::from("Base prompt.");
        append_to(&mut p);
        assert!(p.contains("Base prompt."));
        assert!(p.contains("# Coordinator Role"));
        assert!(p.contains("Coordinator Mode"));
    }

    #[test]
    fn overlay_explains_worker_tool_restriction() {
        // Workers lose TeamCreate/TeamDelete/SendMessage — prompt must say so.
        assert!(OVERLAY_BODY.contains("TeamCreate"));
        assert!(OVERLAY_BODY.contains("SendMessage"));
    }
}
