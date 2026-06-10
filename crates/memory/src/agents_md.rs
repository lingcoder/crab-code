//! AGENTS.md project instruction file discovery and parsing.
//!
//! Collects `AGENTS.md` and `.crab/rules/*.md` files from global, user,
//! and project levels. The system prompt builder calls [`collect_agents_md`]
//! to gather all instruction layers.
//!
//! # Nested directory loading
//!
//! When a tool accesses a file in a subdirectory, the engine calls
//! [`collect_nested_agents_md`] to discover AGENTS.md files along the
//! path from CWD to the target file. Rules with `paths:` frontmatter
//! are only injected when the accessed file matches the glob pattern.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSetBuilder};

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

impl AgentsMdSource {
    /// Human-readable label for prompt injection.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// A parsed rule file — either unconditional (applies everywhere) or
/// conditional (applies only when the accessed file matches `globs`).
#[derive(Debug, Clone)]
pub struct ParsedRule {
    pub content: String,
    pub source: AgentsMdSource,
    /// If `Some`, this rule only applies when the target file matches
    /// one of these gitignore-style globs relative to the rule's directory.
    pub globs: Option<Vec<String>>,
}

// ── Original API (unchanged) ──────────────────────────────────────────

/// Collect all AGENTS.md files by priority (global -> user -> project).
///
/// Returns them in order: global first, then user, then project, so the
/// system prompt builder can append them in that order (project instructions
/// have the highest effective priority since they come last).
///
/// In addition to the top-level `AGENTS.md` at each level, any `*.md` files
/// inside the corresponding `.crab/rules/` directory are loaded and appended
/// (sorted alphabetically by filename) after the AGENTS.md content.
///
/// `global_config_dir` is typically `~/.crab/` — passed explicitly so this
/// module has no dependency on the config crate.
pub fn collect_agents_md(project_dir: &Path, global_config_dir: &Path) -> Vec<AgentsMd> {
    let mut results = Vec::new();

    // 1. Global: ~/.crab/AGENTS.md + ~/.crab/rules/*.md
    if let Some(md) = read_agents_md(&global_config_dir.join("AGENTS.md"), AgentsMdSource::Global) {
        results.push(md);
    }
    results.extend(collect_rules_dir(
        &global_config_dir.join("rules"),
        &AgentsMdSource::Global,
    ));

    // 2. User: ~/.crab/AGENTS.md is the same as global for now
    //    (Claude Code has a separate user dir, but we merge global+user)

    // 3. Project: <project_dir>/AGENTS.md
    if let Some(md) = read_agents_md(&project_dir.join("AGENTS.md"), AgentsMdSource::Project) {
        results.push(md);
    }

    // 3b. Project-local: <project_dir>/AGENTS.local.md (gitignored, per-checkout
    //     private memory). Callers are responsible for gitignore maintenance.
    let local_md = project_dir.join("AGENTS.local.md");
    if local_md.exists()
        && let Some(md) = read_agents_md(&local_md, AgentsMdSource::Project)
    {
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

// ── New API: upward walk from CWD ─────────────────────────────────────

/// Collect AGENTS.md files by walking upward from `cwd` to the filesystem
/// root. Each directory level checks `AGENTS.md`, `.crab/AGENTS.md`, and
/// `.crab/rules/*.md` (unconditional only).
///
/// Global files (`~/.crab/`) are loaded first. Directory walk order is
/// root → CWD, so closer-to-CWD files have higher effective priority
/// (loaded later = higher priority in the system prompt).
///
/// This is the startup-time discovery function. For runtime nested loading
/// when files in subdirectories are accessed, use [`collect_nested_agents_md`].
pub fn collect_agents_md_from_cwd(cwd: &Path, global_config_dir: &Path) -> Vec<AgentsMd> {
    let mut results = Vec::new();

    // 1. Global: ~/.crab/AGENTS.md + ~/.crab/rules/*.md
    if let Some(md) = read_agents_md(&global_config_dir.join("AGENTS.md"), AgentsMdSource::Global) {
        results.push(md);
    }
    results.extend(collect_rules_dir(
        &global_config_dir.join("rules"),
        &AgentsMdSource::Global,
    ));

    // 2. Walk from filesystem root down to CWD
    let dirs = ancestor_dirs(cwd);
    for dir in &dirs {
        collect_dir_agents_md(dir, &mut results);
    }

    results
}

// ── New API: nested directory lazy loading ─────────────────────────────

/// Collect AGENTS.md content for files accessed in subdirectories.
///
/// Given the CWD and a list of target file paths (from tool execution),
/// discovers AGENTS.md files along the paths and returns those not yet
/// loaded (tracked by `loaded_paths`).
///
/// For directories between CWD and the target: loads AGENTS.md +
/// unconditional rules + conditional rules matching the target.
/// For directories from root to CWD (above CWD): loads only conditional
/// rules matching the target.
///
/// `loaded_paths` is mutated to track which files have been returned,
/// preventing re-injection across calls within the same session.
#[allow(clippy::implicit_hasher)]
pub fn collect_nested_agents_md(
    cwd: &Path,
    target_files: &[PathBuf],
    loaded_paths: &mut HashSet<PathBuf>,
    _global_config_dir: &Path,
) -> Vec<AgentsMd> {
    if target_files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    let cwd_ancestors: HashSet<PathBuf> = ancestor_dirs(cwd).into_iter().collect();

    for target in target_files {
        let target_dir = match target.parent() {
            Some(d) => d.to_path_buf(),
            None => continue,
        };

        // Phase 1: Managed/User conditional rules matching target
        // (We don't have separate managed/user dirs yet, skip for now)

        // Phase 2: Nested directories (CWD → target)
        // Load AGENTS.md + unconditional rules + conditional rules
        let nested_dirs = nested_dirs_between(cwd, &target_dir);
        for dir in &nested_dirs {
            // AGENTS.md / .crab/AGENTS.md
            for md in read_dir_agents_md(dir) {
                let path = dir.join("AGENTS.md");
                if loaded_paths.insert(path) {
                    results.push(md.clone());
                }
            }

            // Unconditional rules
            let rules_dir = dir.join(".crab").join("rules");
            for rule in collect_unconditional_rules(&rules_dir, &AgentsMdSource::Project) {
                // We don't have the exact path for dedup here, skip if already loaded
                if !results.iter().any(|r| r.content == rule.content) {
                    results.push(rule);
                }
            }

            // Conditional rules matching target
            for rule in collect_conditioned_rules(&rules_dir, &AgentsMdSource::Project, dir, target)
            {
                if !results.iter().any(|r| r.content == rule.content) {
                    results.push(rule);
                }
            }
        }

        // Phase 3: CWD-level directories (root → CWD)
        // Only conditional rules (unconditional already loaded at startup)
        for dir in ancestor_dirs(cwd) {
            if cwd_ancestors.contains(&dir) && dir != *cwd {
                let rules_dir = dir.join(".crab").join("rules");
                for rule in
                    collect_conditioned_rules(&rules_dir, &AgentsMdSource::Project, &dir, target)
                {
                    if !results.iter().any(|r| r.content == rule.content) {
                        results.push(rule);
                    }
                }
            }
        }
    }

    results
}

// ── Frontmatter parsing ───────────────────────────────────────────────

/// Parse a rule file's frontmatter to extract `paths:` globs.
///
/// Returns `(body_content, globs)`. If no `paths:` field is present,
/// `globs` is `None` (unconditional rule). If `paths:` is present,
/// `globs` contains the parsed patterns (conditional rule).
///
/// The `paths:` field accepts:
/// - A YAML list: `paths:\n  - "src/**/*.rs"\n  - "tests/**/*.rs"`
/// - A comma-separated string: `paths: "src/**/*.rs, tests/**/*.rs"`
/// - A single string: `paths: "src/**/*.rs"`
pub fn parse_rule_frontmatter(content: &str) -> (String, Option<Vec<String>>) {
    let Some((frontmatter, body)) = split_frontmatter(content) else {
        return (content.to_string(), None);
    };

    // Parse the YAML frontmatter
    let parsed: serde_yaml_ng::Value = match serde_yaml_ng::from_str(frontmatter) {
        Ok(v) => v,
        Err(_) => return (content.to_string(), None),
    };

    let Some(paths_value) = parsed.get("paths") else {
        return (body.to_string(), None);
    };

    let patterns = match paths_value {
        // YAML list: paths:\n  - "src/**/*.rs"
        serde_yaml_ng::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>(),
        // Single string or comma-separated: paths: "src/**/*.rs, tests/**/*.rs"
        serde_yaml_ng::Value::String(s) => s.split(',').map(|p| p.trim().to_string()).collect(),
        _ => return (body.to_string(), None),
    };

    // Filter empty strings, strip trailing /**, and check for match-all
    let patterns: Vec<String> = patterns
        .into_iter()
        .map(|p| {
            if p.ends_with("/**") {
                p[..p.len() - 3].to_string()
            } else {
                p
            }
        })
        .filter(|p| !p.is_empty())
        .collect();

    // If all patterns are "**" (match-all), treat as unconditional
    if patterns.is_empty() || patterns.iter().all(|p| p == "**") {
        return (body.to_string(), None);
    }

    (body.to_string(), Some(patterns))
}

/// Check if a target file path matches any of the given globs.
///
/// `base_dir` is the directory containing `.crab/` (the rule's parent dir).
/// `target` is the absolute path of the accessed file.
///
/// Uses gitignore-style matching via the `globset` crate.
pub fn glob_matches(globs: &[String], base_dir: &Path, target: &Path) -> bool {
    let Ok(relative) = target.strip_prefix(base_dir) else {
        return false;
    };
    let Some(relative_str) = relative.to_str() else {
        return false;
    };
    // Normalize Windows paths to forward slashes for glob matching
    let normalized = relative_str.replace('\\', "/");

    let mut builder = GlobSetBuilder::new();
    for pattern in globs {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        }
    }
    let Ok(set) = builder.build() else {
        return false;
    };
    set.is_match(&normalized)
}

// ── Rule collection helpers ───────────────────────────────────────────

/// Collect only unconditional rules (no `paths:` frontmatter) from a
/// rules directory.
fn collect_unconditional_rules(rules_dir: &Path, source: &AgentsMdSource) -> Vec<AgentsMd> {
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
        .filter_map(|path| {
            let content = std::fs::read_to_string(&path).ok()?;
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (body, globs) = parse_rule_frontmatter(trimmed);
            if globs.is_some() {
                return None; // Skip conditional rules
            }
            Some(AgentsMd {
                content: body,
                source: source.clone(),
            })
        })
        .collect()
}

/// Collect only conditional rules (with `paths:` frontmatter) from a
/// rules directory, filtering to those whose globs match the target file.
fn collect_conditioned_rules(
    rules_dir: &Path,
    source: &AgentsMdSource,
    base_dir: &Path,
    target: &Path,
) -> Vec<AgentsMd> {
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
        .filter_map(|path| {
            let content = std::fs::read_to_string(&path).ok()?;
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (body, globs) = parse_rule_frontmatter(trimmed);
            let globs = globs?; // Skip unconditional rules
            if !glob_matches(&globs, base_dir, target) {
                return None;
            }
            Some(AgentsMd {
                content: body,
                source: source.clone(),
            })
        })
        .collect()
}

// ── Internal helpers ──────────────────────────────────────────────────

/// Compute ancestor directories from `path` up to (and including) the
/// filesystem root. Returns root → path order (path is last).
fn ancestor_dirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = Some(path.to_path_buf());
    while let Some(dir) = current {
        dirs.push(dir.clone());
        let parent = dir.parent();
        if parent == Some(&dir) {
            break; // Reached filesystem root
        }
        current = parent.map(Path::to_path_buf);
    }
    dirs.reverse(); // root first, path last
    dirs
}

/// Compute directories between `cwd` and `target_dir` (inclusive).
/// Returns CWD → target order. If target is under CWD, returns all
/// intermediate directories. If target is not under CWD, returns empty.
fn nested_dirs_between(cwd: &Path, target_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = target_dir.to_path_buf();

    // Walk up from target to CWD
    loop {
        dirs.push(current.clone());
        if current == cwd {
            break;
        }
        let parent = match current.parent() {
            Some(p) => p.to_path_buf(),
            None => return Vec::new(), // target not under cwd
        };
        if parent == current {
            return Vec::new(); // Reached root without finding cwd
        }
        current = parent;
    }

    dirs.reverse(); // CWD first, target last
    dirs
}

/// Read AGENTS.md and .crab/AGENTS.md from a single directory.
/// Returns up to 2 entries (deduplicated by content).
fn read_dir_agents_md(dir: &Path) -> Vec<AgentsMd> {
    let mut results = Vec::new();

    if let Some(md) = read_agents_md(&dir.join("AGENTS.md"), AgentsMdSource::Project) {
        results.push(md);
    }

    let nested = dir.join(".crab").join("AGENTS.md");
    if let Some(md) = read_agents_md(&nested, AgentsMdSource::Project)
        && results.last().is_none_or(|last| last.content != md.content)
    {
        results.push(md);
    }

    results
}

/// Collect AGENTS.md + .crab/AGENTS.md + .crab/rules/*.md from a single
/// directory (unconditional rules only). Used by the upward walk.
fn collect_dir_agents_md(dir: &Path, results: &mut Vec<AgentsMd>) {
    // AGENTS.md
    if let Some(md) = read_agents_md(&dir.join("AGENTS.md"), AgentsMdSource::Project)
        && results.last().is_none_or(|last| last.content != md.content)
    {
        results.push(md);
    }

    // .crab/AGENTS.md
    let nested = dir.join(".crab").join("AGENTS.md");
    if let Some(md) = read_agents_md(&nested, AgentsMdSource::Project)
        && results.last().is_none_or(|last| last.content != md.content)
    {
        results.push(md);
    }

    // .crab/rules/*.md (unconditional only)
    results.extend(collect_unconditional_rules(
        &dir.join(".crab").join("rules"),
        &AgentsMdSource::Project,
    ));
}

/// Split content on `---` frontmatter delimiters.
/// Returns `Some((frontmatter, body))` if valid frontmatter is found.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[4..];
    let end = after_first.find("---")?;
    let frontmatter = after_first[..end].trim();
    let body = after_first[end + 3..].trim();
    if frontmatter.is_empty() {
        return None;
    }
    Some((frontmatter, body))
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
    use std::path::PathBuf;

