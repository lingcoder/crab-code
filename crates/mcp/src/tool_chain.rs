//! Tool chain orchestration and templates.
//!
//! Provides [`ToolChain`] for defining sequential tool invocation pipelines
//! where each step's output feeds into the next step's input.
//! [`ToolChainTemplate`] offers pre-built common chains, and [`ChainExecutor`]
//! runs chains with conditional abort on error.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ─── Chain step ────────────────────────────────────────────────────────

/// A single step within a tool chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStep {
    /// Human-readable label for this step.
    pub label: String,
    /// Tool name to invoke.
    pub tool_name: String,
    /// Static parameters merged with piped input.
    pub params: HashMap<String, String>,
    /// Which parameter key receives the previous step's output.
    /// If `None`, previous output is discarded.
    pub input_key: Option<String>,
    /// Abort condition: if the output contains any of these substrings, the
    /// chain stops with an error.
    #[serde(default)]
    pub abort_on: Vec<String>,
}

impl ChainStep {
    /// Create a new step.
    #[must_use]
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            label: String::new(),
            tool_name: tool_name.into(),
            params: HashMap::new(),
            input_key: None,
            abort_on: Vec::new(),
        }
    }

    /// Set the step label.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Set a static parameter.
    #[must_use]
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    /// Set the key that receives piped input from the previous step.
    #[must_use]
    pub fn pipe_into(mut self, key: impl Into<String>) -> Self {
        self.input_key = Some(key.into());
        self
    }

    /// Add an abort condition substring.
    #[must_use]
    pub fn abort_if_contains(mut self, pattern: impl Into<String>) -> Self {
        self.abort_on.push(pattern.into());
        self
    }

    /// Check whether an output string triggers an abort.
    #[must_use]
    pub fn should_abort(&self, output: &str) -> bool {
        self.abort_on
            .iter()
            .any(|pat| output.contains(pat.as_str()))
    }

    /// Build the effective parameters for this step given piped input.
    #[must_use]
    pub fn resolve_params(&self, piped_input: Option<&str>) -> HashMap<String, String> {
        let mut merged = self.params.clone();
        if let (Some(key), Some(input)) = (&self.input_key, piped_input) {
            merged.insert(key.clone(), input.to_string());
        }
        merged
    }
}

// ─── Tool chain ────────────────────────────────────────────────────────

/// An ordered sequence of tool invocations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChain {
    /// Name of this chain.
    pub name: String,
    /// Description of what the chain does.
    pub description: String,
    /// Ordered steps.
    pub steps: Vec<ChainStep>,
}

impl ToolChain {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            steps: Vec::new(),
        }
    }

    /// Number of steps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the chain has no steps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl fmt::Display for ToolChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({} steps)", self.name, self.steps.len())
    }
}

// ─── Chain builder ─────────────────────────────────────────────────────

/// Fluent builder for constructing a [`ToolChain`].
pub struct ChainBuilder {
    name: String,
    description: String,
    steps: Vec<ChainStep>,
}

impl ChainBuilder {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            steps: Vec::new(),
        }
    }

    /// Set chain description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Append a step.
    #[must_use]
    pub fn step(mut self, step: ChainStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Convenience: append a simple tool step with an input pipe key.
    #[must_use]
    pub fn tool(self, tool_name: impl Into<String>, input_key: impl Into<String>) -> Self {
        self.step(ChainStep::new(tool_name).pipe_into(input_key))
    }

    /// Build the chain.
    #[must_use]
    pub fn build(self) -> ToolChain {
        ToolChain {
            name: self.name,
            description: self.description,
            steps: self.steps,
        }
    }
}

// ─── Chain execution ───────────────────────────────────────────────────

/// Outcome of executing a single chain step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_index: usize,
    pub label: String,
    pub tool_name: String,
    pub params: HashMap<String, String>,
    pub output: String,
    pub aborted: bool,
}

/// Outcome of executing an entire chain.
#[derive(Debug, Clone)]
pub struct ChainResult {
    pub chain_name: String,
    pub step_results: Vec<StepResult>,
    pub success: bool,
    /// If the chain aborted, which step caused it.
    pub aborted_at: Option<usize>,
}

impl ChainResult {
    /// Get the final output (last step's output), or `None` if empty.
    #[must_use]
    pub fn final_output(&self) -> Option<&str> {
        self.step_results.last().map(|r| r.output.as_str())
    }

    /// Number of steps that executed.
    #[must_use]
    pub fn steps_executed(&self) -> usize {
        self.step_results.len()
    }
}

/// Executes a [`ToolChain`] step by step, piping outputs forward and checking
/// abort conditions.
///
/// The executor does not call real tools — it delegates each step to a
/// caller-provided function, making it testable and decoupled from the tool
/// system.
pub struct ChainExecutor;

