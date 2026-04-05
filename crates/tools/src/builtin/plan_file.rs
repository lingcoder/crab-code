//! Structured plan file representation and Markdown parsing/rendering.
//!
//! A `PlanFile` consists of one or more `PlanSection`s, each containing
//! ordered `PlanStep`s with status tracking. Plans can be parsed from
//! Markdown and rendered back with status markers.

use std::fmt;
use std::fmt::Write as _;

// ── Types ────────────────────────────────────────────────────────────

/// Status of an individual plan step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
    Skipped,
}

impl PlanStepStatus {
    /// Markdown checkbox representation.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
            Self::Skipped => "[-]",
        }
    }

    /// Parse from a Markdown checkbox marker.
    #[must_use]
    pub fn from_marker(s: &str) -> Self {
        match s.trim() {
            "[x]" | "[X]" => Self::Completed,
            "[~]" => Self::InProgress,
            "[-]" => Self::Skipped,
            _ => Self::Pending,
        }
    }
}

impl fmt::Display for PlanStepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => f.write_str("pending"),
            Self::InProgress => f.write_str("in_progress"),
            Self::Completed => f.write_str("completed"),
            Self::Skipped => f.write_str("skipped"),
        }
    }
}

/// A single step within a plan section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    /// Description of this step.
    pub description: String,
    /// Current status.
    pub status: PlanStepStatus,
    /// Files expected to be affected by this step.
    pub files_affected: Vec<String>,
    /// Estimated complexity: "low", "medium", "high".
    pub estimated_complexity: Option<String>,
}

/// A section within a plan (e.g., a phase or module grouping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSection {
    /// Section title.
    pub title: String,
    /// Ordered steps in this section.
    pub steps: Vec<PlanStep>,
}

impl PlanSection {
    /// Count of completed steps.
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.status == PlanStepStatus::Completed)
            .count()
    }

    /// Whether all steps are completed or skipped.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.steps
            .iter()
            .all(|s| s.status == PlanStepStatus::Completed || s.status == PlanStepStatus::Skipped)
    }
}

/// A complete plan file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanFile {
    /// Plan title / objective.
    pub title: String,
    /// Sections in the plan.
    pub sections: Vec<PlanSection>,
}

impl PlanFile {
    /// Total number of steps across all sections.
    #[must_use]
    pub fn total_steps(&self) -> usize {
        self.sections.iter().map(|s| s.steps.len()).sum()
    }

    /// Total completed steps across all sections.
    #[must_use]
    pub fn completed_steps(&self) -> usize {
        self.sections.iter().map(PlanSection::completed_count).sum()
    }

    /// Completion percentage (0-100).
    #[must_use]
    pub fn completion_pct(&self) -> u8 {
        let total = self.total_steps();
        if total == 0 {
            return 100;
        }
        #[allow(clippy::cast_possible_truncation)]
        let pct = (self.completed_steps() * 100 / total) as u8;
        pct
    }

    /// Whether the entire plan is done.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.sections.iter().all(PlanSection::is_done)
    }

    /// Mark a step as completed by section index and step index.
    pub fn complete_step(&mut self, section: usize, step: usize) -> bool {
        if let Some(sec) = self.sections.get_mut(section)
            && let Some(st) = sec.steps.get_mut(step)
        {
            st.status = PlanStepStatus::Completed;
            return true;
        }
        false
    }
}

// ── Markdown parsing ─────────────────────────────────────────────────

/// Parse a Markdown plan into a `PlanFile`.
///
/// Expected format:
/// ```markdown
/// # Plan Title
///
/// ## Section 1
/// - [x] Step one
/// - [ ] Step two (files: a.rs, b.rs) (complexity: high)
/// - [~] Step three
/// ```
#[must_use]
pub fn parse_plan(markdown: &str) -> PlanFile {
    let mut title = String::new();
    let mut sections = Vec::new();
    let mut current_section: Option<PlanSection> = None;

    for line in markdown.lines() {
        let trimmed = line.trim();

        // H1 = plan title
        if let Some(t) = trimmed.strip_prefix("# ") {
            t.trim().clone_into(&mut title);
            continue;
        }

        // H2 = section title
        if let Some(t) = trimmed.strip_prefix("## ") {
            if let Some(sec) = current_section.take() {
                sections.push(sec);
            }
            current_section = Some(PlanSection {
                title: t.trim().to_owned(),
                steps: Vec::new(),
            });
            continue;
        }

        // List item with checkbox = step
        if let Some(rest) = trimmed.strip_prefix("- ")
            && let Some(step) = parse_step_line(rest)
        {
            if current_section.is_none() {
                current_section = Some(PlanSection {
                    title: "Default".to_owned(),
                    steps: Vec::new(),
                });
            }
            if let Some(sec) = current_section.as_mut() {
                sec.steps.push(step);
            }
        }
    }

    if let Some(sec) = current_section {
        sections.push(sec);
    }

    PlanFile { title, sections }
}

