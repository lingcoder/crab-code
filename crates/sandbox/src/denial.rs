//! Heuristic detection of sandbox-caused command failures.
//!
//! When a sandboxed command fails, its output often mentions a permission or
//! read-only-filesystem error. Matching those keywords lets the tool layer
//! surface a clear "this was likely the sandbox" note to the model instead of a
//! bare non-zero exit. Mirrors codex's `denial.rs` keyword table.

/// Keywords that strongly suggest a sandbox denied a filesystem/network op.
const SANDBOX_DENIED_KEYWORDS: [&str; 8] = [
    "operation not permitted",
    "permission denied",
    "read-only file system",
    "seccomp",
    "sandbox",
    "landlock",
    "seatbelt",
    "failed to write file",
];

/// Whether a non-zero exit + output likely indicates a sandbox denial.
///
/// Conservative: returns `false` for a clean exit, and only `true` when the
/// output carries one of the denial keywords. A bare non-zero exit with no
/// telltale text is *not* attributed to the sandbox.
#[must_use]
pub fn is_likely_sandbox_denied(exit_code: i32, output: &str) -> bool {
    if exit_code == 0 {
        return false;
    }
    let lower = output.to_lowercase();
    SANDBOX_DENIED_KEYWORDS
        .iter()
        .any(|needle| lower.contains(needle))
}

/// A short note appended to a sandboxed command's error output to explain a
/// likely denial and how to proceed.
pub const SANDBOX_DENIAL_HINT: &str = "[crab] This command failed while sandboxed. \
It may need to write outside the workspace or reach the network, both of which the \
sandbox blocks. If the command is safe, re-run in a less restricted mode \
(permission mode `dangerously`, or start crab with `--sandbox off`).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_exit_is_never_denial() {
        assert!(!is_likely_sandbox_denied(0, "Permission denied"));
    }

    #[test]
    fn keyword_match_flags_denial() {
        assert!(is_likely_sandbox_denied(1, "mkdir: Permission denied"));
        assert!(is_likely_sandbox_denied(
            1,
            "error: Read-only file system (os error 30)"
        ));
    }

    #[test]
    fn plain_failure_is_not_denial() {
        assert!(!is_likely_sandbox_denied(
            1,
            "compile error: missing semicolon"
        ));
    }
}