impl ChainExecutor {
    /// Execute a chain using the provided step function.
    ///
    /// `execute_step` receives the tool name and resolved parameters, and
    /// returns the tool output as a `Result<String, String>`. An `Err` is
    /// treated as a tool failure and aborts the chain.
    pub fn execute<F>(chain: &ToolChain, mut execute_step: F) -> ChainResult
    where
        F: FnMut(&str, &HashMap<String, String>) -> Result<String, String>,
    {
        let mut step_results = Vec::new();
        let mut piped: Option<String> = None;

        for (i, step) in chain.steps.iter().enumerate() {
            let params = step.resolve_params(piped.as_deref());

            let output = match execute_step(&step.tool_name, &params) {
                Ok(out) => out,
                Err(err) => {
                    step_results.push(StepResult {
                        step_index: i,
                        label: step.label.clone(),
                        tool_name: step.tool_name.clone(),
                        params,
                        output: err,
                        aborted: true,
                    });
                    return ChainResult {
                        chain_name: chain.name.clone(),
                        step_results,
                        success: false,
                        aborted_at: Some(i),
                    };
                }
            };

            let aborted = step.should_abort(&output);
            step_results.push(StepResult {
                step_index: i,
                label: step.label.clone(),
                tool_name: step.tool_name.clone(),
                params,
                output: output.clone(),
                aborted,
            });

            if aborted {
                return ChainResult {
                    chain_name: chain.name.clone(),
                    step_results,
                    success: false,
                    aborted_at: Some(i),
                };
            }

            piped = Some(output);
        }

        ChainResult {
            chain_name: chain.name.clone(),
            step_results,
            success: true,
            aborted_at: None,
        }
    }
}

// ─── Pre-defined templates ─────────────────────────────────────────────

/// Factory for common tool chain templates.
pub struct ToolChainTemplate;

impl ToolChainTemplate {
    /// Read a file, then edit it.
    /// Step 1 reads the file content, step 2 pipes content into the edit tool.
    #[must_use]
    pub fn read_then_edit(file_path: &str) -> ToolChain {
        ChainBuilder::new("ReadThenEdit")
            .description("Read a file then apply an edit")
            .step(
                ChainStep::new("read_file")
                    .label("Read file")
                    .param("path", file_path),
            )
            .step(
                ChainStep::new("edit_file")
                    .label("Edit file")
                    .param("path", file_path)
                    .pipe_into("content"),
            )
            .build()
    }

    /// Search for files matching a pattern, then read the first result.
    #[must_use]
    pub fn search_then_read(pattern: &str) -> ToolChain {
        ChainBuilder::new("SearchThenRead")
            .description("Search for files then read the first match")
            .step(
                ChainStep::new("glob")
                    .label("Search files")
                    .param("pattern", pattern),
            )
            .step(
                ChainStep::new("read_file")
                    .label("Read first match")
                    .pipe_into("path"),
            )
            .build()
    }

    /// Run a build command, then run tests. Abort if build fails.
    #[must_use]
    pub fn build_then_test(build_cmd: &str, test_cmd: &str) -> ToolChain {
        ChainBuilder::new("BuildThenTest")
            .description("Build the project, then run tests if build succeeds")
            .step(
                ChainStep::new("bash")
                    .label("Build")
                    .param("command", build_cmd)
                    .abort_if_contains("error")
                    .abort_if_contains("FAILED"),
            )
            .step(
                ChainStep::new("bash")
                    .label("Test")
                    .param("command", test_cmd)
                    .abort_if_contains("FAILED"),
            )
            .build()
    }

