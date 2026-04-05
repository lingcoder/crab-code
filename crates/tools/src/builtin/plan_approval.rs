//! Plan approval workflow — tracks review status and feedback history.
//!
//! A `PlanApproval` manages the lifecycle of a plan through draft, review,
//! approval, rejection, and revision states. Each state change is recorded
//! in an `ApprovalHistory` with optional feedback.

use std::fmt;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Types ────────────────────────────────────────────────────────────

/// Approval status of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    Draft,
    PendingReview,
    Approved,
    Rejected,
    Revised,
}

impl fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft => f.write_str("draft"),
            Self::PendingReview => f.write_str("pending_review"),
            Self::Approved => f.write_str("approved"),
            Self::Rejected => f.write_str("rejected"),
            Self::Revised => f.write_str("revised"),
        }
    }
}

impl ApprovalStatus {
    /// Parse from a string representation.
    #[must_use]
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.trim() {
            "draft" => Some(Self::Draft),
            "pending_review" => Some(Self::PendingReview),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "revised" => Some(Self::Revised),
            _ => None,
        }
    }
}

/// A single record in the approval history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRecord {
    /// Unix timestamp (seconds) when this record was created.
    pub timestamp: u64,
    /// Status at time of recording.
    pub status: ApprovalStatus,
    /// Optional feedback or reason for the status change.
    pub feedback: Option<String>,
}

/// Manages the approval workflow for a plan.
#[derive(Debug, Clone)]
pub struct PlanApproval {
    /// Current approval status.
    pub status: ApprovalStatus,
    /// Chronological history of status changes.
    pub history: Vec<ApprovalRecord>,
}

impl Default for PlanApproval {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanApproval {
    /// Create a new approval in Draft state.
    #[must_use]
    pub fn new() -> Self {
        let record = ApprovalRecord {
            timestamp: now_secs(),
            status: ApprovalStatus::Draft,
            feedback: None,
        };
        Self {
            status: ApprovalStatus::Draft,
            history: vec![record],
        }
    }

    /// Submit the plan for review. Only valid from Draft or Revised state.
    pub fn submit_for_review(&mut self) -> bool {
        if self.status != ApprovalStatus::Draft && self.status != ApprovalStatus::Revised {
            return false;
        }
        self.transition(ApprovalStatus::PendingReview, None)
    }

    /// Approve the plan. Only valid from `PendingReview` state.
    pub fn approve(&mut self, feedback: Option<String>) -> bool {
        if self.status != ApprovalStatus::PendingReview {
            return false;
        }
        self.transition(ApprovalStatus::Approved, feedback)
    }

    /// Reject the plan. Only valid from `PendingReview` state.
    pub fn reject(&mut self, feedback: Option<String>) -> bool {
        if self.status != ApprovalStatus::PendingReview {
            return false;
        }
        self.transition(ApprovalStatus::Rejected, feedback)
    }

    /// Request revision of a rejected plan. Only valid from Rejected state.
    pub fn request_revision(&mut self, feedback: Option<String>) -> bool {
        if self.status != ApprovalStatus::Rejected {
            return false;
        }
        self.transition(ApprovalStatus::Revised, feedback)
    }

    /// Number of times the plan has been through review (submitted for review).
    #[must_use]
    pub fn review_count(&self) -> usize {
        self.history
            .iter()
            .filter(|r| r.status == ApprovalStatus::PendingReview)
            .count()
    }

    /// Whether the plan is in a terminal approved state.
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.status == ApprovalStatus::Approved
    }

    /// Whether the plan can be executed (only when approved).
    #[must_use]
    pub fn can_execute(&self) -> bool {
        self.is_approved()
    }

    /// Get the latest feedback from the history, if any.
    #[must_use]
    pub fn latest_feedback(&self) -> Option<&str> {
        self.history
            .iter()
            .rev()
            .find_map(|r| r.feedback.as_deref())
    }

    /// Format a human-readable summary of the approval state.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = format!("Status: {}", self.status);
        let _ = write!(out, "\nReview cycles: {}", self.review_count());
        if let Some(fb) = self.latest_feedback() {
            let _ = write!(out, "\nLatest feedback: {fb}");
        }
        out
    }

    /// Record a state transition.
    fn transition(&mut self, new_status: ApprovalStatus, feedback: Option<String>) -> bool {
        self.status = new_status;
        self.history.push(ApprovalRecord {
            timestamp: now_secs(),
            status: new_status,
            feedback,
        });
        true
    }
}

