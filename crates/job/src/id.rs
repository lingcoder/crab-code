//! Stable identifiers for scheduled jobs.
//!
//! A `JobId` is a process-stable string; cron jobs persist their id so
//! that after a restart callers can locate the same job and cancel /
//! reschedule it. In-memory interval jobs use the same id space but
//! do not survive restart.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable, opaque identifier for a scheduled job.
///
/// Constructed by the scheduler (typically `<kind>-<ulid>`). Clients
/// should treat it as opaque and use it only for cancel / query calls.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub String);

impl JobId {
    /// Borrow the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What kind of schedule a job follows. Stored alongside [`JobId`] so
/// consumers can filter ("show me all heartbeats" vs "show me all cron
/// jobs") without a separate lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    /// Fires exactly once at a future instant or after a delay.
    OneShot,
    /// Fires every N duration, starting at a reference instant.
    Interval,
    /// Fires according to a [`croner`](https://docs.rs/croner) expression.
    Cron,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_is_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(JobId("cron-01".into()));
        assert!(set.contains(&JobId("cron-01".into())));
        assert!(!set.contains(&JobId("cron-02".into())));
    }

    #[test]
    fn job_id_displays_as_inner_string() {
        let id = JobId("interval-abc".into());
        assert_eq!(format!("{id}"), "interval-abc");
    }

    #[test]
    fn job_kind_serde_roundtrip() {
        let k = JobKind::Cron;
        let json = serde_json::to_string(&k).unwrap();
        assert_eq!(json, "\"Cron\"");
        let back: JobKind = serde_json::from_str(&json).unwrap();
        assert_eq!(k, back);
    }
}