    /// Grep for a pattern, then read matching files.
    #[must_use]
    pub fn grep_then_read(pattern: &str, path: &str) -> ToolChain {
        ChainBuilder::new("GrepThenRead")
            .description("Grep for a pattern then read matching files")
            .step(
                ChainStep::new("grep")
                    .label("Search content")
                    .param("pattern", pattern)
                    .param("path", path),
            )
            .step(
                ChainStep::new("read_file")
                    .label("Read matching file")
                    .pipe_into("path"),
            )
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ChainStep tests ──

    #[test]
    fn step_new_defaults() {
        let s = ChainStep::new("read_file");
        assert_eq!(s.tool_name, "read_file");
        assert!(s.label.is_empty());
        assert!(s.params.is_empty());
        assert!(s.input_key.is_none());
        assert!(s.abort_on.is_empty());
    }

    #[test]
    fn step_builder_methods() {
        let s = ChainStep::new("bash")
            .label("Run build")
            .param("command", "cargo build")
            .pipe_into("stdin")
            .abort_if_contains("error");
        assert_eq!(s.label, "Run build");
        assert_eq!(s.params["command"], "cargo build");
        assert_eq!(s.input_key.as_deref(), Some("stdin"));
        assert_eq!(s.abort_on, vec!["error"]);
    }

    #[test]
    fn step_should_abort() {
        let s = ChainStep::new("bash")
            .abort_if_contains("error")
            .abort_if_contains("FAILED");
        assert!(s.should_abort("compilation error on line 5"));
        assert!(s.should_abort("test FAILED"));
        assert!(!s.should_abort("all tests passed"));
    }

    #[test]
    fn step_resolve_params_without_pipe() {
        let s = ChainStep::new("read_file").param("path", "/tmp/x");
        let params = s.resolve_params(None);
        assert_eq!(params["path"], "/tmp/x");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn step_resolve_params_with_pipe() {
        let s = ChainStep::new("edit_file")
            .param("path", "/tmp/x")
            .pipe_into("content");
        let params = s.resolve_params(Some("file data here"));
        assert_eq!(params["path"], "/tmp/x");
        assert_eq!(params["content"], "file data here");
    }

    #[test]
    fn step_resolve_params_pipe_overrides_static() {
        let s = ChainStep::new("tool")
            .param("input", "static")
            .pipe_into("input");
        let params = s.resolve_params(Some("piped"));
        assert_eq!(params["input"], "piped");
    }

    #[test]
    fn step_serde_roundtrip() {
        let s = ChainStep::new("bash")
            .label("build")
            .param("cmd", "make")
            .pipe_into("stdin")
            .abort_if_contains("error");
        let json = serde_json::to_string(&s).unwrap();
        let back: ChainStep = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_name, "bash");
        assert_eq!(back.label, "build");
        assert_eq!(back.input_key.as_deref(), Some("stdin"));
        assert_eq!(back.abort_on, vec!["error"]);
    }

    // ── ToolChain tests ──

    #[test]
    fn chain_new_and_display() {
        let c = ToolChain::new("test");
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert_eq!(c.to_string(), "test(0 steps)");
    }

    #[test]
    fn chain_serde_roundtrip() {
        let c = ChainBuilder::new("demo")
            .description("test chain")
            .step(ChainStep::new("read").label("step1"))
            .step(ChainStep::new("write").label("step2"))
            .build();
        let json = serde_json::to_string(&c).unwrap();
        let back: ToolChain = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "demo");
        assert_eq!(back.steps.len(), 2);
    }

    // ── ChainBuilder tests ──

    #[test]
    fn builder_fluent_api() {
        let chain = ChainBuilder::new("pipeline")
            .description("A test pipeline")
            .step(ChainStep::new("glob").label("find"))
            .tool("read_file", "path")
            .step(ChainStep::new("bash").param("cmd", "wc -l").pipe_into("stdin"))
            .build();
        assert_eq!(chain.name, "pipeline");
        assert_eq!(chain.description, "A test pipeline");
        assert_eq!(chain.len(), 3);
        assert_eq!(chain.steps[1].tool_name, "read_file");
        assert_eq!(chain.steps[1].input_key.as_deref(), Some("path"));
    }

    // ── ChainExecutor tests ──

    #[test]
    fn executor_simple_chain_success() {
        let chain = ChainBuilder::new("test")
            .step(ChainStep::new("step1").label("first"))
            .step(ChainStep::new("step2").label("second").pipe_into("input"))
            .build();

        let result = ChainExecutor::execute(&chain, |name, params| {
            match name {
                "step1" => Ok("output_of_step1".to_string()),
                "step2" => {
                    assert_eq!(params.get("input").unwrap(), "output_of_step1");
                    Ok("final_result".to_string())
                }
                _ => Err("unknown tool".to_string()),
            }
        });

        assert!(result.success);
        assert_eq!(result.steps_executed(), 2);
        assert_eq!(result.final_output(), Some("final_result"));
        assert!(result.aborted_at.is_none());
    }

    #[test]
    fn executor_abort_on_error_substring() {
        let chain = ChainBuilder::new("test")
            .step(
                ChainStep::new("build")
                    .abort_if_contains("error")
            )
            .step(ChainStep::new("test"))
            .build();

        let result = ChainExecutor::execute(&chain, |name, _| {
            match name {
                "build" => Ok("compilation error on line 42".to_string()),
                "test" => Ok("passed".to_string()),
                _ => Err("unknown".to_string()),
            }
        });

        assert!(!result.success);
        assert_eq!(result.aborted_at, Some(0));
        assert_eq!(result.steps_executed(), 1);
        assert!(result.step_results[0].aborted);
    }