/// Parse a single step line after the `- ` prefix.
fn parse_step_line(line: &str) -> Option<PlanStep> {
    // Expected: "[x] description (files: a, b) (complexity: high)"
    let (status, rest) = if line.len() >= 3 && line.starts_with('[') {
        line.find(']').map_or((PlanStepStatus::Pending, line), |close| {
            let marker = &line[..=close];
            let status = PlanStepStatus::from_marker(marker);
            (status, line[close + 1..].trim())
        })
    } else {
        (PlanStepStatus::Pending, line)
    };

    if rest.is_empty() {
        return None;
    }

    let mut description = rest.to_owned();
    let mut files_affected = Vec::new();
    let mut estimated_complexity = None;

    // Extract (files: ...) annotation
    if let Some(start) = description.find("(files:")
        && let Some(end) = description[start..].find(')')
    {
        let files_str = &description[start + 7..start + end];
        files_affected = files_str
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        format!(
            "{}{}",
            description[..start].trim(),
            description[start + end + 1..].trim()
        )
        .trim()
        .clone_into(&mut description);
    }

    // Extract (complexity: ...) annotation
    if let Some(start) = description.find("(complexity:")
        && let Some(end) = description[start..].find(')')
    {
        let c = description[start + 12..start + end].trim().to_owned();
        if !c.is_empty() {
            estimated_complexity = Some(c);
        }
        format!(
            "{}{}",
            description[..start].trim(),
            description[start + end + 1..].trim()
        )
        .trim()
        .clone_into(&mut description);
    }

    Some(PlanStep {
        description,
        status,
        files_affected,
        estimated_complexity,
    })
}

// ── Markdown rendering ───────────────────────────────────────────────

/// Render a `PlanFile` back to Markdown with status markers.
#[must_use]
pub fn render_plan(plan: &PlanFile) -> String {
    let mut out = String::new();

    if !plan.title.is_empty() {
        let _ = writeln!(out, "# {}", plan.title);
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Progress: {}/{} steps ({}%)",
            plan.completed_steps(),
            plan.total_steps(),
            plan.completion_pct()
        );
        let _ = writeln!(out);
    }

    for section in &plan.sections {
        let _ = writeln!(out, "## {}", section.title);
        for step in &section.steps {
            let _ = write!(out, "- {} {}", step.status.marker(), step.description);
            if !step.files_affected.is_empty() {
                let _ = write!(out, " (files: {})", step.files_affected.join(", "));
            }
            if let Some(c) = &step.estimated_complexity {
                let _ = write!(out, " (complexity: {c})");
            }
            out.push('\n');
        }
        out.push('\n');
    }

    out.trim_end().to_owned()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_markdown() -> &'static str {
        "\
# Refactor Auth Module

## Phase 1
- [x] Audit current auth flow
- [ ] Extract token validation (files: auth.rs, token.rs) (complexity: medium)
- [~] Write migration script

