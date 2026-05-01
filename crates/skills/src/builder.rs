//! Fluent API for constructing skills programmatically.
//!
//! [`SkillBuilder`] provides a builder pattern for creating [`Skill`] instances
//! without parsing frontmatter files.

use crate::types::{Skill, SkillContext, SkillSource, SkillTrigger};

// ─── SkillBuilder ──────────────────────────────────────────────────────

/// Fluent builder for constructing [`Skill`] instances.
///
/// # Example
///
/// ```
/// use crab_skills::builder::SkillBuilder;
///
/// let skill = SkillBuilder::new("commit")
///     .description("Create a git commit with a good message")
///     .content("You are a commit helper. ...")
///     .command_trigger("commit")
///     .allowed_tools(["Read", "Bash"])
///     .build()
///     .unwrap();
/// ```
pub struct SkillBuilder {
    name: String,
    description: Option<String>,
    content: Option<String>,
    trigger: SkillTrigger,
    aliases: Vec<String>,
    when_to_use: Option<String>,
    argument_hint: Option<String>,
    allowed_tools: Vec<String>,
    model: Option<String>,
    disable_model_invocation: bool,
    user_invocable: bool,
    context: SkillContext,
    agent: Option<String>,
    effort: Option<String>,
    source: SkillSource,
    files: Option<std::collections::HashMap<String, String>>,
    hooks: Option<serde_json::Value>,
}

