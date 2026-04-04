/// Parsed content from a CRAB.md project instruction file.
#[derive(Debug, Clone)]
pub struct CrabMd {
    pub content: String,
    pub source: CrabMdSource,
}

/// Where a CRAB.md file was loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrabMdSource {
    Global,
    User,
    Project,
}

/// Collect all CRAB.md files by priority (global -> user -> project).
pub fn collect_crab_md(_project_dir: &std::path::Path) -> Vec<CrabMd> {
    // TODO: collect global → user → project CRAB.md files
    Vec::new()
}
