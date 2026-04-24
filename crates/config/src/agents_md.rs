use std::path::Path;

/// Parsed content from a AGENTS.md project instruction file.
#[derive(Debug, Clone)]
pub struct AgentsMd {
    pub content: String,
    pub source: AgentsMdSource,
}

/// Where a AGENTS.md file was loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsMdSource {
    Global,
    User,
    Project,
}

/// Collect all AGENTS.md files by priority (global -> user -> project).
///
/// Returns them in order: global first, then user, then project, so the
/// system prompt builder can append them in that order (project instructions
/// have the highest effective priority since they come last).
///
/// In addition to the top-level `AGENTS.md` at each level, any `*.md` files
/// inside the corresponding `.crab/rules/` directory are loaded and appended
/// (sorted alphabetically by filename) after the AGENTS.md content.
pub fn collect_agents_md(project_dir: &Path) -> Vec<AgentsMd> {
    let mut results = Vec::new();

    // 1. Global: ~/.crab/AGENTS.md + ~/.crab/rules/*.md
    let global_dir = crate::settings::global_config_dir();
    if let Some(md) = read_agents_md(&global_dir.join("AGENTS.md"), AgentsMdSource::Global) {
        results.push(md);
    }
    results.extend(collect_rules_dir(
        &global_dir.join("rules"),
        &AgentsMdSource::Global,
    ));

    // 2. User: ~/.crab/AGENTS.md is the same as global for now
    //    (Claude Code has a separate user dir, but we merge global+user)

    // 3. Project: <project_dir>/AGENTS.md
    if let Some(md) = read_agents_md(&project_dir.join("AGENTS.md"), AgentsMdSource::Project) {
        results.push(md);
    }

    // 4. Also check <project_dir>/.crab/AGENTS.md (nested project config)
    let nested = project_dir.join(".crab").join("AGENTS.md");
    if nested.exists()
        && let Some(md) = read_agents_md(&nested, AgentsMdSource::Project)
    {
        // Avoid duplicate if same as #3
        if results.last().is_none_or(|last| last.content != md.content) {
            results.push(md);
        }
    }

    // 5. Project rules: <project_dir>/.crab/rules/*.md
    results.extend(collect_rules_dir(
        &project_dir.join(".crab").join("rules"),
        &AgentsMdSource::Project,
    ));

    results
}

/// Load all `*.md` files from a rules directory, sorted alphabetically by
/// filename. Missing or unreadable directories return an empty vec.
fn collect_rules_dir(rules_dir: &Path, source: &AgentsMdSource) -> Vec<AgentsMd> {
    let Ok(entries) = std::fs::read_dir(rules_dir) else {
        return Vec::new();
    };

    let mut md_files: Vec<_> = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    md_files.sort_by(|a, b| {
        a.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .cmp(b.file_name().and_then(|n| n.to_str()).unwrap_or(""))
    });

    md_files
        .into_iter()
        .filter_map(|path| read_agents_md(&path, source.clone()))
        .collect()
}

