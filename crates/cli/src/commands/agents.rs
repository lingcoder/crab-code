use std::path::Path;

/// List configured agent definitions from .crab/agents/ directory.
pub fn run() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().unwrap_or_default();

    // Check project-level agents directory
    let project_agents = working_dir.join(".crab").join("agents");
    // Check global agents directory
    let global_agents = crab_config::settings::global_config_dir().join("agents");

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
        eprintln!("To create an agent, add a JSON file to:");
        eprintln!("  .crab/agents/         (project-level)");
        eprintln!("  {}/  (global)", global_agents.display());
        eprintln!();
        eprintln!("Example agent definition (my-agent.json):");
        eprintln!("  {{");
        eprintln!("    \"name\": \"my-agent\",");
        eprintln!("    \"description\": \"A custom agent\",");
        eprintln!("    \"model\": \"claude-sonnet-4-latest\"");
        eprintln!("  }}");
    }

    Ok(())
}

fn list_agents(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut agents = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            match std::fs::read_to_string(&path) {
                Ok(content) => if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    let name = val
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<unnamed>");
                    let desc = val
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let model = val
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    if desc.is_empty() {
                        agents.push(format!("{name} (model: {model})"));
                    } else {
                        agents.push(format!("{name} — {desc} (model: {model})"));
                    }
                } else {
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    agents.push(format!("{filename} — invalid JSON"));
                },
                Err(e) => {
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    agents.push(format!("{filename} — read error: {e}"));
                }
            }
        }
    }
    agents.sort();
    Ok(agents)
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
    fn list_agents_with_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test-agent.json"),
            r#"{"name": "test", "description": "A test agent", "model": "gpt-4"}"#,
        )
        .unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].contains("test"));
        assert!(agents[0].contains("A test agent"));
        assert!(agents[0].contains("gpt-4"));
    }

    #[test]
    fn list_agents_with_minimal_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("minimal.json"), r#"{"name": "min"}"#).unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].contains("min"));
        assert!(agents[0].contains("default")); // default model
    }

    #[test]
    fn list_agents_skips_non_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# Agents").unwrap();
        std::fs::write(dir.path().join("agent.json"), r#"{"name": "ok"}"#).unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
    }

    #[test]
    fn list_agents_invalid_json_handled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.json"), "not json!").unwrap();
        let agents = list_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].contains("invalid JSON"));
    }

    #[test]
    fn run_doesnt_panic() {
        // Should not panic even if no agents exist
        let result = run();
        assert!(result.is_ok());
    }
}
