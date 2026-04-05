use std::path::Path;

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
///
/// Returns them in order: global first, then user, then project, so the
/// system prompt builder can append them in that order (project instructions
/// have the highest effective priority since they come last).
pub fn collect_crab_md(project_dir: &Path) -> Vec<CrabMd> {
    let mut results = Vec::new();

    // 1. Global: ~/.crab/CRAB.md
    let global_dir = crate::settings::global_config_dir();
    if let Some(md) = read_crab_md(&global_dir.join("CRAB.md"), CrabMdSource::Global) {
        results.push(md);
    }

    // 2. User: ~/.crab/CRAB.md is the same as global for now
    //    (Claude Code has a separate user dir, but we merge global+user)

    // 3. Project: <project_dir>/CRAB.md
    if let Some(md) = read_crab_md(&project_dir.join("CRAB.md"), CrabMdSource::Project) {
        results.push(md);
    }

    // 4. Also check <project_dir>/.crab/CRAB.md (nested project config)
    let nested = project_dir.join(".crab").join("CRAB.md");
    if nested.exists()
        && let Some(md) = read_crab_md(&nested, CrabMdSource::Project)
    {
        // Avoid duplicate if same as #3
        if results.last().is_none_or(|last| last.content != md.content) {
            results.push(md);
        }
    }

    results
}

/// Read a single CRAB.md file, returning `None` if it doesn't exist or is empty.
fn read_crab_md(path: &Path, source: CrabMdSource) -> Option<CrabMd> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(CrabMd {
        content: trimmed.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn collect_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let results = collect_crab_md(dir.path());
        // No CRAB.md files — may have global if ~/.crab/CRAB.md exists,
        // but no project-level ones
        for md in &results {
            assert_ne!(md.source, CrabMdSource::Project);
        }
    }

    #[test]
    fn collect_project_crab_md() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CRAB.md"), "# Project Rules\nBe helpful.").unwrap();
        let results = collect_crab_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == CrabMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Be helpful"));
    }

    #[test]
    fn collect_nested_crab_md() {
        let dir = tempfile::tempdir().unwrap();
        let nested_dir = dir.path().join(".crab");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(nested_dir.join("CRAB.md"), "Nested instructions").unwrap();
        let results = collect_crab_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == CrabMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Nested"));
    }

    #[test]
    fn empty_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CRAB.md"), "   ").unwrap();
        let results = collect_crab_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == CrabMdSource::Project)
            .collect();
        assert!(project_mds.is_empty());
    }

    #[test]
    fn read_nonexistent_returns_none() {
        assert!(read_crab_md(Path::new("/no/such/file"), CrabMdSource::Global).is_none());
    }

    #[test]
    fn collect_both_root_and_nested_crab_md() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CRAB.md"), "Root instructions").unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("CRAB.md"), "Nested instructions").unwrap();

        let results = collect_crab_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == CrabMdSource::Project)
            .collect();
        // Both root and nested should be collected (different content)
        assert_eq!(project_mds.len(), 2);
        assert!(project_mds[0].content.contains("Root"));
        assert!(project_mds[1].content.contains("Nested"));
    }

    #[test]
    fn collect_deduplicates_identical_root_and_nested() {
        let dir = tempfile::tempdir().unwrap();
        let same_content = "Identical instructions";
        fs::write(dir.path().join("CRAB.md"), same_content).unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("CRAB.md"), same_content).unwrap();

        let results = collect_crab_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == CrabMdSource::Project)
            .collect();
        // Should deduplicate identical content
        assert_eq!(project_mds.len(), 1);
    }

    #[test]
    fn crab_md_source_equality() {
        assert_eq!(CrabMdSource::Global, CrabMdSource::Global);
        assert_ne!(CrabMdSource::Global, CrabMdSource::Project);
        assert_ne!(CrabMdSource::User, CrabMdSource::Project);
    }

    #[test]
    fn read_crab_md_trims_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("CRAB.md"), "  \n  content here  \n  ").unwrap();
        let md = read_crab_md(&dir.path().join("CRAB.md"), CrabMdSource::Project).unwrap();
        assert_eq!(md.content, "content here");
    }

    #[test]
    fn nested_empty_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("CRAB.md"), "   \n  \t  ").unwrap();
        let results = collect_crab_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == CrabMdSource::Project)
            .collect();
        assert!(project_mds.is_empty());
    }

    #[test]
    fn crab_md_clone() {
        let md = CrabMd {
            content: "test".into(),
            source: CrabMdSource::Global,
        };
        let cloned = md.clone();
        assert_eq!(cloned.content, "test");
        assert_eq!(cloned.source, CrabMdSource::Global);
    }
}