/// Read a single AGENTS.md file, returning `None` if it doesn't exist or is empty.
fn read_agents_md(path: &Path, source: AgentsMdSource) -> Option<AgentsMd> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(AgentsMd {
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
        let results = collect_agents_md(dir.path());
        // No AGENTS.md files — may have global if ~/.crab/AGENTS.md exists,
        // but no project-level ones
        for md in &results {
            assert_ne!(md.source, AgentsMdSource::Project);
        }
    }

    #[test]
    fn collect_project_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Project Rules\nBe helpful.").unwrap();
        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Be helpful"));
    }

    #[test]
    fn collect_nested_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let nested_dir = dir.path().join(".crab");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(nested_dir.join("AGENTS.md"), "Nested instructions").unwrap();
        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Nested"));
    }

    #[test]
    fn empty_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "   ").unwrap();
        let results = collect_agents_md(dir.path());
        assert!(!results.iter().any(|md| md.source == AgentsMdSource::Project));
    }

    #[test]
    fn read_nonexistent_returns_none() {
        assert!(read_agents_md(Path::new("/no/such/file"), AgentsMdSource::Global).is_none());
    }

    #[test]
    fn collect_both_root_and_nested_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "Root instructions").unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("AGENTS.md"), "Nested instructions").unwrap();

        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
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
        fs::write(dir.path().join("AGENTS.md"), same_content).unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("AGENTS.md"), same_content).unwrap();

        let results = collect_agents_md(dir.path());
        let project_count = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .count();
        // Should deduplicate identical content
        assert_eq!(project_count, 1);
    }

    #[test]
    fn agents_md_source_equality() {
        assert_eq!(AgentsMdSource::Global, AgentsMdSource::Global);
        assert_ne!(AgentsMdSource::Global, AgentsMdSource::Project);
        assert_ne!(AgentsMdSource::User, AgentsMdSource::Project);
    }

    #[test]
    fn read_agents_md_trims_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "  \n  content here  \n  ").unwrap();
        let md = read_agents_md(&dir.path().join("AGENTS.md"), AgentsMdSource::Project).unwrap();
        assert_eq!(md.content, "content here");
    }

    #[test]
    fn nested_empty_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("AGENTS.md"), "   \n  \t  ").unwrap();
        let results = collect_agents_md(dir.path());
        assert!(!results.iter().any(|md| md.source == AgentsMdSource::Project));
    }

    #[test]
    fn collect_project_rules_dir_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("20-style.md"), "Style rule").unwrap();
        fs::write(rules.join("10-security.md"), "Security rule").unwrap();
        fs::write(rules.join("30-testing.md"), "Testing rule").unwrap();

        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 3);
        assert!(project_mds[0].content.contains("Security rule"));
        assert!(project_mds[1].content.contains("Style rule"));
        assert!(project_mds[2].content.contains("Testing rule"));
    }

    #[test]
    fn rules_dir_appended_after_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "Top-level CRAB").unwrap();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("a.md"), "A rule").unwrap();

        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 2);
        assert!(project_mds[0].content.contains("Top-level CRAB"));
        assert!(project_mds[1].content.contains("A rule"));
    }

    #[test]
    fn rules_dir_non_md_files_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("keep.md"), "Kept").unwrap();
        fs::write(rules.join("skip.txt"), "Skipped text").unwrap();
        fs::write(rules.join("README"), "Skipped no-ext").unwrap();
        fs::write(rules.join("notes.MD"), "Uppercase ext").unwrap();

        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 2);
        let contents: Vec<&str> = project_mds.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.iter().any(|c| c.contains("Kept")));
        assert!(contents.iter().any(|c| c.contains("Uppercase ext")));
    }

    #[test]
    fn rules_dir_empty_files_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("empty.md"), "   \n\t ").unwrap();
        fs::write(rules.join("real.md"), "Real content").unwrap();

        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Real content"));
    }

    #[test]
    fn rules_dir_missing_ok() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "Just AGENTS.md").unwrap();
        // No .crab/rules/ directory at all
        let results = collect_agents_md(dir.path());
        let project_count = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .count();
        assert_eq!(project_count, 1);
    }

    #[test]
    fn rules_dir_subdirectories_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join(".crab").join("rules");
        let nested = rules.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("inner.md"), "Should not load").unwrap();
        fs::write(rules.join("top.md"), "Top rule").unwrap();

        let results = collect_agents_md(dir.path());
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Top rule"));
    }

    #[test]
    fn agents_md_clone() {
        let md = AgentsMd {
            content: "test".into(),
            source: AgentsMdSource::Global,
        };
        #[allow(clippy::redundant_clone)]
        let cloned = md.clone();
        assert_eq!(cloned.content, "test");
        assert_eq!(cloned.source, AgentsMdSource::Global);
    }
}
