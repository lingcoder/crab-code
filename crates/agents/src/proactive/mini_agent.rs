//! Forked speculation mini-agent — runs a cheap model in parallel with
//! the main query loop to propose likely next actions.
//!
//! The current implementation uses simple pattern matching on conversation
//! context to generate suggestions. A future version will call a small/fast
//! LLM for richer suggestions.

use super::suggestion::{ActionType, Suggestion};

/// A lightweight agent that analyzes conversation context and generates
/// proactive suggestions.
pub struct ProactiveAgent;

impl ProactiveAgent {
    /// Analyze a conversation transcript (recent messages) and return
    /// ranked suggestions for the user's next action.
    ///
    /// The `context` parameter is a concatenation of recent message texts.
    /// For now this uses pattern-matching heuristics.
    #[must_use]
    pub fn analyze(context: &str) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();
        let lower = context.to_lowercase();

        // Detect build/test failures.
        if lower.contains("error[e") || lower.contains("could not compile") {
            suggestions.push(Suggestion::new(
                "Fix compilation errors",
                "The last build failed with compilation errors. Review the error messages and fix the source files.",
                0.9,
                ActionType::FixError("compilation".into()),
            ));
        }

        if lower.contains("test result: failed")
            || lower.contains("FAILED") && lower.contains("test")
        {
            suggestions.push(Suggestion::new(
                "Investigate test failures",
                "Tests are failing. Review the test output to identify which tests failed and why.",
                0.85,
                ActionType::RunCommand("cargo nextest run".into()),
            ));
        }

        if lower.contains("warning[") || lower.contains("cargo warning") {
            suggestions.push(Suggestion::new(
                "Fix compiler warnings",
                "Compiler warnings were detected. Address them to keep the build clean.",
                0.5,
                ActionType::RunCommand("cargo clippy --workspace".into()),
            ));
        }

        // Detect file-not-found or path errors.
        if lower.contains("no such file") || lower.contains("file not found") {
            suggestions.push(Suggestion::new(
                "Check file paths",
                "A file was not found. Verify the path exists and is correctly spelled.",
                0.7,
                ActionType::Advice,
            ));
        }

        // Suggest running tests after code changes.
        if lower.contains("cargo build") && lower.contains("finished") {
            suggestions.push(Suggestion::new(
                "Run tests",
                "Build succeeded. Run the test suite to verify nothing is broken.",
                0.6,
                ActionType::RunCommand("cargo nextest run --workspace".into()),
            ));
        }

        // Suggest format/lint after edits.
        if lower.contains("wrote") || lower.contains("edited") || lower.contains("modified") {
            suggestions.push(Suggestion::new(
                "Check formatting",
                "Files were recently modified. Run format and lint checks.",
                0.4,
                ActionType::RunCommand("cargo fmt --all && cargo clippy --workspace".into()),
            ));
        }

        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_empty_context() {
        let suggestions = ProactiveAgent::analyze("");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn analyze_detects_compilation_error() {
        let context = "error[E0425]: cannot find value `foo` in this scope\n --> src/main.rs:5:5";
        let suggestions = ProactiveAgent::analyze(context);
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.title.contains("compilation")));
    }

    #[test]
    fn analyze_detects_test_failure() {
        let context = "test result: FAILED. 1 passed; 1 failed; 0 ignored";
        let suggestions = ProactiveAgent::analyze(context);
        assert!(suggestions.iter().any(|s| s.title.contains("test")));
    }

    #[test]
    fn analyze_detects_warnings() {
        let context = "warning[unused]: variable `x` is never used";
        let suggestions = ProactiveAgent::analyze(context);
        assert!(suggestions.iter().any(|s| s.title.contains("warnings")));
    }

    #[test]
    fn analyze_detects_build_success() {
        let context =
            "cargo build --release\n   Compiling crab-code v0.1.0\n    Finished `release` profile";
        let suggestions = ProactiveAgent::analyze(context);
        assert!(suggestions.iter().any(|s| s.title.contains("Run tests")));
    }

    #[test]
    fn analyze_detects_file_not_found() {
        let context = "No such file or directory: ./missing.rs";
        let suggestions = ProactiveAgent::analyze(context);
        assert!(suggestions.iter().any(|s| s.title.contains("file paths")));
    }

    #[test]
    fn analyze_multiple_signals() {
        let context =
            "error[E0308]: mismatched types\nwarning[unused]: dead code\ntest result: FAILED";
        let suggestions = ProactiveAgent::analyze(context);
        // Should have at least compilation error + test failure + warnings
        assert!(suggestions.len() >= 2);
    }

    #[test]
    fn suggestion_confidence_is_valid() {
        let suggestions = ProactiveAgent::analyze("error[E0425]: not found");
        for s in &suggestions {
            assert!(s.confidence >= 0.0 && s.confidence <= 1.0);
        }
    }
}