/// Get current time as seconds since UNIX epoch.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_approval_is_draft() {
        let approval = PlanApproval::new();
        assert_eq!(approval.status, ApprovalStatus::Draft);
        assert_eq!(approval.history.len(), 1);
        assert_eq!(approval.history[0].status, ApprovalStatus::Draft);
    }

    #[test]
    fn submit_from_draft() {
        let mut approval = PlanApproval::new();
        assert!(approval.submit_for_review());
        assert_eq!(approval.status, ApprovalStatus::PendingReview);
        assert_eq!(approval.history.len(), 2);
    }

    #[test]
    fn submit_from_invalid_state_fails() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        // Already pending, can't submit again
        assert!(!approval.submit_for_review());
    }

    #[test]
    fn approve_from_pending() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        assert!(approval.approve(Some("Looks good!".into())));
        assert_eq!(approval.status, ApprovalStatus::Approved);
        assert!(approval.is_approved());
        assert!(approval.can_execute());
    }

    #[test]
    fn approve_from_invalid_state_fails() {
        let mut approval = PlanApproval::new();
        assert!(!approval.approve(None));
    }

    #[test]
    fn reject_from_pending() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        assert!(approval.reject(Some("Needs more detail".into())));
        assert_eq!(approval.status, ApprovalStatus::Rejected);
        assert!(!approval.is_approved());
        assert!(!approval.can_execute());
    }

    #[test]
    fn reject_from_invalid_state_fails() {
        let mut approval = PlanApproval::new();
        assert!(!approval.reject(None));
    }

    #[test]
    fn request_revision_from_rejected() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        approval.reject(Some("Too vague".into()));
        assert!(approval.request_revision(Some("Added details".into())));
        assert_eq!(approval.status, ApprovalStatus::Revised);
    }

    #[test]
    fn request_revision_from_invalid_state_fails() {
        let mut approval = PlanApproval::new();
        assert!(!approval.request_revision(None));
    }

    #[test]
    fn full_workflow_draft_to_approved() {
        let mut approval = PlanApproval::new();
        assert!(approval.submit_for_review());
        assert!(approval.reject(Some("Missing tests section".into())));
        assert!(approval.request_revision(Some("Will add tests".into())));
        assert!(approval.submit_for_review());
        assert!(approval.approve(Some("LGTM".into())));

        assert!(approval.is_approved());
        assert_eq!(approval.history.len(), 6); // draft + 5 transitions
        assert_eq!(approval.review_count(), 2);
    }

    #[test]
    fn review_count_tracks_submissions() {
        let mut approval = PlanApproval::new();
        assert_eq!(approval.review_count(), 0);
        approval.submit_for_review();
        assert_eq!(approval.review_count(), 1);
        approval.reject(None);
        approval.request_revision(None);
        approval.submit_for_review();
        assert_eq!(approval.review_count(), 2);
    }

    #[test]
    fn latest_feedback_returns_most_recent() {
        let mut approval = PlanApproval::new();
        assert!(approval.latest_feedback().is_none());
        approval.submit_for_review();
        approval.reject(Some("Bad".into()));
        assert_eq!(approval.latest_feedback(), Some("Bad"));
        approval.request_revision(Some("Fixed".into()));
        assert_eq!(approval.latest_feedback(), Some("Fixed"));
    }

    #[test]
    fn latest_feedback_skips_none() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        approval.reject(Some("Issue found".into()));
        approval.request_revision(None); // no feedback
        // Should still find "Issue found" as latest feedback with content
        // Actually request_revision(None) pushes a record with None feedback,
        // so latest_feedback scans backwards and skips it.
        assert_eq!(approval.latest_feedback(), Some("Issue found"));
    }

    #[test]
    fn summary_format() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        approval.reject(Some("Needs work".into()));
        let s = approval.summary();
        assert!(s.contains("Status: rejected"));
        assert!(s.contains("Review cycles: 1"));
        assert!(s.contains("Latest feedback: Needs work"));
    }

    #[test]
    fn approval_status_display() {
        assert_eq!(ApprovalStatus::Draft.to_string(), "draft");
        assert_eq!(ApprovalStatus::PendingReview.to_string(), "pending_review");
        assert_eq!(ApprovalStatus::Approved.to_string(), "approved");
        assert_eq!(ApprovalStatus::Rejected.to_string(), "rejected");
        assert_eq!(ApprovalStatus::Revised.to_string(), "revised");
    }

    #[test]
    fn approval_status_from_str() {
        assert_eq!(
            ApprovalStatus::from_str_value("draft"),
            Some(ApprovalStatus::Draft)
        );
        assert_eq!(
            ApprovalStatus::from_str_value("pending_review"),
            Some(ApprovalStatus::PendingReview)
        );
        assert_eq!(
            ApprovalStatus::from_str_value("approved"),
            Some(ApprovalStatus::Approved)
        );
        assert_eq!(
            ApprovalStatus::from_str_value("rejected"),
            Some(ApprovalStatus::Rejected)
        );
        assert_eq!(
            ApprovalStatus::from_str_value("revised"),
            Some(ApprovalStatus::Revised)
        );
        assert_eq!(ApprovalStatus::from_str_value("unknown"), None);
    }

    #[test]
    fn default_is_new() {
        let a = PlanApproval::default();
        assert_eq!(a.status, ApprovalStatus::Draft);
        assert_eq!(a.history.len(), 1);
    }

    #[test]
    fn cannot_approve_after_approved() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        approval.approve(None);
        // Already approved, cannot approve again
        assert!(!approval.approve(None));
    }

    #[test]
    fn cannot_reject_after_approved() {
        let mut approval = PlanApproval::new();
        approval.submit_for_review();
        approval.approve(None);
        assert!(!approval.reject(None));
    }

    #[test]
    fn history_timestamps_are_non_zero() {
        let approval = PlanApproval::new();
        assert!(approval.history[0].timestamp > 0);
    }

    #[test]
    fn can_execute_only_when_approved() {
        let mut approval = PlanApproval::new();
        assert!(!approval.can_execute());
        approval.submit_for_review();
        assert!(!approval.can_execute());
        approval.approve(None);
        assert!(approval.can_execute());
    }
}
