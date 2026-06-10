//! Background memory-consolidation cycles.
//!
//! autoDream runs when a session closes: it checks three cheap gates (time
//! since last run, number of new sessions since, lock-file availability)
//! and, if all three pass, spawns a forked agent that reads recent session
//! transcripts and rewrites the user's memory files to consolidate stale
//! information.
//!
//! This module is gated behind the `auto-dream` Cargo feature (default off)
//! and the `CRAB_AUTO_DREAM=1` env var. `SessionConfig::auto_dream_enabled`
//! lights up both.
//!
//! ## Status
//!
//! - **Done**: `AutoDreamConfig` (`{ min_hours, min_sessions, enabled }`);
//!   three-gate `DreamGate` check; `CONSOLIDATION_PROMPT` template;
//!   `AutoDream` bookkeeping state (cycle count, last-run timestamp);
//!   lock-file helper using `fd-lock`.
//! - **Deferred**: the actual forked-agent runner that sends the prompt to
//!   the LLM, applies the returned Edit/Write tool calls, and appends an
//!   `"Improved N memories"` system message to the main transcript. That
//!   requires plumbing an `AgentSession` fork path, which is cross-crate
//!   work; tracked as a follow-up. `run_dream_cycle` below is still a
//!   logging stub and returns [`DreamOutcome::Skipped`] with a clear reason.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default minimum hours between dream cycles.
pub const DEFAULT_MIN_HOURS: u32 = 24;

/// Default minimum new sessions before a dream is allowed.
pub const DEFAULT_MIN_SESSIONS: u32 = 5;

/// Default consolidation lock filename, stored under the session history root.
pub const DEFAULT_LOCK_FILENAME: &str = ".consolidate-lock";

/// Stale-lock threshold: any lock older than this is reclaimable even if the
/// PID appears live.
pub const STALE_LOCK_AFTER: Duration = Duration::from_secs(60 * 60);

/// Configuration controlling when a dream cycle is allowed to run.
#[derive(Debug, Clone)]
pub struct AutoDreamConfig {
    /// Whether auto-dream is enabled.
    pub enabled: bool,
    /// Minimum hours between cycles.
    pub min_hours: u32,
    /// Minimum number of new sessions since the last cycle.
    pub min_sessions: u32,
}

impl Default for AutoDreamConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_hours: DEFAULT_MIN_HOURS,
            min_sessions: DEFAULT_MIN_SESSIONS,
        }
    }
}

impl AutoDreamConfig {
    /// Build a config from env: `CRAB_AUTO_DREAM=1` enables, optional
    /// `CRAB_AUTO_DREAM_MIN_HOURS` / `CRAB_AUTO_DREAM_MIN_SESSIONS` tune the
    /// gates. Env absence ⇒ defaults.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_env_lookup(|k| std::env::var(k).ok())
    }

    fn from_env_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let enabled =
            lookup("CRAB_AUTO_DREAM").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "TRUE"));
        let min_hours = lookup("CRAB_AUTO_DREAM_MIN_HOURS")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_MIN_HOURS);
        let min_sessions = lookup("CRAB_AUTO_DREAM_MIN_SESSIONS")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_MIN_SESSIONS);
        Self {
            enabled,
            min_hours,
            min_sessions,
        }
    }
}

/// The reason a dream cycle ran, was skipped, or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamOutcome {
    /// One or more gates are closed; no dream ran.
    Skipped { reason: &'static str },
    /// A forked-agent dream ran and produced a reflection summary.
    Completed { files_touched: usize },
    /// A forked-agent dream ran and produced a text reflection.
    Reflected(String),
    /// The dream started but errored out; lock was rolled back.
    Failed,
}

/// Runtime state for the auto-dream subsystem.
#[derive(Debug)]
pub struct AutoDream {
    config: AutoDreamConfig,
    /// Timestamp of the last completed dream cycle, if any.
    last_dream: Option<Instant>,
    /// Total number of dream cycles attempted (skipped + completed).
    cycle_count: u64,
}

impl AutoDream {
    /// Create a new `AutoDream` instance with the given configuration.
    #[must_use]
    pub fn new(config: AutoDreamConfig) -> Self {
        Self {
            config,
            last_dream: None,
            cycle_count: 0,
        }
    }

