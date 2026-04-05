//! Skill discovery, loading, and matching.
//!
//! Skills are prompt templates that activate based on user input (slash commands,
//! pattern matches, or manual invocation). They live in `.crab/skills/` directories.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A skill definition loaded from a `.md` file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique skill name (e.g. "commit", "review-pr").
    pub name: String,
    /// Human-readable description shown in listings.
    #[serde(default)]
    pub description: String,
    /// How this skill is triggered.
    #[serde(default)]
    pub trigger: SkillTrigger,
    /// The skill's prompt content (markdown body after frontmatter).
    #[serde(skip)]
    pub content: String,
    /// Source file path (for debugging/reloading).
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

/// How a skill is activated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillTrigger {
    /// Activated by `/command` slash syntax.
    Command {
        /// The slash command name (without the leading `/`).
        name: String,
    },
    /// Activated when user input matches a regex pattern.
    Pattern {
        /// Regex pattern to match against user input.
        regex: String,
    },
    /// Only activated when explicitly called by name.
    #[default]
    Manual,
}

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

    /// Find a skill by exact name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
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

/// Load a skill from a markdown file with YAML frontmatter.
///
/// Expected format:
/// ```text
/// ---
/// name: commit
/// description: Create a git commit
/// trigger:
///   type: command
///   name: commit
/// ---
///
/// Skill prompt content goes here...
/// ```
fn load_skill_file(path: &Path) -> crab_common::Result<Skill> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| crab_common::Error::Other(format!("failed to read skill file: {e}")))?;

    parse_skill_content(&content, Some(path))
}

/// Parse skill content from a string (frontmatter + body).
fn parse_skill_content(content: &str, source_path: Option<&Path>) -> crab_common::Result<Skill> {
    let (frontmatter, body) = split_frontmatter(content)?;

    let mut skill: Skill = serde_json::from_value(parse_simple_yaml(&frontmatter))
        .map_err(|e| crab_common::Error::Other(format!("invalid skill frontmatter: {e}")))?;

    skill.content = body;
    skill.source_path = source_path.map(Path::to_path_buf);

    if skill.name.is_empty() {
        // Fall back to filename without extension.
        if let Some(stem) = source_path.and_then(|p| p.file_stem()) {
            skill.name = stem.to_string_lossy().into_owned();
        }
    }

    Ok(skill)
}

/// Split frontmatter from body. Frontmatter is delimited by `---` lines.
fn split_frontmatter(content: &str) -> crab_common::Result<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(crab_common::Error::Other(
            "skill file must start with '---' frontmatter delimiter".into(),
        ));
    }

    // Find the closing ---
    let after_first = &trimmed[3..].trim_start_matches(['\r', '\n']);
    after_first.find("\n---").map_or_else(
        || {
            Err(crab_common::Error::Other(
                "skill file missing closing '---' frontmatter delimiter".into(),
            ))
        },
        |end_pos| {
            let frontmatter = after_first[..end_pos].to_string();
            let body_start = end_pos + 4; // skip \n---
            let body = after_first[body_start..]
                .trim_start_matches(['\r', '\n'])
                .to_string();
            Ok((frontmatter, body))
        },
    )
}