    fn fake_global_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    // ── Original tests ────────────────────────────────────────────────

    #[test]
    fn collect_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let results = collect_agents_md(dir.path(), &global);
        for md in &results {
            assert_ne!(md.source, AgentsMdSource::Project);
        }
    }

    #[test]
    fn collect_project_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        fs::write(dir.path().join("AGENTS.md"), "# Project Rules\nBe helpful.").unwrap();
        let results = collect_agents_md(dir.path(), &global);
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 1);
        assert!(project_mds[0].content.contains("Be helpful"));
    }

    #[test]
    fn collect_nested_agents_md_basic() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let nested_dir = dir.path().join(".crab");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(nested_dir.join("AGENTS.md"), "Nested instructions").unwrap();
        let results = collect_agents_md(dir.path(), &global);
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
        let (_gd, global) = fake_global_dir();
        fs::write(dir.path().join("AGENTS.md"), "   ").unwrap();
        let results = collect_agents_md(dir.path(), &global);
        assert!(
            !results
                .iter()
                .any(|md| md.source == AgentsMdSource::Project)
        );
    }

    #[test]
    fn read_nonexistent_returns_none() {
        assert!(read_agents_md(Path::new("/no/such/file"), AgentsMdSource::Global).is_none());
    }

    #[test]
    fn collect_both_root_and_nested_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        fs::write(dir.path().join("AGENTS.md"), "Root instructions").unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("AGENTS.md"), "Nested instructions").unwrap();

        let results = collect_agents_md(dir.path(), &global);
        let project_mds: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project_mds.len(), 2);
        assert!(project_mds[0].content.contains("Root"));
        assert!(project_mds[1].content.contains("Nested"));
    }

    #[test]
    fn collect_deduplicates_identical_root_and_nested() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let same_content = "Identical instructions";
        fs::write(dir.path().join("AGENTS.md"), same_content).unwrap();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("AGENTS.md"), same_content).unwrap();

        let results = collect_agents_md(dir.path(), &global);
        let project_count = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .count();
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
        let (_gd, global) = fake_global_dir();
        let nested = dir.path().join(".crab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("AGENTS.md"), "   \n  \t  ").unwrap();
        let results = collect_agents_md(dir.path(), &global);
        assert!(
            !results
                .iter()
                .any(|md| md.source == AgentsMdSource::Project)
        );
    }

    #[test]
    fn collect_project_rules_dir_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("20-style.md"), "Style rule").unwrap();
        fs::write(rules.join("10-security.md"), "Security rule").unwrap();
        fs::write(rules.join("30-testing.md"), "Testing rule").unwrap();

        let results = collect_agents_md(dir.path(), &global);
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
        let (_gd, global) = fake_global_dir();
        fs::write(dir.path().join("AGENTS.md"), "Top-level CRAB").unwrap();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("a.md"), "A rule").unwrap();

        let results = collect_agents_md(dir.path(), &global);
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
        let (_gd, global) = fake_global_dir();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("keep.md"), "Kept").unwrap();
        fs::write(rules.join("skip.txt"), "Skipped text").unwrap();
        fs::write(rules.join("README"), "Skipped no-ext").unwrap();
        fs::write(rules.join("notes.MD"), "Uppercase ext").unwrap();

        let results = collect_agents_md(dir.path(), &global);
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
        let (_gd, global) = fake_global_dir();
        let rules = dir.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("empty.md"), "   \n\t ").unwrap();
        fs::write(rules.join("real.md"), "Real content").unwrap();

        let results = collect_agents_md(dir.path(), &global);
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
        let (_gd, global) = fake_global_dir();
        fs::write(dir.path().join("AGENTS.md"), "Just AGENTS.md").unwrap();
        let results = collect_agents_md(dir.path(), &global);
        let project_count = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .count();
        assert_eq!(project_count, 1);
    }

    #[test]
    fn rules_dir_subdirectories_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let rules = dir.path().join(".crab").join("rules");
        let nested = rules.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("inner.md"), "Should not load").unwrap();
        fs::write(rules.join("top.md"), "Top rule").unwrap();

        let results = collect_agents_md(dir.path(), &global);
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

    // ── New tests: upward walk ────────────────────────────────────────

    #[test]
    fn collect_agents_md_from_cwd_walks_upward() {
        let root = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let a = root.path().join("a");
        let b = a.join("b");
        fs::create_dir_all(&b).unwrap();

        fs::write(root.path().join("AGENTS.md"), "Root rules").unwrap();
        fs::write(a.join("AGENTS.md"), "A rules").unwrap();
        fs::write(b.join("AGENTS.md"), "B rules").unwrap();

        let results = collect_agents_md_from_cwd(&b, &global);
        let project: Vec<_> = results
            .iter()
            .filter(|md| md.source == AgentsMdSource::Project)
            .collect();
        assert_eq!(project.len(), 3);
        // Order: root first, then a, then b (root→CWD)
        assert!(project[0].content.contains("Root"));
        assert!(project[1].content.contains("A rules"));
        assert!(project[2].content.contains("B rules"));
    }

    #[test]
    fn collect_agents_md_from_cwd_includes_crab_rules() {
        let root = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let rules = root.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();
        fs::write(rules.join("style.md"), "Style guide").unwrap();

        let results = collect_agents_md_from_cwd(root.path(), &global);
        assert!(results.iter().any(|r| r.content.contains("Style guide")));
    }

    // ── New tests: frontmatter parsing ────────────────────────────────

    #[test]
    fn parse_rule_frontmatter_no_frontmatter() {
        let (body, globs) = parse_rule_frontmatter("Just a rule\nNo frontmatter");
        assert_eq!(body, "Just a rule\nNo frontmatter");
        assert!(globs.is_none());
    }

    #[test]
    fn parse_rule_frontmatter_with_paths_list() {
        let content = "---\npaths:\n  - \"src/**/*.rs\"\n  - \"tests/**/*.rs\"\n---\nRule body";
        let (body, globs) = parse_rule_frontmatter(content);
        assert_eq!(body, "Rule body");
        let globs = globs.unwrap();
        assert_eq!(globs.len(), 2);
        assert_eq!(globs[0], "src/**/*.rs");
        assert_eq!(globs[1], "tests/**/*.rs");
    }

    #[test]
    fn parse_rule_frontmatter_with_paths_string() {
        let content = "---\npaths: \"src/**/*.rs, tests/**/*.rs\"\n---\nRule body";
        let (body, globs) = parse_rule_frontmatter(content);
        assert_eq!(body, "Rule body");
        let globs = globs.unwrap();
        assert_eq!(globs.len(), 2);
        assert_eq!(globs[0], "src/**/*.rs");
        assert_eq!(globs[1], "tests/**/*.rs");
    }

    #[test]
    fn parse_rule_frontmatter_strips_trailing_globstar() {
        let content = "---\npaths:\n  - \"src/**\"\n---\nBody";
        let (_, globs) = parse_rule_frontmatter(content);
        let globs = globs.unwrap();
        assert_eq!(globs[0], "src");
    }

    #[test]
    fn parse_rule_frontmatter_match_all_treated_as_unconditional() {
        let content = "---\npaths:\n  - \"**\"\n---\nBody";
        let (_, globs) = parse_rule_frontmatter(content);
        assert!(globs.is_none(), "** should be treated as unconditional");
    }

    #[test]
    fn parse_rule_frontmatter_no_paths_field() {
        let content = "---\ndescription: A rule\n---\nBody";
        let (body, globs) = parse_rule_frontmatter(content);
        assert_eq!(body, "Body");
        assert!(globs.is_none());
    }

    // ── New tests: glob matching ──────────────────────────────────────

    #[test]
    fn glob_matches_simple() {
        let base = Path::new("/project");
        let target = Path::new("/project/src/lib.rs");
        assert!(glob_matches(&["src/**/*.rs".into()], base, target));
    }

    #[test]
    fn glob_matches_no_match() {
        let base = Path::new("/project");
        let target = Path::new("/project/tests/lib.rs");
        assert!(!glob_matches(&["src/**/*.rs".into()], base, target));
    }

    #[test]
    fn glob_matches_multiple_patterns() {
        let base = Path::new("/project");
        let globs = vec!["src/**/*.rs".into(), "tests/**/*.rs".into()];
        assert!(glob_matches(&globs, base, Path::new("/project/src/lib.rs")));
        assert!(glob_matches(
            &globs,
            base,
            Path::new("/project/tests/lib.rs")
        ));
        assert!(!glob_matches(
            &globs,
            base,
            Path::new("/project/docs/readme.md")
        ));
    }

    #[test]
    fn glob_matches_outside_base() {
        let base = Path::new("/project");
        let target = Path::new("/other/src/lib.rs");
        assert!(!glob_matches(&["src/**/*.rs".into()], base, target));
    }

    // ── New tests: nested directory loading ───────────────────────────

    #[test]
    fn nested_dirs_between_basic() {
        let cwd = Path::new("/project");
        let target = Path::new("/project/a/b/c/file.rs");
        let dirs = nested_dirs_between(cwd, target.parent().unwrap());
        assert_eq!(dirs.len(), 4);
        assert_eq!(dirs[0], Path::new("/project"));
        assert_eq!(dirs[1], Path::new("/project/a"));
        assert_eq!(dirs[2], Path::new("/project/a/b"));
        assert_eq!(dirs[3], Path::new("/project/a/b/c"));
    }

    #[test]
    fn nested_dirs_between_same_dir() {
        let cwd = Path::new("/project");
        let target = Path::new("/project/file.rs");
        let dirs = nested_dirs_between(cwd, target.parent().unwrap());
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], Path::new("/project"));
    }

    #[test]
    fn nested_dirs_between_not_under_cwd() {
        let cwd = Path::new("/project");
        let target = Path::new("/other/file.rs");
        let dirs = nested_dirs_between(cwd, target.parent().unwrap());
        assert!(dirs.is_empty());
    }

    #[test]
    fn ancestor_dirs_basic() {
        let dirs = ancestor_dirs(Path::new("/a/b/c"));
        assert_eq!(dirs.last(), Some(&PathBuf::from("/a/b/c")));
        assert_eq!(dirs.first(), Some(&PathBuf::from("/")));
    }

    #[test]
    fn collect_nested_agents_md_finds_along_path() {
        let root = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let a = root.path().join("a");
        let b = a.join("b");
        fs::create_dir_all(&b).unwrap();

        fs::write(root.path().join("AGENTS.md"), "Root rules").unwrap();
        fs::write(a.join("AGENTS.md"), "A rules").unwrap();

        let target = b.join("file.rs");
        let mut loaded = HashSet::new();
        let results = collect_nested_agents_md(root.path(), &[target], &mut loaded, &global);

        // Should find Root and A AGENTS.md
        assert!(results.iter().any(|r| r.content.contains("Root")));
        assert!(results.iter().any(|r| r.content.contains("A rules")));
    }

    #[test]
    fn collect_nested_agents_md_deduplicates() {
        let root = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        fs::write(root.path().join("AGENTS.md"), "Same content").unwrap();

        let target = root.path().join("file.rs");
        let mut loaded = HashSet::new();

        // First call
        let results1 =
            collect_nested_agents_md(root.path(), &[target.clone()], &mut loaded, &global);
        assert_eq!(results1.len(), 1);

        // Second call should return nothing (already loaded)
        let results2 = collect_nested_agents_md(root.path(), &[target], &mut loaded, &global);
        assert!(results2.is_empty());
    }

    #[test]
    fn collect_nested_agents_md_conditional_rules() {
        let root = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let rules = root.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();

        // Conditional rule matching src/**/*.rs
        fs::write(
            rules.join("rust-style.md"),
            "---\npaths:\n  - \"src/**/*.rs\"\n---\nRust style guide",
        )
        .unwrap();

        // Unconditional rule
        fs::write(rules.join("general.md"), "General rules").unwrap();

        let target = root.path().join("src").join("lib.rs");
        let mut loaded = HashSet::new();
        let results = collect_nested_agents_md(root.path(), &[target], &mut loaded, &global);

        assert!(results.iter().any(|r| r.content.contains("Rust style")));
        assert!(results.iter().any(|r| r.content.contains("General")));
    }

    #[test]
    fn collect_nested_agents_md_conditional_rules_no_match() {
        let root = tempfile::tempdir().unwrap();
        let (_gd, global) = fake_global_dir();
        let rules = root.path().join(".crab").join("rules");
        fs::create_dir_all(&rules).unwrap();

        // Conditional rule matching only src/**/*.rs
        fs::write(
            rules.join("rust-style.md"),
            "---\npaths:\n  - \"src/**/*.rs\"\n---\nRust style guide",
        )
        .unwrap();

        // Target in tests/ — should NOT match
        let target = root.path().join("tests").join("test.rs");
        let mut loaded = HashSet::new();
        let results = collect_nested_agents_md(root.path(), &[target], &mut loaded, &global);

        assert!(!results.iter().any(|r| r.content.contains("Rust style")));
    }
}