    /// Check whether a dream cycle is currently allowed.
    ///
    /// `sessions_since_last` is the number of new session transcripts the
    /// caller has scanned since the previous consolidation. `lock_path` is
    /// the path to a lock file on disk; an unheld lock (or a stale one
    /// older than [`STALE_LOCK_AFTER`]) is considered open.
    ///
    /// Returns a specific gate reason when a check fails — useful for
    /// tracing/telemetry on which of the three gates closed.
    #[must_use]
    pub fn gate(&self, sessions_since_last: u32, lock_path: Option<&Path>) -> DreamGate {
        if !self.config.enabled {
            return DreamGate::Closed {
                reason: "auto-dream disabled in config",
            };
        }

        if let Some(last) = self.last_dream {
            let min_elapsed = Duration::from_secs(u64::from(self.config.min_hours) * 3600);
            if last.elapsed() < min_elapsed {
                return DreamGate::Closed {
                    reason: "time-since-last-dream < min_hours",
                };
            }
        }

        if sessions_since_last < self.config.min_sessions {
            return DreamGate::Closed {
                reason: "sessions-since-last-dream < min_sessions",
            };
        }

        if let Some(path) = lock_path
            && lock_is_held(path)
        {
            return DreamGate::Closed {
                reason: "consolidation lock is held by another process",
            };
        }

        DreamGate::Open
    }

    /// Legacy alias for [`AutoDream::gate`] returning a simple bool — kept
    /// so existing callers that only need a yes/no check keep working.
    #[must_use]
    pub fn should_dream(&self) -> bool {
        matches!(self.gate(u32::MAX, None), DreamGate::Open)
    }

    /// Run a dream cycle.
    ///
    /// Performs a heuristic reflection over the provided conversation context:
    /// extracts key decisions, unresolved questions, and error patterns.
    /// The reflection summary is returned as `DreamOutcome::Reflected`.
    ///
    /// A full implementation would fork an LLM agent to read session
    /// transcripts and rewrite memory files. For now, this produces a
    /// lightweight heuristic reflection.
    ///
    /// # Arguments
    ///
    /// * `recent_context` — concatenated text of recent conversation messages.
    pub fn run_dream_cycle(&mut self, recent_context: &str) -> DreamOutcome {
        self.last_dream = Some(Instant::now());
        self.cycle_count += 1;

        let reflection = generate_reflection(recent_context);

        tracing::info!(
            cycle = self.cycle_count,
            reflection_len = reflection.len(),
            "auto-dream cycle completed (heuristic reflection)"
        );

        DreamOutcome::Reflected(reflection)
    }

    /// Number of dream cycles that have been attempted (skipped or completed).
    #[must_use]
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }

    /// Whether auto-dream is enabled by configuration.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Access the current configuration.
    #[must_use]
    pub fn config(&self) -> &AutoDreamConfig {
        &self.config
    }

    /// Replace the configuration at runtime.
    pub fn set_config(&mut self, config: AutoDreamConfig) {
        self.config = config;
    }
}

/// Three-state gate result. `Open` means a dream may run right now; `Closed`
/// carries a short human-readable reason for tracing/telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamGate {
    Open,
    Closed { reason: &'static str },
}

/// Compute the default lock path: `{sessions_dir}/.consolidate-lock`.
#[must_use]
pub fn default_lock_path(sessions_dir: impl Into<PathBuf>) -> PathBuf {
    sessions_dir.into().join(DEFAULT_LOCK_FILENAME)
}

/// Best-effort check of whether a consolidation lock is currently held.
///
/// A missing file counts as "not held". A file whose mtime is older than
/// [`STALE_LOCK_AFTER`] is reclaimable. Any I/O error is logged and treated
/// as "not held" so a transient fs issue doesn't permanently block dreaming.
pub fn lock_is_held(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            tracing::warn!(lock_path = %path.display(), error = %e, "lock stat failed");
            false
        }
        Ok(meta) => {
            let modified = match meta.modified() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(lock_path = %path.display(), error = %e, "lock mtime unreadable");
                    return false;
                }
            };
            match modified.elapsed() {
                Ok(age) => age < STALE_LOCK_AFTER,
                Err(_) => true, // mtime in the future → assume live
            }
        }
    }
}

