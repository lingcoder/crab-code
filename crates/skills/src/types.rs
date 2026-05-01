//! Core skill types.
//!
//! Defines [`Skill`], [`SkillTrigger`], [`SkillContext`], and [`SkillSource`].
//! Aligned with CCB `types/command.ts` (`PromptCommand` + `BuiltinSkillDefinition`).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ─── Skill ─────────────────────────────────────────────────────────────

/// A skill definition — a prompt template that can be triggered by user input,
/// slash commands, or model invocation.
///
/// Skills live in `.crab/skills/` directories (disk-based) or are compiled
/// into the binary (built-in). Each skill produces prompt content that is
/// injected into the conversation when activated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    // ─── Identity ───────────────────────────────────────────────────
    /// Unique skill name (e.g. `"commit"`, `"review-pr"`).
    pub name: String,

    /// Human-readable description shown in listings and to the model.
    #[serde(default)]
    pub description: String,

    /// Alternative names that also resolve to this skill.
    #[serde(default)]
    pub aliases: Vec<String>,

    // ─── Trigger ────────────────────────────────────────────────────
    /// How this skill is activated.
    #[serde(default)]
    pub trigger: SkillTrigger,

    // ─── Content ────────────────────────────────────────────────────
    /// The skill's prompt content (markdown body after frontmatter).
    #[serde(skip)]
    pub content: String,

    /// Source file path (for debugging/reloading).
    #[serde(skip)]
    pub source_path: Option<PathBuf>,

    // ─── Model invocation metadata ──────────────────────────────────
    /// Guidance for when the model should invoke this skill.
    #[serde(default)]
    pub when_to_use: Option<String>,

    /// Hint for expected arguments (e.g. `"[interval] <prompt>"`).
    #[serde(default)]
    pub argument_hint: Option<String>,

    // ─── Access control ─────────────────────────────────────────────
    /// Tool names the model is allowed to use within this skill.
    /// Empty means no restriction.
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Model override (e.g. `"sonnet"`, `"haiku"`).
    #[serde(default)]
    pub model: Option<String>,

    /// If `true`, the model cannot auto-invoke this skill — only users
    /// can activate it via `/name`.
    #[serde(default)]
    pub disable_model_invocation: bool,

    /// Whether users can invoke this skill via `/name` syntax.
    /// Defaults to `true`.
    #[serde(default = "default_true")]
    pub user_invocable: bool,

    // ─── Execution context ──────────────────────────────────────────
    /// Execution mode: inline in current session or forked as sub-agent.
    #[serde(default)]
    pub context: SkillContext,

    /// Agent type to use when `context` is `Fork`.
    #[serde(default)]
    pub agent: Option<String>,

    /// Effort level hint (`"low"`, `"medium"`, `"high"`, `"max"`).
    #[serde(default)]
    pub effort: Option<String>,

    // ─── Source tracking ────────────────────────────────────────────
    /// Where this skill was loaded from.
    #[serde(default)]
    pub source: SkillSource,

    // ─── Reference files ────────────────────────────────────────────
    /// Files shipped with the skill, extracted to disk on first invocation.
    /// Keys are relative paths, values are file contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<HashMap<String, String>>,

    // ─── Hooks ──────────────────────────────────────────────────────
    /// Hook definitions from frontmatter, consumed by the plugin layer.
    /// Stored as opaque JSON to avoid coupling with the hook type system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<serde_json::Value>,
}

impl Skill {
    /// Create a minimal skill with just a name and content.
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            aliases: Vec::new(),
            trigger: SkillTrigger::default(),
            content: content.into(),
            source_path: None,
            when_to_use: None,
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            context: SkillContext::default(),
            agent: None,
            effort: None,
            source: SkillSource::default(),
            files: None,
            hooks: None,
        }
    }
}

fn default_true() -> bool {
    true
}

// ─── SkillTrigger ──────────────────────────────────────────────────────

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

// ─── SkillContext ──────────────────────────────────────────────────────

/// Execution context for a skill.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillContext {
    /// Execute inline in the current session (default).
    #[default]
    Inline,
    /// Fork a sub-agent to execute the skill.
    Fork,
}

// ─── SkillSource ──────────────────────────────────────────────────────

/// Where a skill was loaded from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillSource {
    /// Compiled into the binary.
    #[default]
    Builtin,
    /// Loaded from a `.md` file on disk.
    Disk,
    /// Provided by a plugin.
    Plugin {
        /// Name of the plugin that provided this skill.
        plugin_name: String,
    },
    /// Provided by an MCP server.
    Mcp {
        /// Name of the MCP server that provided this skill.
        server_name: String,
    },
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_new_defaults() {
        let skill = Skill::new("test", "prompt content");
        assert_eq!(skill.name, "test");
        assert_eq!(skill.content, "prompt content");
        assert!(skill.description.is_empty());
        assert!(skill.aliases.is_empty());
        assert!(matches!(skill.trigger, SkillTrigger::Manual));
        assert!(skill.user_invocable);
        assert!(!skill.disable_model_invocation);
        assert_eq!(skill.context, SkillContext::Inline);
        assert_eq!(skill.source, SkillSource::Builtin);
        assert!(skill.allowed_tools.is_empty());
        assert!(skill.files.is_none());
        assert!(skill.hooks.is_none());
    }

    #[test]
    fn skill_trigger_default_is_manual() {
        assert!(matches!(SkillTrigger::default(), SkillTrigger::Manual));
    }

    #[test]
    fn skill_context_default_is_inline() {
        assert_eq!(SkillContext::default(), SkillContext::Inline);
    }

    #[test]
    fn skill_source_default_is_builtin() {
        assert_eq!(SkillSource::default(), SkillSource::Builtin);
    }

    #[test]
    fn skill_serde_roundtrip() {
        let skill = Skill {
            name: "test".into(),
            description: "A test".into(),
            trigger: SkillTrigger::Command {
                name: "test".into(),
            },
            source: SkillSource::Disk,
            user_invocable: true,
            ..Skill::new("test", "")
        };
        let json = serde_json::to_string(&skill).unwrap();
        let parsed: Skill = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert!(matches!(parsed.trigger, SkillTrigger::Command { name } if name == "test"));
        assert_eq!(parsed.source, SkillSource::Disk);
    }

    #[test]
    fn skill_source_plugin_variant() {
        let source = SkillSource::Plugin {
            plugin_name: "my-plugin".into(),
        };
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("plugin"));
        assert!(json.contains("my-plugin"));
    }

    #[test]
    fn skill_source_mcp_variant() {
        let source = SkillSource::Mcp {
            server_name: "filesystem".into(),
        };
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("mcp"));
        assert!(json.contains("filesystem"));
    }

    #[test]
    fn skill_context_serde() {
        let inline: SkillContext = serde_json::from_str(r#""inline""#).unwrap();
        assert_eq!(inline, SkillContext::Inline);
        let fork: SkillContext = serde_json::from_str(r#""fork""#).unwrap();
        assert_eq!(fork, SkillContext::Fork);
    }
}
