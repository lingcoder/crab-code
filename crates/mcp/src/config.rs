use std::path::PathBuf;

use serde_json::Value;

/// Load MCP server configurations from one or more JSON files and merge them.
///
/// Each file must be a JSON object. Keys across files are merged (later files
/// win on duplicate keys).
pub fn load_mcp_configs(paths: &[PathBuf]) -> anyhow::Result<Value> {
    let mut merged = serde_json::Map::new();
    for path in paths {
        let content = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!("failed to read MCP config '{}': {}", path.display(), e)
        })?;
        let parsed: Value = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("failed to parse MCP config '{}': {}", path.display(), e)
        })?;
        if let Value::Object(map) = parsed {
            for (k, v) in map {
                merged.insert(k, v);
            }
        } else {
            anyhow::bail!("MCP config '{}' must be a JSON object", path.display());
        }
    }
    Ok(Value::Object(merged))
}

/// Load the project-shared `.mcp.json` from a project root, if present.
///
/// The file uses the CC ecosystem shape — an `mcpServers` object keyed by
/// server name:
///
/// ```json
/// { "mcpServers": { "fs": { "command": "fs-mcp", "args": [] } } }
/// ```
///
/// Returns the inner `mcpServers` object, `None` when the file is absent,
/// and an error when it exists but is malformed.
pub fn load_project_mcp_json(project_dir: &std::path::Path) -> anyhow::Result<Option<Value>> {
    let path = project_dir.join(".mcp.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "failed to read '{}': {}",
                path.display(),
                e
            ));
        }
    };
    let parsed: Value = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse '{}': {}", path.display(), e))?;
    let servers = parsed.get("mcpServers").cloned().ok_or_else(|| {
        anyhow::anyhow!("'{}' is missing the \"mcpServers\" object", path.display())
    })?;
    if !servers.is_object() {
        anyhow::bail!("'{}': \"mcpServers\" must be a JSON object", path.display());
    }
    Ok(Some(servers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn load_mcp_configs_empty_paths() {
        let result = load_mcp_configs(&[]).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn load_mcp_configs_rejects_missing_file() {
        assert!(load_mcp_configs(&[PathBuf::from("/nonexistent.json")]).is_err());
    }

    #[test]
    fn project_mcp_json_absent_is_none() {
        let dir = std::env::temp_dir().join("crab_mcp_json_absent");
        let _ = std::fs::create_dir_all(&dir);
        assert!(load_project_mcp_json(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_mcp_json_parses_cc_shape() {
        let dir = std::env::temp_dir().join("crab_mcp_json_ok");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers": {"fs": {"command": "fs-mcp"}}}"#,
        )
        .unwrap();
        let servers = load_project_mcp_json(&dir).unwrap().unwrap();
        assert_eq!(servers["fs"]["command"], "fs-mcp");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_mcp_json_missing_key_errors() {
        let dir = std::env::temp_dir().join("crab_mcp_json_bad");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(".mcp.json"), r#"{"servers": {}}"#).unwrap();
        assert!(load_project_mcp_json(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