/// Generate a heuristic reflection summary from recent conversation context.
///
/// Extracts key patterns: errors encountered, files modified, decisions made,
/// and unresolved questions. A future version will call a small LLM for
/// richer, more nuanced reflections.
fn generate_reflection(context: &str) -> String {
    let lower = context.to_lowercase();
    let mut sections = Vec::new();

    // Extract error patterns.
    let error_count = lower.matches("error[").count()
        + lower.matches("could not compile").count()
        + lower.matches("FAILED").count();
    if error_count > 0 {
        sections.push(format!(
            "Encountered {error_count} error(s) during the session."
        ));
    }

    // Detect files mentioned.
    let file_mentions: Vec<&str> = context
        .lines()
        .filter(|line| {
            line.contains(".rs")
                || line.contains(".toml")
                || line.contains(".md")
                || line.contains(".json")
        })
        .take(5)
        .collect();
    if !file_mentions.is_empty() {
        sections.push(format!("Files discussed: {}", file_mentions.len()));
    }

    // Detect test activity.
    if lower.contains("test result:") || lower.contains("cargo test") {
        sections.push("Testing activity detected in the session.".to_string());
    }

    // Detect build activity.
    if lower.contains("cargo build") || lower.contains("cargo check") {
        sections.push("Build/check activity detected.".to_string());
    }

    if sections.is_empty() {
        "Dream cycle completed: no significant patterns detected in recent context.".to_string()
    } else {
        format!(
            "Dream reflection:\n{}",
            sections
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

/// Consolidation prompt body, with `{memory_root}` / `{transcript_dir}` /
/// `{session_ids}` placeholders filled in by [`build_consolidation_prompt`].
pub const CONSOLIDATION_PROMPT_TEMPLATE: &str = "\
# Dream: Memory Consolidation

You are performing a dream — a reflective pass over your memory files. \
Synthesize what you've learned recently into durable, well-organized \
memories so that future sessions can orient quickly.

Memory directory: `{memory_root}`
Session transcripts: `{transcript_dir}` (large JSONL files — grep narrowly)

## Phase 1 — Orient
- ls memory directory
- Read INDEX to understand current state
- Skim existing topic files

## Phase 2 — Gather recent signal
1. Daily logs (logs/YYYY/MM/YYYY-MM-DD.md)
2. Existing memories that drifted
3. Transcript search (grep narrowly)

## Phase 3 — Consolidate
Write or update memory files, focus on:
- Merging new signal into existing files
- Converting relative dates to absolute
- Deleting contradicted facts

## Phase 4 — Prune and index
Update INDEX to stay under MAX_ENTRYPOINT_LINES + ~25KB
- Remove stale pointers
- Shorten verbose entries
- Add newly important memories
- Resolve contradictions

---

Return a brief summary of what you consolidated, updated, or pruned.

**Tool constraints for this run:** Bash restricted to read-only \
(ls, find, grep, cat, stat, wc, head, tail). No writes.

Sessions since last consolidation ({session_count}):
{session_list}
";

/// Fill placeholders in [`CONSOLIDATION_PROMPT_TEMPLATE`].
#[allow(clippy::literal_string_with_formatting_args)]
#[must_use]
pub fn build_consolidation_prompt(
    memory_root: &Path,
    transcript_dir: &Path,
    session_ids: &[String],
) -> String {
    let session_list = if session_ids.is_empty() {
        "- (none — consolidation heuristic fallback)".to_string()
    } else {
        session_ids
            .iter()
            .map(|id| format!("- {id}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    CONSOLIDATION_PROMPT_TEMPLATE
        .replace("{memory_root}", &memory_root.display().to_string())
        .replace("{transcript_dir}", &transcript_dir.display().to_string())
        .replace("{session_count}", &session_ids.len().to_string())
        .replace("{session_list}", &session_list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool, min_hours: u32, min_sessions: u32) -> AutoDreamConfig {
        AutoDreamConfig {
            enabled,
            min_hours,
            min_sessions,
        }
    }

    #[test]
    fn default_config_uses_documented_defaults() {
        let c = AutoDreamConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.min_hours, DEFAULT_MIN_HOURS);
        assert_eq!(c.min_sessions, DEFAULT_MIN_SESSIONS);
    }

    #[test]
    fn from_env_disabled_without_flag() {
        let c = AutoDreamConfig::from_env_lookup(|_| None);
        assert!(!c.enabled);
    }

    #[test]
    fn from_env_parses_tuning_vars() {
        let c = AutoDreamConfig::from_env_lookup(|k| match k {
            "CRAB_AUTO_DREAM" => Some("1".into()),
            "CRAB_AUTO_DREAM_MIN_HOURS" => Some("6".into()),
            "CRAB_AUTO_DREAM_MIN_SESSIONS" => Some("2".into()),
            _ => None,
        });
        assert!(c.enabled);
        assert_eq!(c.min_hours, 6);
        assert_eq!(c.min_sessions, 2);
    }

    #[test]
    fn gate_disabled_is_always_closed() {
        let d = AutoDream::new(cfg(false, 0, 0));
        assert!(matches!(d.gate(100, None), DreamGate::Closed { .. }));
    }

    #[test]
    fn gate_too_few_sessions_is_closed() {
        let d = AutoDream::new(cfg(true, 0, 5));
        let res = d.gate(3, None);
        assert!(matches!(res, DreamGate::Closed { reason } if reason.contains("sessions")));
    }

    #[test]
    fn gate_time_window_blocks_when_recent() {
        let mut d = AutoDream::new(cfg(true, 24, 0));
        d.run_dream_cycle("test context");
        let res = d.gate(100, None);
        assert!(matches!(res, DreamGate::Closed { reason } if reason.contains("time")));
    }

    #[test]
    fn gate_opens_when_all_passes() {
        let d = AutoDream::new(cfg(true, 0, 5));
        assert_eq!(d.gate(5, None), DreamGate::Open);
    }

    #[test]
    fn lock_is_held_missing_file_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!lock_is_held(&dir.path().join(".consolidate-lock")));
    }

    #[test]
    fn lock_is_held_fresh_file_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".consolidate-lock");
        std::fs::write(&path, b"pid-12345").unwrap();
        assert!(lock_is_held(&path));
    }

    #[test]
    fn run_dream_cycle_updates_state() {
        let mut d = AutoDream::new(cfg(true, 0, 0));
        assert_eq!(d.cycle_count(), 0);
        let outcome = d.run_dream_cycle("some context");
        assert!(matches!(outcome, DreamOutcome::Reflected(_)));
        assert_eq!(d.cycle_count(), 1);
        assert!(d.last_dream.is_some());
    }

    #[test]
    fn run_dream_cycle_empty_context() {
        let mut d = AutoDream::new(cfg(true, 0, 0));
        let outcome = d.run_dream_cycle("");
        assert!(
            matches!(outcome, DreamOutcome::Reflected(ref text) if text.contains("no significant patterns"))
        );
    }

    #[test]
    fn run_dream_cycle_detects_errors() {
        let mut d = AutoDream::new(cfg(true, 0, 0));
        let context = "error[E0425]: cannot find value `foo`\nerror[E0308]: mismatched types";
        let outcome = d.run_dream_cycle(context);
        match outcome {
            DreamOutcome::Reflected(text) => {
                assert!(text.contains("2 error"));
            }
            _ => panic!("expected Reflected"),
        }
    }

    #[test]
    fn generate_reflection_empty() {
        let r = generate_reflection("");
        assert!(r.contains("no significant patterns"));
    }

    #[test]
    fn generate_reflection_errors() {
        let r = generate_reflection("error[E0425]: not found\ncould not compile");
        assert!(r.contains("error"));
    }

    #[test]
    fn generate_reflection_tests() {
        let r = generate_reflection("test result: FAILED. 3 passed; 1 failed");
        assert!(r.contains("Testing activity"));
    }

    #[test]
    fn consolidation_prompt_substitutes_placeholders() {
        let prompt = build_consolidation_prompt(
            Path::new("/home/u/.crab/memory"),
            Path::new("/home/u/.crab/sessions"),
            &["sess-1".into(), "sess-2".into()],
        );
        assert!(prompt.contains("/home/u/.crab/memory"));
        assert!(prompt.contains("/home/u/.crab/sessions"));
        assert!(prompt.contains("Sessions since last consolidation (2):"));
        assert!(prompt.contains("- sess-1"));
        assert!(prompt.contains("- sess-2"));
    }

    #[test]
    fn consolidation_prompt_handles_zero_sessions() {
        let prompt = build_consolidation_prompt(Path::new("/mem"), Path::new("/sess"), &[]);
        assert!(prompt.contains("Sessions since last consolidation (0):"));
        assert!(prompt.contains("heuristic fallback"));
    }

    #[test]
    fn default_lock_path_joins_sessions_dir() {
        let p = default_lock_path(Path::new("/foo/bar"));
        assert_eq!(p, PathBuf::from("/foo/bar/.consolidate-lock"));
    }
}
