use std::fmt;
use std::fmt::Write as _;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ─── MemoryType ─────────────────────────────────────────────────

/// Classification of a memory file: `User`, `Feedback`, `Project`, or `Reference`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        };
        f.write_str(s)
    }
}

impl FromStr for MemoryType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "user" => Ok(Self::User),
            "feedback" => Ok(Self::Feedback),
            "project" => Ok(Self::Project),
            "reference" => Ok(Self::Reference),
            other => Err(format!("unknown memory type: {other}")),
        }
    }
}

// ─── MemoryMetadata ─────────────────────────────────────────────

/// Frontmatter metadata for a single memory file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryMetadata {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

// ─── Frontmatter helpers ────────────────────────────────────────

/// Split raw file content into (`frontmatter_str`, rest) if `---` delimiters
/// are present; returns `None` otherwise.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    let trimmed = trimmed.strip_prefix("---")?;
    // Find the closing `---`
    let end = trimmed.find("\n---")?;
    let front = &trimmed[..end];
    let rest_start = end + 4; // skip "\n---"
    let rest = if rest_start < trimmed.len() {
        &trimmed[rest_start..]
    } else {
        ""
    };
    // Strip a single leading newline from the body if present.
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    Some((front, rest))
}

/// Parse YAML frontmatter (between `---` delimiters) into [`MemoryMetadata`].
///
/// Returns `None` when the content has no valid frontmatter or required fields
/// (`name`, `description`, `type`) are missing.
pub fn parse_frontmatter(content: &str) -> Option<MemoryMetadata> {
    let (yaml, _) = split_frontmatter(content)?;
    // serde_yml may return Err for missing required fields — map to None.
    serde_yml::from_str(yaml).ok()
}

/// Return the body text after the frontmatter delimiters.
///
/// If no frontmatter is found the entire content is returned unchanged.
pub fn extract_body(content: &str) -> &str {
    match split_frontmatter(content) {
        Some((_, body)) => body,
        None => content,
    }
}

/// Render [`MemoryMetadata`] as a YAML frontmatter block (including `---`
/// delimiters).
pub fn format_frontmatter(metadata: &MemoryMetadata) -> String {
    // Build YAML manually for deterministic ordering (serde_yml output order
    // is not guaranteed).
    let mut out = String::from("---\n");
    let _ = writeln!(out, "name: {}", metadata.name);
    let _ = writeln!(out, "description: {}", metadata.description);
    let _ = writeln!(out, "type: {}", metadata.memory_type);
    if let Some(ref ts) = metadata.created_at {
        let _ = writeln!(out, "created_at: {ts}");
    }
    if let Some(ref ts) = metadata.updated_at {
        let _ = writeln!(out, "updated_at: {ts}");
    }
    out.push_str("---\n");
    out
}

