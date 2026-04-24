//! System prompt construction for the agent.
//!
//! Assembles the system prompt from:
//! - AGENTS.md project instructions
//! - Tool descriptions (from `ToolRegistry`)
//! - Environment info (OS, shell, cwd, git status)
//! - Current date/time

use std::fmt::Write;
use std::path::Path;

use crab_config::agents_md;
use crab_memory::MemoryFile;
use crab_tools::registry::ToolRegistry;

use super::git_context::GitContext;

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

    // Git context
    append_git_context(&mut prompt, project_dir);

    // Tool descriptions
    append_tool_descriptions(&mut prompt, registry);

    // AGENTS.md instructions
    append_agents_md_instructions(&mut prompt, project_dir);

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

    // Shell. Mirrors CCB's getShellInfoLine: read $SHELL only (never COMSPEC —
    // that's cmd.exe, which would push the model toward Windows syntax), and
    // on Windows append explicit Unix-syntax guidance since our Bash tool runs
    // bash/zsh (Git Bash).
    let shell_raw = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let shell_name = if shell_raw.contains("zsh") {
        "zsh"
    } else if shell_raw.contains("bash") {
        "bash"
    } else {
        shell_raw.as_str()
    };
    if cfg!(windows) {
        let _ = writeln!(
            prompt,
            "- Shell: {shell_name} (use Unix shell syntax, not Windows — \
             e.g., /dev/null not NUL, forward slashes in paths)"
        );
    } else {
        let _ = writeln!(prompt, "- Shell: {shell_name}");
    }

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
        let _ = writeln!(
            prompt,
            "## {} (type: {})\n",
            mem.metadata.name, mem.metadata.memory_type
        );
        if !mem.metadata.description.is_empty() {
            let _ = writeln!(prompt, "> {}\n", mem.metadata.description);
        }
        let _ = writeln!(prompt, "{}\n", mem.body);
    }
}

/// Append AGENTS.md project instructions.
fn append_agents_md_instructions(prompt: &mut String, project_dir: &Path) {
    let agents_mds = agents_md::collect_agents_md(project_dir);
    if agents_mds.is_empty() {
        return;
    }

    let _ = writeln!(prompt, "# Project Instructions (AGENTS.md)\n");
    for md in &agents_mds {
        let source = match md.source {
            agents_md::AgentsMdSource::Global => "global",
            agents_md::AgentsMdSource::User => "user",
            agents_md::AgentsMdSource::Project => "project",
        };
        let _ = writeln!(prompt, "<!-- source: {source} -->");
        let _ = writeln!(prompt, "{}", md.content);
        let _ = writeln!(prompt);
    }
}

/// Append git context to the prompt if the project directory is a git repository.
fn append_git_context(prompt: &mut String, project_dir: &Path) {
    if let Some(ctx) = GitContext::collect(project_dir) {
        let _ = writeln!(prompt, "# Git Context\n");
        let _ = write!(prompt, "{}", ctx.format());
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
    #[cfg(windows)]
    fn windows_shell_line_appends_unix_syntax_hint() {
        let mut prompt = String::new();
        append_environment_info(&mut prompt);
        assert!(
            prompt.contains("use Unix shell syntax"),
            "Windows prompt should tell the model to use Unix syntax, got: {prompt}"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn non_windows_shell_line_has_no_unix_hint() {
        let mut prompt = String::new();
        append_environment_info(&mut prompt);
        assert!(!prompt.contains("use Unix shell syntax"));
    }

    #[test]
    fn build_with_memories_includes_memory_section() {
        use crab_memory::{MemoryMetadata, MemoryType};
        use std::path::PathBuf;

        let registry = ToolRegistry::new();
        let memories = vec![
            MemoryFile {
                filename: "user_role.md".into(),
                path: PathBuf::from("user_role.md"),
                metadata: MemoryMetadata {
                    name: "User role".into(),
                    description: "Senior Rust developer".into(),
                    memory_type: MemoryType::User,
                    created_at: None,
                    updated_at: None,
                },
                body: "The user is a senior Rust developer.".into(),
                mtime: None,
            },
            MemoryFile {
                filename: "feedback_testing.md".into(),
                path: PathBuf::from("feedback_testing.md"),
                metadata: MemoryMetadata {
                    name: "No mocks".into(),
                    description: "Use real DB in tests".into(),
                    memory_type: MemoryType::Feedback,
                    created_at: None,
                    updated_at: None,
                },
                body: "Always use real database connections in integration tests.".into(),
                mtime: None,
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
        use crab_memory::{MemoryMetadata, MemoryType};
        use std::path::PathBuf;

        let mut prompt = String::new();
        let memories = vec![MemoryFile {
            filename: "test.md".into(),
            path: PathBuf::from("test.md"),
            metadata: MemoryMetadata {
                name: "Test".into(),
                description: "A test memory".into(),
                memory_type: MemoryType::Project,
                created_at: None,
                updated_at: None,
            },
            body: "Some project context.".into(),
            mtime: None,
        }];
        append_memory_context(&mut prompt, &memories);
        assert!(prompt.contains("## Test (type: project)"));
        assert!(prompt.contains("> A test memory"));
        assert!(prompt.contains("Some project context."));
    }

    #[test]
    fn epoch_days_to_ymd_leap_year() {
        // 2000-03-01 = 11017 days since epoch
        let (y, m, d) = epoch_days_to_ymd(11017);
        assert_eq!(y, 2000);
        assert_eq!(m, 3);
        assert_eq!(d, 1);
    }

    #[test]
    fn epoch_days_to_ymd_end_of_year() {
        // 2023-12-31 = 19722 days since epoch
        let (y, m, d) = epoch_days_to_ymd(19722);
        assert_eq!(y, 2023);
        assert_eq!(m, 12);
        assert_eq!(d, 31);
    }

    #[test]
    fn build_system_prompt_with_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "# Custom Project Rules\nBe helpful.",
        )
        .unwrap();

        let registry = ToolRegistry::new();
        let prompt = build_system_prompt(dir.path(), &registry, None);
        assert!(prompt.contains("Custom Project Rules"));
        assert!(prompt.contains("Be helpful."));
        assert!(prompt.contains("Project Instructions"));
    }

    #[test]
    fn build_system_prompt_empty_custom_instructions_omitted() {
        let registry = ToolRegistry::new();
        let prompt = build_system_prompt(Path::new("."), &registry, Some(""));
        assert!(!prompt.contains("User Instructions"));
    }

    #[test]
    fn append_memory_context_multiple() {
        use crab_memory::{MemoryMetadata, MemoryType};
        use std::path::PathBuf;

        let mut prompt = String::new();
        let memories = vec![
            MemoryFile {
                filename: "a.md".into(),
                path: PathBuf::from("a.md"),
                metadata: MemoryMetadata {
                    name: "A".into(),
                    description: "da".into(),
                    memory_type: MemoryType::User,
                    created_at: None,
                    updated_at: None,
                },
                body: "ba".into(),
                mtime: None,
            },
            MemoryFile {
                filename: "b.md".into(),
                path: PathBuf::from("b.md"),
                metadata: MemoryMetadata {
                    name: "B".into(),
                    description: "db".into(),
                    memory_type: MemoryType::Feedback,
                    created_at: None,
                    updated_at: None,
                },
                body: "bb".into(),
                mtime: None,
            },
        ];
        append_memory_context(&mut prompt, &memories);
        assert!(prompt.contains("## A (type: user)"));
        assert!(prompt.contains("## B (type: feedback)"));
    }
}
