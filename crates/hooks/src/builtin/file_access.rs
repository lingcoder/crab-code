//! Built-in file access tracking hook.
//!
//! Monitors PostToolUse events from file-related tools (Read, Edit, Write,
//! Grep, Glob) and records which files the agent accessed during a session.

use std::path::PathBuf;

use crate::registry::{HookEventType, HookSource, RegisteredHook};
use crate::types::{CommandHook, HookType};

/// What kind of file access occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAccessType {
    Read,
    Write,
    Edit,
    Search,
}

/// A single file access event recorded by the tracking hook.
#[derive(Debug, Clone)]
pub struct FileAccessRecord {
    pub tool_name: String,
    pub file_path: PathBuf,
    pub access_type: FileAccessType,
    pub timestamp: std::time::Instant,
}

/// Tool names that trigger file access tracking.
const FILE_TOOLS: &[(&str, FileAccessType)] = &[
    ("read", FileAccessType::Read),
    ("grep", FileAccessType::Search),
    ("glob", FileAccessType::Search),
    ("edit", FileAccessType::Edit),
    ("write", FileAccessType::Write),
];

/// Returns the file access type for a tool name, if it's a tracked tool.
#[must_use]
pub fn classify_tool(tool_name: &str) -> Option<FileAccessType> {
    let lower = tool_name.to_lowercase();
    FILE_TOOLS
        .iter()
        .find(|(name, _)| lower.contains(name))
        .map(|(_, kind)| *kind)
}

/// Extract file path from a PostToolUse event's output JSON.
///
/// Looks for common patterns in tool output: `file_path`, `path`, or
/// `filePath` keys.
#[must_use]
pub fn extract_file_path(output: &serde_json::Value) -> Option<PathBuf> {
    output
        .get("file_path")
        .or_else(|| output.get("path"))
        .or_else(|| output.get("filePath"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
}

/// Create the built-in file access tracking hook registration.
///
/// The returned hook listens for `PostToolUse` events. The actual tracking
/// logic runs in the agent layer which calls [`classify_tool`] and
/// [`extract_file_path`] to build [`FileAccessRecord`]s.
#[must_use]
pub fn file_access_hook() -> RegisteredHook {
    RegisteredHook {
        id: "builtin:file_access".into(),
        event_filter: vec![HookEventType::PostToolUse],
        hook_type: HookType::Command(CommandHook {
            command: String::new(),
            timeout_secs: 0,
        }),
        source: HookSource::Builtin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_known_tools() {
        assert_eq!(classify_tool("Read"), Some(FileAccessType::Read));
        assert_eq!(classify_tool("read"), Some(FileAccessType::Read));
        assert_eq!(classify_tool("Grep"), Some(FileAccessType::Search));
        assert_eq!(classify_tool("Glob"), Some(FileAccessType::Search));
        assert_eq!(classify_tool("Edit"), Some(FileAccessType::Edit));
        assert_eq!(classify_tool("Write"), Some(FileAccessType::Write));
    }

    #[test]
    fn classify_unknown_tool() {
        assert_eq!(classify_tool("bash"), None);
        assert_eq!(classify_tool("unknown"), None);
    }

    #[test]
    fn extract_file_path_from_output() {
        let output = serde_json::json!({"file_path": "/tmp/test.rs"});
        assert_eq!(
            extract_file_path(&output),
            Some(PathBuf::from("/tmp/test.rs"))
        );

        let output = serde_json::json!({"path": "/tmp/other.rs"});
        assert_eq!(
            extract_file_path(&output),
            Some(PathBuf::from("/tmp/other.rs"))
        );

        let output = serde_json::json!({"filePath": "/tmp/camel.rs"});
        assert_eq!(
            extract_file_path(&output),
            Some(PathBuf::from("/tmp/camel.rs"))
        );
    }

    #[test]
    fn extract_file_path_missing() {
        let output = serde_json::json!({"result": "ok"});
        assert_eq!(extract_file_path(&output), None);
    }

    #[test]
    fn file_access_hook_registration() {
        let hook = file_access_hook();
        assert_eq!(hook.id, "builtin:file_access");
        assert_eq!(hook.source, HookSource::Builtin);
        assert_eq!(hook.event_filter, vec![HookEventType::PostToolUse]);
    }
}