    #[test]
    fn executor_abort_on_tool_error() {
        let chain = ChainBuilder::new("test")
            .step(ChainStep::new("failing_tool"))
            .step(ChainStep::new("never_reached"))
            .build();

        let result = ChainExecutor::execute(&chain, |name, _| {
            match name {
                "failing_tool" => Err("tool crashed".to_string()),
                _ => Ok("ok".to_string()),
            }
        });

        assert!(!result.success);
        assert_eq!(result.aborted_at, Some(0));
        assert_eq!(result.steps_executed(), 1);
        assert_eq!(result.step_results[0].output, "tool crashed");
    }

    #[test]
    fn executor_empty_chain() {
        let chain = ToolChain::new("empty");
        let result = ChainExecutor::execute(&chain, |_, _| Ok("x".to_string()));
        assert!(result.success);
        assert_eq!(result.steps_executed(), 0);
        assert!(result.final_output().is_none());
    }

    #[test]
    fn executor_three_step_pipeline() {
        let chain = ChainBuilder::new("pipeline")
            .step(ChainStep::new("a"))
            .step(ChainStep::new("b").pipe_into("in"))
            .step(ChainStep::new("c").pipe_into("in"))
            .build();

        let result = ChainExecutor::execute(&chain, |name, params| {
            let prev = params.get("in").cloned().unwrap_or_default();
            Ok(format!("{name}({prev})"))
        });

        assert!(result.success);
        assert_eq!(result.final_output(), Some("c(b(a()))"));
    }

    #[test]
    fn executor_params_include_static_and_piped() {
        let chain = ChainBuilder::new("test")
            .step(ChainStep::new("source"))
            .step(
                ChainStep::new("sink")
                    .param("mode", "append")
                    .pipe_into("data"),
            )
            .build();

        let result = ChainExecutor::execute(&chain, |name, params| {
            if name == "sink" {
                assert_eq!(params.get("mode").unwrap(), "append");
                assert_eq!(params.get("data").unwrap(), "from_source");
            }
            Ok("from_source".to_string())
        });

        assert!(result.success);
    }

    // ── Template tests ──

    #[test]
    fn template_read_then_edit() {
        let chain = ToolChainTemplate::read_then_edit("/src/main.rs");
        assert_eq!(chain.name, "ReadThenEdit");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.steps[0].tool_name, "read_file");
        assert_eq!(chain.steps[0].params["path"], "/src/main.rs");
        assert_eq!(chain.steps[1].tool_name, "edit_file");
        assert_eq!(chain.steps[1].input_key.as_deref(), Some("content"));
    }

    #[test]
    fn template_search_then_read() {
        let chain = ToolChainTemplate::search_then_read("**/*.rs");
        assert_eq!(chain.name, "SearchThenRead");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.steps[0].params["pattern"], "**/*.rs");
        assert_eq!(chain.steps[1].input_key.as_deref(), Some("path"));
    }

    #[test]
    fn template_build_then_test() {
        let chain = ToolChainTemplate::build_then_test("cargo build", "cargo test");
        assert_eq!(chain.name, "BuildThenTest");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.steps[0].params["command"], "cargo build");
        assert!(chain.steps[0].abort_on.contains(&"error".to_string()));
        assert_eq!(chain.steps[1].params["command"], "cargo test");
    }

    #[test]
    fn template_grep_then_read() {
        let chain = ToolChainTemplate::grep_then_read("TODO", "/proj");
        assert_eq!(chain.name, "GrepThenRead");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.steps[0].params["pattern"], "TODO");
        assert_eq!(chain.steps[0].params["path"], "/proj");
    }

    #[test]
    fn template_build_then_test_aborts_on_build_error() {
        let chain = ToolChainTemplate::build_then_test("make", "make test");
        let result = ChainExecutor::execute(&chain, |_name, params| {
            if params.get("command") == Some(&"make".to_string()) {
                Ok("error: undefined reference to main".to_string())
            } else {
                Ok("all tests passed".to_string())
            }
        });
        assert!(!result.success);
        assert_eq!(result.aborted_at, Some(0));
        assert_eq!(result.steps_executed(), 1);
    }

    #[test]
    fn template_build_then_test_succeeds() {
        let chain = ToolChainTemplate::build_then_test("make", "make test");
        let result = ChainExecutor::execute(&chain, |_name, params| {
            if params.get("command") == Some(&"make".to_string()) {
                Ok("build successful".to_string())
            } else {
                Ok("42 tests passed".to_string())
            }
        });
        assert!(result.success);
        assert_eq!(result.steps_executed(), 2);
        assert_eq!(result.final_output(), Some("42 tests passed"));
    }

    // ── ChainResult tests ──

    #[test]
    fn chain_result_final_output_empty() {
        let r = ChainResult {
            chain_name: "x".into(),
            step_results: vec![],
            success: true,
            aborted_at: None,
        };
        assert!(r.final_output().is_none());
        assert_eq!(r.steps_executed(), 0);
    }
}