/// Parse a simple YAML-like frontmatter into a JSON value.
///
/// Supports flat key-value pairs and one level of nesting via indentation.
/// This avoids pulling in a full YAML parser dependency.
fn parse_simple_yaml(yaml: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut current_nested_key: Option<String> = None;
    let mut nested_map = serde_json::Map::new();

    for line in yaml.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();

        if indent >= 2 {
            // Nested value under current_nested_key.
            if let Some(colon_pos) = trimmed.find(':') {
                let key = trimmed[..colon_pos].trim();
                let value = trimmed[colon_pos + 1..].trim();
                nested_map.insert(
                    key.to_string(),
                    serde_json::Value::String(value.to_string()),
                );
            }
        } else if let Some(colon_pos) = trimmed.find(':') {
            // Flush previous nested map.
            if let Some(ref nk) = current_nested_key
                && !nested_map.is_empty()
            {
                map.insert(nk.clone(), serde_json::Value::Object(nested_map.clone()));
                nested_map.clear();
            }

            let key = trimmed[..colon_pos].trim().to_string();
            let value = trimmed[colon_pos + 1..].trim();

            if value.is_empty() {
                // This key has nested children.
                current_nested_key = Some(key);
            } else {
                current_nested_key = None;
                map.insert(key, serde_json::Value::String(value.to_string()));
            }
        }
    }

    // Flush final nested map.
    if let Some(ref nk) = current_nested_key
        && !nested_map.is_empty()
    {
        map.insert(nk.clone(), serde_json::Value::Object(nested_map));
    }

    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let content = "---\nname: test\n---\nBody here";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm, "name: test");
        assert_eq!(body, "Body here");
    }

    #[test]
    fn split_frontmatter_missing_close() {
        let content = "---\nname: test\nno closing";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn split_frontmatter_no_start() {
        let content = "no frontmatter here";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn parse_simple_yaml_flat() {
        let yaml = "name: commit\ndescription: Create a commit";
        let val = parse_simple_yaml(yaml);
        assert_eq!(val["name"], "commit");
        assert_eq!(val["description"], "Create a commit");
    }

    #[test]
    fn parse_simple_yaml_nested() {
        let yaml = "name: test\ntrigger:\n  type: command\n  name: test";
        let val = parse_simple_yaml(yaml);
        assert_eq!(val["name"], "test");
        assert_eq!(val["trigger"]["type"], "command");
        assert_eq!(val["trigger"]["name"], "test");
    }

    #[test]
    fn parse_skill_content_command_trigger() {
        let content = "---\nname: commit\ndescription: Create a git commit\ntrigger:\n  type: command\n  name: commit\n---\nYou are a commit helper.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.name, "commit");
        assert_eq!(skill.description, "Create a git commit");
        assert_eq!(skill.content, "You are a commit helper.");
        assert!(matches!(
            skill.trigger,
            SkillTrigger::Command { ref name } if name == "commit"
        ));
    }

    #[test]
    fn parse_skill_content_manual_trigger() {
        let content = "---\nname: helper\ndescription: A helper skill\n---\nHelp content.";
        let skill = parse_skill_content(content, None).unwrap();
        assert_eq!(skill.name, "helper");
        assert!(matches!(skill.trigger, SkillTrigger::Manual));
    }

    #[test]
    fn registry_register_and_find() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            name: "test".into(),
            description: "Test skill".into(),
            trigger: SkillTrigger::Manual,
            content: "content".into(),
            source_path: None,
        });

        assert_eq!(reg.len(), 1);
        assert!(reg.find("test").is_some());
        assert!(reg.find("missing").is_none());
    }

    #[test]
    fn registry_override_same_name() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            name: "x".into(),
            description: "first".into(),
            trigger: SkillTrigger::Manual,
            content: String::new(),
            source_path: None,
        });
        reg.register(Skill {
            name: "x".into(),
            description: "second".into(),
            trigger: SkillTrigger::Manual,
            content: String::new(),
            source_path: None,
        });

        assert_eq!(reg.len(), 1);
        assert_eq!(reg.find("x").unwrap().description, "second");
    }

    #[test]
    fn registry_find_command() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            name: "commit".into(),
            description: String::new(),
            trigger: SkillTrigger::Command {
                name: "commit".into(),
            },
            content: String::new(),
            source_path: None,
        });

        assert!(reg.find_command("commit").is_some());
        assert!(reg.find_command("other").is_none());
    }

    #[test]
    fn registry_match_input_command() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            name: "commit".into(),
            description: String::new(),
            trigger: SkillTrigger::Command {
                name: "commit".into(),
            },
            content: String::new(),
            source_path: None,
        });

        let matches = reg.match_input("/commit");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "commit");

        assert!(reg.match_input("commit").is_empty()); // no slash
    }

    #[test]
    fn registry_match_input_pattern() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            name: "fix-bug".into(),
            description: String::new(),
            trigger: SkillTrigger::Pattern {
                regex: r"(?i)fix\s+bug".into(),
            },
            content: String::new(),
            source_path: None,
        });

        assert_eq!(reg.match_input("please fix bug #123").len(), 1);
        assert!(reg.match_input("add feature").is_empty());
    }

    #[test]
    fn registry_discover_empty_dir() {
        let tmp = std::env::temp_dir().join("crab_skill_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let reg = SkillRegistry::discover(&[tmp.clone()]).unwrap();
        assert!(reg.is_empty());
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn registry_discover_nonexistent_dir() {
        let reg = SkillRegistry::discover(&[PathBuf::from("/nonexistent/path/skills")]).unwrap();
        assert!(reg.is_empty());
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = SkillRegistry::default();
        assert!(reg.is_empty());
    }
}
