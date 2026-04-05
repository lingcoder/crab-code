//! System prompt construction for the agent.
//!
//! Assembles the system prompt from:
//! - CRAB.md project instructions
//! - Tool descriptions (from `ToolRegistry`)
//! - Environment info (OS, shell, cwd, git status)
//! - Current date/time

use std::fmt::Write;
use std::path::Path;

use crab_config::crab_md;
use crab_session::MemoryFile;
use crab_tools::registry::ToolRegistry;

/// Build the complete system prompt.
pub fn build_system_prompt(
    project_dir: &Path,
    registry: &ToolRegistry,
    custom_instructions: Option<&str>,
) -> String {
    build_system_prompt_with_memories(project_dir, registry, custom_instructions, &[])
}

/// Build the complete system prompt with memory context injected.
pub fn build_system_prompt_with_memories(
    project_dir: &Path,
    registry: &ToolRegistry,
    custom_instructions: Option<&str>,
    memories: &[MemoryFile],
) -> String {
    let mut prompt = String::with_capacity(4096);

    // Base instructions
    let _ = writeln!(
        prompt,
        "You are Crab Code, an AI coding assistant. \
        You help users with software engineering tasks using the tools available to you."
    );
    let _ = writeln!(prompt);

    // Environment info
    append_environment_info(&mut prompt);

    // Tool descriptions
    append_tool_descriptions(&mut prompt, registry);

    // CRAB.md instructions
    append_crab_md_instructions(&mut prompt, project_dir);

    // Memory context
    append_memory_context(&mut prompt, memories);

    // Custom instructions from settings
    if let Some(instructions) = custom_instructions
        && !instructions.is_empty()
    {
        let _ = writeln!(prompt, "# User Instructions\n");
        let _ = writeln!(prompt, "{instructions}");
        let _ = writeln!(prompt);
    }

    prompt
}

/// Append environment information to the prompt.
fn append_environment_info(prompt: &mut String) {
    let _ = writeln!(prompt, "# Environment\n");

    // OS
    let _ = writeln!(prompt, "- Platform: {}", std::env::consts::OS);
    let _ = writeln!(prompt, "- Architecture: {}", std::env::consts::ARCH);

    // Shell
    let shell = std::env::var("SHELL")
        .or_else(|_| std::env::var("COMSPEC"))
        .unwrap_or_else(|_| "unknown".to_string());
    let _ = writeln!(prompt, "- Shell: {shell}");

    // Working directory
    if let Ok(cwd) = std::env::current_dir() {
        let _ = writeln!(prompt, "- Working directory: {}", cwd.display());
    }

    // Date/time
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple date formatting (avoid chrono dependency)
    let secs = now.as_secs();
    let days = secs / 86400;
    // Approximate year/month/day from epoch days
    let (year, month, day) = epoch_days_to_ymd(days);
    let _ = writeln!(prompt, "- Date: {year:04}-{month:02}-{day:02}");

    let _ = writeln!(prompt);
}

/// Append tool descriptions for the model.
fn append_tool_descriptions(prompt: &mut String, registry: &ToolRegistry) {
    if registry.is_empty() {
        return;
    }

    let _ = writeln!(prompt, "# Available Tools\n");
    let _ = writeln!(
        prompt,
        "You have {} tools available. Use them to help the user.\n",
        registry.len()
    );
    for name in registry.tool_names() {
        if let Some(tool) = registry.get(name) {
            let _ = writeln!(prompt, "- **{}**: {}", tool.name(), tool.description());
        }
    }
    let _ = writeln!(prompt);
}

/// Append loaded memory files to the system prompt.
fn append_memory_context(prompt: &mut String, memories: &[MemoryFile]) {
    if memories.is_empty() {
        return;
    }

    let _ = writeln!(prompt, "# Loaded Memories\n");
    let _ = writeln!(
        prompt,
        "The following memories were loaded from previous sessions.\n"
    );
    for mem in memories {
        let _ = writeln!(prompt, "## {} (type: {})\n", mem.name, mem.memory_type);
        if !mem.description.is_empty() {
            let _ = writeln!(prompt, "> {}\n", mem.description);
        }
        let _ = writeln!(prompt, "{}\n", mem.body);
    }
}