impl SkillBuilder {
    /// Start building a skill with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            content: None,
            trigger: SkillTrigger::Manual,
            aliases: Vec::new(),
            when_to_use: None,
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            context: SkillContext::Inline,
            agent: None,
            effort: None,
            source: SkillSource::Builtin,
            files: None,
            hooks: None,
        }
    }

    /// Set the human-readable description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the skill's prompt content (markdown body).
    #[must_use]
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Set a `/command` trigger.
    #[must_use]
    pub fn command_trigger(mut self, name: impl Into<String>) -> Self {
        self.trigger = SkillTrigger::Command { name: name.into() };
        self
    }

    /// Set a regex pattern trigger.
    #[must_use]
    pub fn pattern_trigger(mut self, regex: impl Into<String>) -> Self {
        self.trigger = SkillTrigger::Pattern {
            regex: regex.into(),
        };
        self
    }

    /// Add aliases for the skill.
    #[must_use]
    pub fn aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    /// Set when the model should use this skill.
    #[must_use]
    pub fn when_to_use(mut self, hint: impl Into<String>) -> Self {
        self.when_to_use = Some(hint.into());
        self
    }

    /// Set the argument hint shown to users.
    #[must_use]
    pub fn argument_hint(mut self, hint: impl Into<String>) -> Self {
        self.argument_hint = Some(hint.into());
        self
    }

    /// Set allowed tools for this skill.
    #[must_use]
    pub fn allowed_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    /// Set the model override.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Disable model invocation (user-only via `/name`).
    #[must_use]
    pub fn disable_model_invocation(mut self) -> Self {
        self.disable_model_invocation = true;
        self
    }

    /// Set whether users can invoke via `/name`.
    #[must_use]
    pub fn user_invocable(mut self, invocable: bool) -> Self {
        self.user_invocable = invocable;
        self
    }

    /// Set the execution context.
    #[must_use]
    pub fn context(mut self, context: SkillContext) -> Self {
        self.context = context;
        self
    }

    /// Set the agent type (for forked execution).
    #[must_use]
    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Set the effort level.
    #[must_use]
    pub fn effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// Set the skill source.
    #[must_use]
    pub fn source(mut self, source: SkillSource) -> Self {
        self.source = source;
        self
    }

    /// Set reference files shipped with the skill.
    #[must_use]
    pub fn files(mut self, files: std::collections::HashMap<String, String>) -> Self {
        self.files = Some(files);
        self
    }

    /// Set hook definitions (opaque JSON).
    #[must_use]
    pub fn hooks(mut self, hooks: serde_json::Value) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Consume the builder and produce a [`Skill`].
    ///
    /// # Errors
    ///
    /// Returns `Err` if the skill name is empty or if no content was provided.
    pub fn build(self) -> Result<Skill, String> {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return Err("skill name must not be empty".into());
        }

        let content = self
            .content
            .filter(|c| !c.trim().is_empty())
            .ok_or("skill content must not be empty")?;

        Ok(Skill {
            name,
            description: self.description.unwrap_or_default(),
            aliases: self.aliases,
            trigger: self.trigger,
            content,
            source_path: None,
            when_to_use: self.when_to_use,
            argument_hint: self.argument_hint,
            allowed_tools: self.allowed_tools,
            model: self.model,
            disable_model_invocation: self.disable_model_invocation,
            user_invocable: self.user_invocable,
            context: self.context,
            agent: self.agent,
            effort: self.effort,
            source: self.source,
            files: self.files,
            hooks: self.hooks,
        })
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_minimal() {
        let skill = SkillBuilder::new("test")
            .content("prompt content")
            .build()
            .unwrap();
        assert_eq!(skill.name, "test");
        assert_eq!(skill.content, "prompt content");
        assert!(matches!(skill.trigger, SkillTrigger::Manual));
        assert!(skill.user_invocable);
        assert!(!skill.disable_model_invocation);
    }

    #[test]
    fn builder_full_chain() {
        let skill = SkillBuilder::new("batch")
            .description("Parallel execution")
            .content("batch prompt")
            .command_trigger("batch")
            .aliases(["parallel", "sweep"])
            .when_to_use("Large-scale changes")
            .argument_hint("<instruction>")
            .allowed_tools(["Read", "Bash"])
            .model("sonnet")
            .disable_model_invocation()
            .context(SkillContext::Fork)
            .agent("code-architect")
            .effort("high")
            .build()
            .unwrap();

        assert_eq!(skill.name, "batch");
        assert_eq!(skill.description, "Parallel execution");
        assert!(matches!(skill.trigger, SkillTrigger::Command { ref name } if name == "batch"));
        assert_eq!(skill.aliases, vec!["parallel", "sweep"]);
        assert_eq!(skill.when_to_use.as_deref(), Some("Large-scale changes"));
        assert_eq!(skill.argument_hint.as_deref(), Some("<instruction>"));
        assert_eq!(skill.allowed_tools, vec!["Read", "Bash"]);
        assert_eq!(skill.model.as_deref(), Some("sonnet"));
        assert!(skill.disable_model_invocation);
        assert_eq!(skill.context, SkillContext::Fork);
        assert_eq!(skill.agent.as_deref(), Some("code-architect"));
        assert_eq!(skill.effort.as_deref(), Some("high"));
    }

    #[test]
    fn builder_empty_name_fails() {
        let result = SkillBuilder::new("").content("x").build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_no_content_fails() {
        let result = SkillBuilder::new("test").build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_whitespace_content_fails() {
        let result = SkillBuilder::new("test").content("  ").build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_command_trigger() {
        let skill = SkillBuilder::new("test")
            .content("prompt")
            .command_trigger("test")
            .build()
            .unwrap();
        assert!(matches!(skill.trigger, SkillTrigger::Command { .. }));
    }

    #[test]
    fn builder_pattern_trigger() {
        let skill = SkillBuilder::new("test")
            .content("prompt")
            .pattern_trigger(r"fix\s+bug")
            .build()
            .unwrap();
        assert!(matches!(skill.trigger, SkillTrigger::Pattern { .. }));
    }

    #[test]
    fn builder_source_default_is_builtin() {
        let skill = SkillBuilder::new("test").content("x").build().unwrap();
        assert_eq!(skill.source, SkillSource::Builtin);
    }

    #[test]
    fn builder_custom_source() {
        let skill = SkillBuilder::new("test")
            .content("x")
            .source(SkillSource::Plugin {
                plugin_name: "my-plugin".into(),
            })
            .build()
            .unwrap();
        assert!(matches!(skill.source, SkillSource::Plugin { .. }));
    }
}
