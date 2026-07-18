use std::path::Path;

/// List configured agent definitions from .crab/agents/ directory.
pub fn run() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().unwrap_or_default();

    // Check project-level agents directory
    let project_agents = working_dir.join(".crab").join("agents");
    // Check global agents directory
    let global_agents = crab_config::config::global_config_dir().join("agents");

    let mut found_any = false;

    if project_agents.exists() {
        let agents = list_agents(&project_agents)?;
        if !agents.is_empty() {
            found_any = true;
            eprintln!("Project agents ({}):", project_agents.display());
            for agent in &agents {
                eprintln!("  {agent}");
            }
        }
    }

    if global_agents.exists() {
        let agents = list_agents(&global_agents)?;
        if !agents.is_empty() {
            if found_any {
                eprintln!();
            }
            found_any = true;
            eprintln!("Global agents ({}):", global_agents.display());
            for agent in &agents {
                eprintln!("  {agent}");
            }
        }
    }

    if !found_any {
        eprintln!("No agent definitions found.");
        eprintln!();
        eprintln!("To create an agent, add a Markdown file with frontmatter to:");
        eprintln!("  .crab/agents/         (project-level)");
        eprintln!("  {}/  (global)", global_agents.display());
        eprintln!("  .claude/agents/       (Claude Code compatibility)");
        eprintln!();
        eprintln!("Example agent definition (reviewer.md):");
        eprintln!("  ---");
        eprintln!("  name: reviewer");
        eprintln!("  description: Reviews code for bugs");
        eprintln!("  tools: Read, Grep");
        eprintln!("  ---");
        eprintln!("  You are a code reviewer. Find bugs.");
    }

    Ok(())
}

fn list_agents(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut agents = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(content) => agents.push(describe_agent(&filename, &content)),
            Err(e) => agents.push(format!("{filename} — read error: {e}")),
        }
    }
    agents.sort();
    Ok(agents)
}

/// Summarize one agent Markdown file from its frontmatter.
fn describe_agent(filename: &str, content: &str) -> String {
    let stem = filename.strip_suffix(".md").unwrap_or(filename);
    let Ok((frontmatter, _)) = crab_skills::frontmatter::split_frontmatter(content) else {
        return format!("{stem} — missing frontmatter");
    };
    let yaml = crab_skills::frontmatter::parse_simple_yaml(&frontmatter);
    let name = yaml
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(stem);
    let desc = yaml
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tools = yaml.get("tools").and_then(|v| v.as_str());
    let mut line = if desc.is_empty() {
        name.to_string()
    } else {
        format!("{name} — {desc}")
    };
    if let Some(tools) = tools.filter(|s| !s.is_empty()) {
        use std::fmt::Write as _;
        let _ = write!(line, " (tools: {tools})");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_agents_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert!(agents.is_empty());
    }

    #[test]
    fn list_agents_with_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("reviewer.md"),
            "---
name: reviewer
description: Reviews code
tools: Read, Grep
---
You review code.",
        )
        .unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].contains("reviewer"));
        assert!(agents[0].contains("Reviews code"));
        assert!(agents[0].contains("Read, Grep"));
    }

    #[test]
    fn list_agents_name_falls_back_to_stem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("helper.md"),
            "---
description: A helper
---
Help.",
        )
        .unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].contains("helper"));
    }

    #[test]
    fn list_agents_skips_non_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not an agent").unwrap();
        std::fs::write(
            dir.path().join("a.md"),
            "---
name: a
---
Agent.",
        )
        .unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn list_agents_missing_frontmatter_handled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.md"), "no frontmatter here").unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].contains("missing frontmatter"));
    }

    #[test]
    fn run_doesnt_panic() {
        // Should not panic even if no agents exist
        let result = run();
        assert!(result.is_ok());
    }
}