/// Append CRAB.md project instructions.
fn append_crab_md_instructions(prompt: &mut String, project_dir: &Path) {
    let crab_mds = crab_md::collect_crab_md(project_dir);
    if crab_mds.is_empty() {
        return;
    }

    let _ = writeln!(prompt, "# Project Instructions (CRAB.md)\n");
    for md in &crab_mds {
        let source = match md.source {
            crab_md::CrabMdSource::Global => "global",
            crab_md::CrabMdSource::User => "user",
            crab_md::CrabMdSource::Project => "project",
        };
        let _ = writeln!(prompt, "<!-- source: {source} -->");
        let _ = writeln!(prompt, "{}", md.content);
        let _ = writeln!(prompt);
    }
}

/// Convert epoch days to (year, month, day).
/// Simple civil date calculation (no leap second handling).
fn epoch_days_to_ymd(epoch_days: u64) -> (u32, u32, u32) {
    // Algorithm from Howard Hinnant's date library (public domain)
    let z = epoch_days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    #[allow(clippy::cast_possible_truncation)]
    (y as u32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days_to_ymd_unix_epoch() {
        let (y, m, d) = epoch_days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn epoch_days_to_ymd_known_date() {
        // 2024-01-01 = 19723 days since epoch
        let (y, m, d) = epoch_days_to_ymd(19723);
        assert_eq!(y, 2024);
        assert_eq!(m, 1);
        assert_eq!(d, 1);
    }

    #[test]
    fn build_system_prompt_basic() {
        let registry = ToolRegistry::new();
        let prompt = build_system_prompt(Path::new("."), &registry, None);
        assert!(prompt.contains("Crab Code"));
        assert!(prompt.contains("Platform:"));
        assert!(prompt.contains("Date:"));
    }

    #[test]
    fn build_system_prompt_with_custom_instructions() {
        let registry = ToolRegistry::new();
        let prompt = build_system_prompt(
            Path::new("."),
            &registry,
            Some("Always respond in Chinese."),
        );
        assert!(prompt.contains("Always respond in Chinese."));
        assert!(prompt.contains("User Instructions"));
    }

    #[test]
    fn build_system_prompt_no_custom_instructions() {
        let registry = ToolRegistry::new();
        let prompt = build_system_prompt(Path::new("."), &registry, None);
        assert!(!prompt.contains("User Instructions"));
    }

    #[test]
    fn append_environment_info_has_platform() {
        let mut prompt = String::new();
        append_environment_info(&mut prompt);
        assert!(prompt.contains("Platform:"));
        assert!(prompt.contains("Shell:"));
    }

    #[test]
    fn build_with_memories_includes_memory_section() {
        let registry = ToolRegistry::new();
        let memories = vec![
            MemoryFile {
                name: "User role".into(),
                description: "Senior Rust developer".into(),
                memory_type: "user".into(),
                body: "The user is a senior Rust developer.".into(),
                filename: "user_role.md".into(),
            },
            MemoryFile {
                name: "No mocks".into(),
                description: "Use real DB in tests".into(),
                memory_type: "feedback".into(),
                body: "Always use real database connections in integration tests.".into(),
                filename: "feedback_testing.md".into(),
            },
        ];
        let prompt = build_system_prompt_with_memories(Path::new("."), &registry, None, &memories);
        assert!(prompt.contains("Loaded Memories"));
        assert!(prompt.contains("User role"));
        assert!(prompt.contains("type: user"));
        assert!(prompt.contains("Senior Rust developer"));
        assert!(prompt.contains("No mocks"));
        assert!(prompt.contains("type: feedback"));
        assert!(prompt.contains("real database"));
    }

    #[test]
    fn build_with_no_memories_omits_section() {
        let registry = ToolRegistry::new();
        let prompt = build_system_prompt_with_memories(Path::new("."), &registry, None, &[]);
        assert!(!prompt.contains("Loaded Memories"));
    }

    #[test]
    fn append_memory_context_empty() {
        let mut prompt = String::new();
        append_memory_context(&mut prompt, &[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn append_memory_context_formats_entries() {
        let mut prompt = String::new();
        let memories = vec![MemoryFile {
            name: "Test".into(),
            description: "A test memory".into(),
            memory_type: "project".into(),
            body: "Some project context.".into(),
            filename: "test.md".into(),
        }];
        append_memory_context(&mut prompt, &memories);
        assert!(prompt.contains("## Test (type: project)"));
        assert!(prompt.contains("> A test memory"));
        assert!(prompt.contains("Some project context."));
    }
}
