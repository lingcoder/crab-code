//! Skill registry — discovery, registration, and lookup.
//!
//! [`SkillRegistry`] aggregates skills from multiple sources (bundled, disk,
//! plugin, MCP) and provides lookup by name, slash command, or input matching.

use std::path::PathBuf;

use crate::frontmatter::load_skill_file;
use crate::types::{Skill, SkillTrigger};

// ─── SkillRegistry ─────────────────────────────────────────────────────

/// Registry of loaded skills with lookup and matching.
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self { skills: Vec::new() }
    }

    /// Discover and load skills from one or more directories.
    ///
    /// Each directory is scanned for `.md` files with YAML frontmatter
    /// containing skill metadata. The markdown body becomes the skill content.
    ///
    /// Directories are scanned in order; later skills with the same name
    /// override earlier ones (project skills override global ones).
    pub fn discover(paths: &[PathBuf]) -> crab_common::Result<Self> {
        let mut registry = Self::new();

        for dir in paths {
            if !dir.exists() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "md") {
                        match load_skill_file(&path) {
                            Ok(skill) => {
                                tracing::debug!(
                                    name = skill.name.as_str(),
                                    path = %path.display(),
                                    "loaded skill"
                                );
                                registry.register(skill);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    path = %path.display(),
                                    error = %e,
                                    "failed to load skill file"
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(registry)
    }

    /// Register a skill (replaces existing skill with the same name).
    pub fn register(&mut self, skill: Skill) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.name == skill.name) {
            *existing = skill;
        } else {
            self.skills.push(skill);
        }
    }

    /// Register multiple skills at once.
    pub fn register_all(&mut self, skills: impl IntoIterator<Item = Skill>) {
        for skill in skills {
            self.register(skill);
        }
    }

    /// Find a skill by exact name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Skill> {
        self.skills
            .iter()
            .find(|s| s.name == name || s.aliases.iter().any(|a| a == name))
    }

    /// Find a skill by slash command name.
    #[must_use]
    pub fn find_command(&self, command: &str) -> Option<&Skill> {
        self.skills
            .iter()
            .find(|s| matches!(&s.trigger, SkillTrigger::Command { name } if name == command))
    }

    /// Find all skills whose trigger pattern matches the given input.
    #[must_use]
    pub fn match_input(&self, input: &str) -> Vec<&Skill> {
        self.skills
            .iter()
            .filter(|s| match &s.trigger {
                SkillTrigger::Pattern { regex } => {
                    regex::Regex::new(regex).is_ok_and(|re| re.is_match(input))
                }
                SkillTrigger::Command { name } => {
                    input.starts_with('/') && input.trim_start_matches('/') == name.as_str()
                }
                SkillTrigger::Manual => false,
            })
            .collect()
    }

    /// List all skills available for model invocation.
    ///
    /// Filters out skills with `disable_model_invocation: true`.
    #[must_use]
    pub fn model_invocable(&self) -> Vec<&Skill> {
        self.skills
            .iter()
            .filter(|s| !s.disable_model_invocation && !s.description.is_empty())
            .collect()
    }

    /// List all skills available for user invocation via `/name`.
    #[must_use]
    pub fn user_invocable(&self) -> Vec<&Skill> {
        self.skills.iter().filter(|s| s.user_invocable).collect()
    }

    /// List all registered skills.
    #[must_use]
    pub fn list(&self) -> &[Skill] {
        &self.skills
    }

    /// Number of registered skills.
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::types::{SkillSource, SkillTrigger};

    use super::*;

    fn make_skill(name: &str) -> Skill {
        Skill {
            name: name.into(),
            trigger: SkillTrigger::Manual,
            ..Skill::new(name, "content")
        }
    }

    #[test]
    fn register_and_find() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("test"));

        assert_eq!(reg.len(), 1);
        assert!(reg.find("test").is_some());
        assert!(reg.find("missing").is_none());
    }

    #[test]
    fn override_same_name() {
        let mut reg = SkillRegistry::new();
        let mut s1 = make_skill("x");
        s1.description = "first".into();
        reg.register(s1);

        let mut s2 = make_skill("x");
        s2.description = "second".into();
        reg.register(s2);

        assert_eq!(reg.len(), 1);
        assert_eq!(reg.find("x").unwrap().description, "second");
    }

    #[test]
    fn find_by_alias() {
        let mut reg = SkillRegistry::new();
        let mut skill = make_skill("commit");
        skill.aliases = vec!["ci".into(), "c".into()];
        reg.register(skill);

        assert!(reg.find("commit").is_some());
        assert!(reg.find("ci").is_some());
        assert!(reg.find("c").is_some());
        assert!(reg.find("x").is_none());
    }

    #[test]
    fn find_command() {
        let mut reg = SkillRegistry::new();
        let mut skill = make_skill("commit");
        skill.trigger = SkillTrigger::Command {
            name: "commit".into(),
        };
        reg.register(skill);

        assert!(reg.find_command("commit").is_some());
        assert!(reg.find_command("other").is_none());
    }

    #[test]
    fn match_input_command() {
        let mut reg = SkillRegistry::new();
        let mut skill = make_skill("commit");
        skill.trigger = SkillTrigger::Command {
            name: "commit".into(),
        };
        reg.register(skill);

        let matches = reg.match_input("/commit");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "commit");
        assert!(reg.match_input("commit").is_empty());
    }

    #[test]
    fn match_input_pattern() {
        let mut reg = SkillRegistry::new();
        let mut skill = make_skill("fix-bug");
        skill.trigger = SkillTrigger::Pattern {
            regex: r"(?i)fix\s+bug".into(),
        };
        reg.register(skill);

        assert_eq!(reg.match_input("please fix bug #123").len(), 1);
        assert!(reg.match_input("add feature").is_empty());
    }

    #[test]
    fn match_input_manual_never_matches() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("manual-skill"));
        assert!(reg.match_input("manual-skill").is_empty());
        assert!(reg.match_input("/manual-skill").is_empty());
    }

    #[test]
    fn match_input_multiple_matches() {
        let mut reg = SkillRegistry::new();
        let mut s1 = make_skill("broad");
        s1.trigger = SkillTrigger::Pattern {
            regex: r".*".into(),
        };
        reg.register(s1);

        let mut s2 = make_skill("specific");
        s2.trigger = SkillTrigger::Pattern {
            regex: r"fix".into(),
        };
        reg.register(s2);

        assert_eq!(reg.match_input("fix this bug").len(), 2);
    }

    #[test]
    fn register_all() {
        let mut reg = SkillRegistry::new();
        reg.register_all(vec![make_skill("a"), make_skill("b"), make_skill("c")]);
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn model_invocable_filters() {
        let mut reg = SkillRegistry::new();

        let mut s1 = make_skill("visible");
        s1.description = "has description".into();
        s1.disable_model_invocation = false;
        reg.register(s1);

        let mut s2 = make_skill("hidden");
        s2.description = "also described".into();
        s2.disable_model_invocation = true;
        reg.register(s2);

        let mut s3 = make_skill("no-desc");
        s3.description = String::new();
        reg.register(s3);

        let invocable = reg.model_invocable();
        assert_eq!(invocable.len(), 1);
        assert_eq!(invocable[0].name, "visible");
    }

    #[test]
    fn user_invocable_filters() {
        let mut reg = SkillRegistry::new();

        let s1 = make_skill("public");
        reg.register(s1);

        let mut s2 = make_skill("internal");
        s2.user_invocable = false;
        reg.register(s2);

        let invocable = reg.user_invocable();
        assert_eq!(invocable.len(), 1);
        assert_eq!(invocable[0].name, "public");
    }

    #[test]
    fn discover_empty_dir() {
        let tmp = std::env::temp_dir().join("crab_skill_test_empty_reg");
        let _ = std::fs::create_dir_all(&tmp);
        let reg = SkillRegistry::discover(std::slice::from_ref(&tmp)).unwrap();
        assert!(reg.is_empty());
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn discover_nonexistent_dir() {
        let reg = SkillRegistry::discover(&[PathBuf::from("/nonexistent/path/skills")]).unwrap();
        assert!(reg.is_empty());
    }

    #[test]
    fn discover_with_skill_files() {
        let tmp = std::env::temp_dir().join("crab_skill_test_discover_reg");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(
            tmp.join("commit.md"),
            "---\nname: commit\ndescription: Create commit\ntrigger:\n  type: command\n  name: commit\n---\nCommit helper.",
        )
        .unwrap();

        let reg = SkillRegistry::discover(std::slice::from_ref(&tmp)).unwrap();
        assert_eq!(reg.len(), 1);
        let skill = reg.find("commit").unwrap();
        assert_eq!(skill.source, SkillSource::Disk);
        assert!(reg.find_command("commit").is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_skips_non_md_files() {
        let tmp = std::env::temp_dir().join("crab_skill_test_nonmd_reg");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(tmp.join("notes.txt"), "not a skill").unwrap();
        let reg = SkillRegistry::discover(std::slice::from_ref(&tmp)).unwrap();
        assert!(reg.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_returns_all() {
        let mut reg = SkillRegistry::new();
        reg.register(make_skill("a"));
        reg.register(make_skill("b"));
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = SkillRegistry::default();
        assert!(reg.is_empty());
    }
}
