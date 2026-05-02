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
        .expect("built-in skill 'remember' must be valid")
}

const PROMPT: &str = "\
Save the given information to the memory system so it persists across sessions.

# Memory file format

Each memory is a single markdown file with YAML frontmatter:

```markdown
---
name: short title
description: one-line hook used to decide relevance in future conversations
type: user | project | feedback | reference
---

Body in markdown. For feedback/project memories include **Why:** and \
**How to apply:** lines.
```

# Memory types

- **user** — the person's role, preferences, expertise (\"senior Rust dev, new \
to React\").
- **project** — current work, decisions, deadlines, motivations not derivable \
from code (\"merge freeze starts 2026-03-05\").
- **feedback** — corrections or validated approaches the user gave \
(\"don't mock the DB in integration tests\"). Include the reason.
- **reference** — pointers to external systems (\"pipeline bugs tracked in \
Linear project INGEST\").

# Workflow

1. Parse the user's input.
2. Decide which type fits best. If ambiguous, prefer the narrowest type \
(reference > feedback > project > user).
3. Pick a kebab-case filename derived from the name \
(e.g. `prefer-rust-nightly.md`, `no-emoji-commits.md`).
4. Glob the memory directory and Read any near-duplicate to decide between \
**update existing** and **write new**. Don't create two files for the same fact.
5. Write the file with the Write tool to the configured memory directory.
6. Append a one-line index entry to `MEMORY.md`: \
`- [Title](filename.md) — one-line hook`.
7. Confirm to the user with the file path and type.

# Examples

- \"prefer Rust nightly for this repo\" → `project`, file \
`prefer-rust-nightly.md`.
- \"no emoji in commits\" → `feedback`, file `no-emoji-commits.md`, with a \
**Why:** line if the user gave one.
- \"staging API is at api-staging.example.com\" → `reference`, file \
`staging-api-url.md`.

Never save secrets, ephemeral task state, or anything already documented in \
CLAUDE.md.
";
