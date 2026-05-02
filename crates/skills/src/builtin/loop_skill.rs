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
        .expect("built-in skill 'loop' must be valid")
}

const PROMPT: &str = "\
Run a prompt or slash command on a recurring interval.

# Parsing the request

Split the user input into an optional interval and a task description.

- Interval tokens look like `30s`, `5m`, `1h`, `2h30m`, or `every 10 minutes`.
- If no interval is given, self-pace based on the task: short polls (~30s) for \
fast-moving status, longer waits (5-15m) for builds, deploys, or PR checks.
- Everything after the interval is the task. It may itself be a slash command \
(e.g. `/loop 5m /commit`) — invoke that skill on every iteration.

# Execution cycle

Repeat until the user cancels:

1. Execute the task once.
2. Report what happened in 1-2 lines (status, key change, error).
3. Wait the chosen interval.
4. Loop.

Track an iteration counter and surface it (`iteration 4`) so the user can see \
progress without scrolling.

# Common patterns

- Status polling — `/loop 1m gh pr checks 123` watches a CI run.
- File watching — re-read a log or source file each tick and diff against last.
- Monitoring — periodically curl an endpoint, ping a service, query a DB.
- Recurring maintenance — re-run formatters, regenerate docs, refresh caches.

# Error handling

Transient failures (network blip, rate limit, missing file that may appear) do \
NOT stop the loop — log the error briefly and continue. Only halt for:

- A persistent failure that has occurred 3+ iterations in a row.
- A non-recoverable error (auth revoked, target deleted).
- An explicit user request to stop.

# Before starting

Always echo back the parsed plan once and wait for confirmation:

> Looping `<task>` every `<interval>`. Reply `stop` to cancel.

This avoids burning cycles on a misread interval or wrong target.
";
