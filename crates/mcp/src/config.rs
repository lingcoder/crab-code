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
}