/// Format a memory entry for inclusion in a system prompt.
///
/// Produces: `[type] name: description\nbody`
pub fn format_memory_for_prompt(metadata: &MemoryMetadata, body: &str) -> String {
    let header = format!(
        "[{}] {}: {}",
        metadata.memory_type, metadata.name, metadata.description
    );
    if body.is_empty() {
        header
    } else {
        format!("{header}\n{body}")
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── MemoryType ──────────────────────────────────────────

    #[test]
    fn memory_type_display() {
        assert_eq!(MemoryType::User.to_string(), "user");
        assert_eq!(MemoryType::Feedback.to_string(), "feedback");
        assert_eq!(MemoryType::Project.to_string(), "project");
        assert_eq!(MemoryType::Reference.to_string(), "reference");
    }

    #[test]
    fn memory_type_from_str() {
        assert_eq!("user".parse::<MemoryType>().unwrap(), MemoryType::User);
        assert_eq!(
            "Feedback".parse::<MemoryType>().unwrap(),
            MemoryType::Feedback
        );
        assert_eq!(
            "PROJECT".parse::<MemoryType>().unwrap(),
            MemoryType::Project
        );
        assert_eq!(
            "Reference".parse::<MemoryType>().unwrap(),
            MemoryType::Reference
        );
        assert!("unknown".parse::<MemoryType>().is_err());
    }

    #[test]
    fn memory_type_serde_roundtrip() {
        let original = MemoryType::Feedback;
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"feedback\"");
        let parsed: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn memory_type_copy_semantics() {
        let a = MemoryType::User;
        let b = a; // copy
        assert_eq!(a, b);
    }

    // ── MemoryMetadata ──────────────────────────────────────

    #[test]
    fn metadata_serde_roundtrip() {
        let meta = MemoryMetadata {
            name: "test".into(),
            description: "A test memory".into(),
            memory_type: MemoryType::Project,
            created_at: Some("2025-01-01".into()),
            updated_at: Some("2025-06-01".into()),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: MemoryMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, meta);
    }

    #[test]
    fn metadata_skip_none_fields() {
        let meta = MemoryMetadata {
            name: "n".into(),
            description: "d".into(),
            memory_type: MemoryType::User,
            created_at: None,
            updated_at: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(!json.contains("created_at"));
        assert!(!json.contains("updated_at"));
    }

    // ── parse_frontmatter ───────────────────────────────────

    #[test]
    fn parse_frontmatter_valid() {
        let content = "---\nname: my-mem\ndescription: hello\ntype: user\n---\nbody text";
        let meta = parse_frontmatter(content).unwrap();
        assert_eq!(meta.name, "my-mem");
        assert_eq!(meta.description, "hello");
        assert_eq!(meta.memory_type, MemoryType::User);
        assert!(meta.created_at.is_none());
    }

    #[test]
    fn parse_frontmatter_with_timestamps() {
        let content = "---\nname: ts\ndescription: d\ntype: feedback\ncreated_at: 2025-01-01\nupdated_at: 2025-06-01\n---\n";
        let meta = parse_frontmatter(content).unwrap();
        assert_eq!(meta.created_at.as_deref(), Some("2025-01-01"));
        assert_eq!(meta.updated_at.as_deref(), Some("2025-06-01"));
    }

    #[test]
    fn parse_frontmatter_no_delimiters() {
        let content = "no frontmatter here";
        assert!(parse_frontmatter(content).is_none());
    }

    #[test]
    fn parse_frontmatter_missing_name() {
        let content = "---\ndescription: d\ntype: user\n---\n";
        assert!(parse_frontmatter(content).is_none());
    }

    #[test]
    fn parse_frontmatter_missing_type() {
        let content = "---\nname: n\ndescription: d\n---\n";
        assert!(parse_frontmatter(content).is_none());
    }

    #[test]
    fn parse_frontmatter_unknown_fields_ignored() {
        let content = "---\nname: n\ndescription: d\ntype: project\nextra: ignored\n---\n";
        let meta = parse_frontmatter(content).unwrap();
        assert_eq!(meta.name, "n");
    }

    #[test]
    fn parse_frontmatter_incomplete_returns_none() {
        // Opening delimiter but no closing one.
        let content = "---\nname: n\ndescription: d\ntype: user\n";
        assert!(parse_frontmatter(content).is_none());
    }

    // ── extract_body ────────────────────────────────────────

    #[test]
    fn extract_body_with_frontmatter() {
        let content = "---\nname: n\ndescription: d\ntype: user\n---\nthe body";
        assert_eq!(extract_body(content), "the body");
    }

    #[test]
    fn extract_body_no_frontmatter() {
        let content = "just plain text";
        assert_eq!(extract_body(content), "just plain text");
    }

    #[test]
    fn extract_body_empty_body() {
        let content = "---\nname: n\ndescription: d\ntype: user\n---\n";
        assert_eq!(extract_body(content), "");
    }

    // ── format_frontmatter ──────────────────────────────────

    #[test]
    fn format_frontmatter_roundtrip() {
        let meta = MemoryMetadata {
            name: "roundtrip".into(),
            description: "test".into(),
            memory_type: MemoryType::Reference,
            created_at: Some("2025-01-01".into()),
            updated_at: None,
        };
        let formatted = format_frontmatter(&meta);
        let parsed = parse_frontmatter(&formatted).unwrap();
        assert_eq!(parsed.name, meta.name);
        assert_eq!(parsed.description, meta.description);
        assert_eq!(parsed.memory_type, meta.memory_type);
        assert_eq!(parsed.created_at, meta.created_at);
        assert_eq!(parsed.updated_at, meta.updated_at);
    }

    // ── format_memory_for_prompt ─────────────────────────────

    #[test]
    fn format_memory_for_prompt_with_body() {
        let meta = MemoryMetadata {
            name: "test".into(),
            description: "desc".into(),
            memory_type: MemoryType::User,
            created_at: None,
            updated_at: None,
        };
        let result = format_memory_for_prompt(&meta, "my body");
        assert_eq!(result, "[user] test: desc\nmy body");
    }

    #[test]
    fn format_memory_for_prompt_empty_body() {
        let meta = MemoryMetadata {
            name: "test".into(),
            description: "desc".into(),
            memory_type: MemoryType::Feedback,
            created_at: None,
            updated_at: None,
        };
        let result = format_memory_for_prompt(&meta, "");
        assert_eq!(result, "[feedback] test: desc");
    }
}
