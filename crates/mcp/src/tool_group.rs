//! Tool grouping and fuzzy search.
//!
//! Provides [`ToolGroup`] for categorizing tools (builtin / mcp / plugin / user)
//! and [`ToolIndex`] for group-based filtering and keyword search across tool
//! names and descriptions.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Category a tool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGroup {
    /// Built-in tools shipped with the binary (Read, Write, Bash, etc.).
    Builtin,
    /// Tools discovered via MCP servers.
    Mcp,
    /// Tools loaded from plugins (WASM / dynamic).
    Plugin,
    /// User-defined custom tools.
    User,
}

impl fmt::Display for ToolGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Mcp => write!(f, "mcp"),
            Self::Plugin => write!(f, "plugin"),
            Self::User => write!(f, "user"),
        }
    }
}

impl std::str::FromStr for ToolGroup {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "builtin" => Ok(Self::Builtin),
            "mcp" => Ok(Self::Mcp),
            "plugin" => Ok(Self::Plugin),
            "user" => Ok(Self::User),
            other => Err(format!("unknown tool group: {other}")),
        }
    }
}

/// A tool entry within the index, carrying its group plus searchable metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedTool {
    pub name: String,
    pub description: String,
    pub group: ToolGroup,
}

impl IndexedTool {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        group: ToolGroup,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            group,
        }
    }
}

/// Index of tools supporting group filtering and keyword search.
#[derive(Debug, Default)]
pub struct ToolIndex {
    tools: Vec<IndexedTool>,
}