## Phase 2
- [ ] Deploy new auth service
- [-] Remove legacy endpoints
"
    }

    #[test]
    fn parse_plan_title() {
        let plan = parse_plan(sample_markdown());
        assert_eq!(plan.title, "Refactor Auth Module");
    }

    #[test]
    fn parse_plan_sections() {
        let plan = parse_plan(sample_markdown());
        assert_eq!(plan.sections.len(), 2);
        assert_eq!(plan.sections[0].title, "Phase 1");
        assert_eq!(plan.sections[1].title, "Phase 2");
    }

    #[test]
    fn parse_plan_steps() {
        let plan = parse_plan(sample_markdown());
        assert_eq!(plan.sections[0].steps.len(), 3);
        assert_eq!(plan.sections[1].steps.len(), 2);
    }

    #[test]
    fn parse_plan_step_statuses() {
        let plan = parse_plan(sample_markdown());
        let s0 = &plan.sections[0].steps;
        assert_eq!(s0[0].status, PlanStepStatus::Completed);
        assert_eq!(s0[1].status, PlanStepStatus::Pending);
        assert_eq!(s0[2].status, PlanStepStatus::InProgress);

        let s1 = &plan.sections[1].steps;
        assert_eq!(s1[0].status, PlanStepStatus::Pending);
        assert_eq!(s1[1].status, PlanStepStatus::Skipped);
    }

    #[test]
    fn parse_plan_step_files_and_complexity() {
        let plan = parse_plan(sample_markdown());
        let step = &plan.sections[0].steps[1];
        assert_eq!(step.files_affected, vec!["auth.rs", "token.rs"]);
        assert_eq!(step.estimated_complexity.as_deref(), Some("medium"));
    }

    #[test]
    fn parse_plan_step_description_cleaned() {
        let plan = parse_plan(sample_markdown());
        let step = &plan.sections[0].steps[1];
        assert_eq!(step.description, "Extract token validation");
    }

    #[test]
    fn total_and_completed() {
        let plan = parse_plan(sample_markdown());
        assert_eq!(plan.total_steps(), 5);
        assert_eq!(plan.completed_steps(), 1);
    }

    #[test]
    fn completion_pct() {
        let plan = parse_plan(sample_markdown());
        assert_eq!(plan.completion_pct(), 20); // 1/5
    }

    #[test]
    fn completion_pct_empty_plan() {
        let plan = PlanFile {
            title: String::new(),
            sections: vec![],
        };
        assert_eq!(plan.completion_pct(), 100);
    }

    #[test]
    fn is_complete_false() {
        let plan = parse_plan(sample_markdown());
        assert!(!plan.is_complete());
    }

    #[test]
    fn is_complete_true() {
        let md = "\
# Done Plan
## Section
- [x] Step 1
- [-] Step 2
";
        let plan = parse_plan(md);
        assert!(plan.is_complete());
    }

    #[test]
    fn section_is_done() {
        let plan = parse_plan(sample_markdown());
        assert!(!plan.sections[0].is_done());
        // Phase 2: pending + skipped -> not done
        assert!(!plan.sections[1].is_done());
    }

    #[test]
    fn complete_step_marks_done() {
        let mut plan = parse_plan(sample_markdown());
        assert!(plan.complete_step(0, 1));
        assert_eq!(plan.sections[0].steps[1].status, PlanStepStatus::Completed);
        assert_eq!(plan.completed_steps(), 2);
    }

    #[test]
    fn complete_step_out_of_range() {
        let mut plan = parse_plan(sample_markdown());
        assert!(!plan.complete_step(99, 0));
        assert!(!plan.complete_step(0, 99));
    }

    #[test]
    fn render_plan_roundtrip() {
        let plan = parse_plan(sample_markdown());
        let rendered = render_plan(&plan);
        assert!(rendered.contains("# Refactor Auth Module"));
        assert!(rendered.contains("## Phase 1"));
        assert!(rendered.contains("[x] Audit current auth flow"));
        assert!(rendered.contains("[ ] Extract token validation"));
        assert!(rendered.contains("(files: auth.rs, token.rs)"));
        assert!(rendered.contains("(complexity: medium)"));
        assert!(rendered.contains("[~] Write migration script"));
        assert!(rendered.contains("[-] Remove legacy endpoints"));
        assert!(rendered.contains("Progress: 1/5 steps (20%)"));
    }

    #[test]
    fn render_empty_plan() {
        let plan = PlanFile {
            title: String::new(),
            sections: vec![],
        };
        assert!(render_plan(&plan).is_empty());
    }

    #[test]
    fn step_status_display() {
        assert_eq!(PlanStepStatus::Pending.to_string(), "pending");
        assert_eq!(PlanStepStatus::InProgress.to_string(), "in_progress");
        assert_eq!(PlanStepStatus::Completed.to_string(), "completed");
        assert_eq!(PlanStepStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn step_status_marker_roundtrip() {
        for status in [
            PlanStepStatus::Pending,
            PlanStepStatus::InProgress,
            PlanStepStatus::Completed,
            PlanStepStatus::Skipped,
        ] {
            assert_eq!(PlanStepStatus::from_marker(status.marker()), status);
        }
    }

    #[test]
    fn from_marker_case_insensitive_x() {
        assert_eq!(
            PlanStepStatus::from_marker("[X]"),
            PlanStepStatus::Completed
        );
    }

    #[test]
    fn parse_no_sections_creates_default() {
        let md = "# My Plan\n- [ ] step one\n- [x] step two\n";
        let plan = parse_plan(md);
        assert_eq!(plan.sections.len(), 1);
        assert_eq!(plan.sections[0].title, "Default");
        assert_eq!(plan.sections[0].steps.len(), 2);
    }

    #[test]
    fn parse_empty_checkbox_ignored() {
        let md = "# Plan\n## S\n- [ ] \n- [ ] valid step\n";
        let plan = parse_plan(md);
        // Empty description step is skipped (rest after marker is empty)
        assert_eq!(plan.sections[0].steps.len(), 1);
    }

    #[test]
    fn parse_line_without_checkbox() {
        let md = "# Plan\n## S\n- plain item without checkbox\n";
        let plan = parse_plan(md);
        // "plain item..." doesn't start with `[`, so it's parsed as pending
        assert_eq!(plan.sections[0].steps.len(), 1);
        assert_eq!(plan.sections[0].steps[0].status, PlanStepStatus::Pending);
        assert_eq!(
            plan.sections[0].steps[0].description,
            "plain item without checkbox"
        );
    }
}