impl ToolIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool to the index.
    pub fn add(&mut self, tool: IndexedTool) {
        self.tools.push(tool);
    }

    /// Return all indexed tools.
    #[must_use]
    pub fn all(&self) -> &[IndexedTool] {
        &self.tools
    }

    /// Number of indexed tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Filter tools by group.
    #[must_use]
    pub fn by_group(&self, group: ToolGroup) -> Vec<&IndexedTool> {
        self.tools.iter().filter(|t| t.group == group).collect()
    }

    /// List the distinct groups that have at least one tool.
    #[must_use]
    pub fn groups(&self) -> Vec<ToolGroup> {
        let mut seen = Vec::new();
        for t in &self.tools {
            if !seen.contains(&t.group) {
                seen.push(t.group);
            }
        }
        seen
    }

    /// Search tools by keyword. Matches case-insensitively against name and
    /// description. Multiple space-separated keywords are ANDed.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&IndexedTool> {
        let keywords: Vec<String> = query
            .split_whitespace()
            .map(|k| k.to_ascii_lowercase())
            .collect();
        if keywords.is_empty() {
            return self.tools.iter().collect();
        }
        self.tools
            .iter()
            .filter(|t| {
                let name_lower = t.name.to_ascii_lowercase();
                let desc_lower = t.description.to_ascii_lowercase();
                keywords
                    .iter()
                    .all(|kw| name_lower.contains(kw.as_str()) || desc_lower.contains(kw.as_str()))
            })
            .collect()
    }

    /// Search tools by keyword within a specific group.
    #[must_use]
    pub fn search_in_group(&self, query: &str, group: ToolGroup) -> Vec<&IndexedTool> {
        let keywords: Vec<String> = query
            .split_whitespace()
            .map(|k| k.to_ascii_lowercase())
            .collect();
        self.tools
            .iter()
            .filter(|t| {
                if t.group != group {
                    return false;
                }
                if keywords.is_empty() {
                    return true;
                }
                let name_lower = t.name.to_ascii_lowercase();
                let desc_lower = t.description.to_ascii_lowercase();
                keywords
                    .iter()
                    .all(|kw| name_lower.contains(kw.as_str()) || desc_lower.contains(kw.as_str()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_index() -> ToolIndex {
        let mut idx = ToolIndex::new();
        idx.add(IndexedTool::new("read_file", "Read a file from disk", ToolGroup::Builtin));
        idx.add(IndexedTool::new("write_file", "Write content to a file", ToolGroup::Builtin));
        idx.add(IndexedTool::new("bash", "Execute shell commands", ToolGroup::Builtin));
        idx.add(IndexedTool::new("github_search", "Search GitHub repos", ToolGroup::Mcp));
        idx.add(IndexedTool::new("jira_create", "Create JIRA tickets", ToolGroup::Mcp));
        idx.add(IndexedTool::new("wasm_transform", "Transform data via WASM", ToolGroup::Plugin));
        idx.add(IndexedTool::new("my_script", "User custom script runner", ToolGroup::User));
        idx
    }

    #[test]
    fn tool_group_display_and_parse() {
        for (group, label) in [
            (ToolGroup::Builtin, "builtin"),
            (ToolGroup::Mcp, "mcp"),
            (ToolGroup::Plugin, "plugin"),
            (ToolGroup::User, "user"),
        ] {
            assert_eq!(group.to_string(), label);
            assert_eq!(label.parse::<ToolGroup>().unwrap(), group);
        }
        assert!("unknown".parse::<ToolGroup>().is_err());
    }

    #[test]
    fn tool_group_serde_roundtrip() {
        let g = ToolGroup::Mcp;
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(json, r#""mcp""#);
        let back: ToolGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ToolGroup::Mcp);
    }

    #[test]
    fn index_add_and_len() {
        let idx = sample_index();
        assert_eq!(idx.len(), 7);
        assert!(!idx.is_empty());
        assert!(ToolIndex::new().is_empty());
    }

    #[test]
    fn index_by_group() {
        let idx = sample_index();
        assert_eq!(idx.by_group(ToolGroup::Builtin).len(), 3);
        assert_eq!(idx.by_group(ToolGroup::Mcp).len(), 2);
        assert_eq!(idx.by_group(ToolGroup::Plugin).len(), 1);
        assert_eq!(idx.by_group(ToolGroup::User).len(), 1);
    }

    #[test]
    fn index_groups() {
        let idx = sample_index();
        let groups = idx.groups();
        assert_eq!(groups.len(), 4);
        assert!(groups.contains(&ToolGroup::Builtin));
        assert!(groups.contains(&ToolGroup::Mcp));
    }

    #[test]
    fn search_single_keyword() {
        let idx = sample_index();
        let results = idx.search("file");
        assert_eq!(results.len(), 2); // read_file, write_file
    }

    #[test]
    fn search_case_insensitive() {
        let idx = sample_index();
        let results = idx.search("FILE");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_multiple_keywords_and() {
        let idx = sample_index();
        // "read" AND "file" → only read_file
        let results = idx.search("read file");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "read_file");
    }

    #[test]
    fn search_matches_description() {
        let idx = sample_index();
        let results = idx.search("shell");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "bash");
    }

    #[test]
    fn search_empty_query_returns_all() {
        let idx = sample_index();
        assert_eq!(idx.search("").len(), 7);
        assert_eq!(idx.search("   ").len(), 7);
    }

    #[test]
    fn search_no_match() {
        let idx = sample_index();
        assert!(idx.search("nonexistent_xyz").is_empty());
    }

    #[test]
    fn search_in_group() {
        let idx = sample_index();
        let results = idx.search_in_group("file", ToolGroup::Builtin);
        assert_eq!(results.len(), 2);
        // Same keyword but in Mcp group → nothing
        let results = idx.search_in_group("file", ToolGroup::Mcp);
        assert!(results.is_empty());
    }

    #[test]
    fn search_in_group_empty_query() {
        let idx = sample_index();
        let results = idx.search_in_group("", ToolGroup::Mcp);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn indexed_tool_serde_roundtrip() {
        let t = IndexedTool::new("test", "A test tool", ToolGroup::Plugin);
        let json = serde_json::to_string(&t).unwrap();
        let back: IndexedTool = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "test");
        assert_eq!(back.group, ToolGroup::Plugin);
    }
}
