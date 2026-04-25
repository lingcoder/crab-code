//! Plugin layer loader.
//!
//! Scans `$CRAB_CONFIG_DIR/plugins/<name>/config.json`, gates each plugin by
//! the user-level `enabledPlugins` map, parses every enabled plugin's JSON
//! contribution, converts it to `toml::Value`, and returns the list in
//! alphabetical order of `<name>` so the upstream merge chain can fold each
//! contribution in deterministically.
//!
//! Aligned with `docs/config.md` §2 (plugin layer notes) and §10.1
//! (graceful degradation: skip-and-warn on individual plugin parse errors).
//!
//! Security constraint: a plugin's `config.json` MUST NOT set the top-level
//! `env` field. Allowing it would let plugins silently inject secrets or
//! proxies into Crab and child processes. Such files are skipped with a
//! warning instead of being merged.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::config::{EnabledPluginValue, config_file_name};
use crate::loader::ResolveContext;

/// Scan the plugin directory and return enabled plugin contributions.
///
/// Returns every enabled plugin's `config.json` as a `toml::Value`, sorted
/// by plugin name. Missing directories yield an empty list. Individual plugin
/// failures are logged to stderr and skipped — the overall resolve never
/// fails because of a single bad plugin.
pub fn load_enabled_plugin_configs(ctx: &ResolveContext) -> crab_core::Result<Vec<Value>> {
    let plugins_dir = ctx.config_dir.join("plugins");
    let user_config = ctx.config_dir.join(config_file_name());
    let enabled = peek_enabled_plugins(&user_config);

    let mut plugin_names = match list_plugin_dirs(&plugins_dir) {
        Ok(names) => names,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(crab_core::Error::Config(format!(
                "failed to read plugins dir {}: {e}",
                plugins_dir.display()
            )));
        }
    };
    plugin_names.sort();

    let mut configs = Vec::new();
    for name in plugin_names {
        if !is_enabled(enabled.as_ref(), &name) {
            continue;
        }
        let cfg_path = plugins_dir.join(&name).join("config.json");
        match load_plugin_config_as_toml(&cfg_path) {
            Ok(Some(value)) => configs.push(value),
            Ok(None) => {}
            Err(e) => {
                eprintln!("[config] warning: plugin '{name}' config.json invalid: {e}; skipping");
            }
        }
    }
    Ok(configs)
}

/// Peek `enabledPlugins` directly from the user-level `config.toml` without
/// running the full resolve pipeline (which would call back into us — circular).
/// A missing or unparseable file yields `None`, which downstream treats as
/// "no plugins enabled".
fn peek_enabled_plugins(path: &Path) -> Option<HashMap<String, EnabledPluginValue>> {
    #[derive(Deserialize)]
    struct Peek {
        #[serde(rename = "enabledPlugins")]
        enabled_plugins: Option<HashMap<String, EnabledPluginValue>>,
    }

    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str::<Peek>(&text).ok()?.enabled_plugins
}

/// Return the list of immediate subdirectory names under `plugins_dir`.
/// Surfaces the IO error so the caller can distinguish `NotFound` (no plugins
/// installed → empty contribution list) from a real read failure.
fn list_plugin_dirs(plugins_dir: &Path) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(plugins_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

/// A plugin is considered enabled when its name appears in the map with an
/// enabling value. With no map at all, no plugins are enabled (opt-in model).
fn is_enabled(enabled: Option<&HashMap<String, EnabledPluginValue>>, name: &str) -> bool {
    enabled
        .and_then(|map| map.get(name))
        .is_some_and(EnabledPluginValue::is_enabled)
}

/// Read a plugin's `config.json`, parse it as JSON, enforce the security
/// constraints, and convert the result into a `toml::Value` ready to join the
/// merge chain. `Ok(None)` means "no contribution" (file simply absent).
fn load_plugin_config_as_toml(path: &Path) -> crab_core::Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(crab_core::Error::Config(format!(
                "failed to read {}: {e}",
                path.display()
            )));
        }
    };

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| crab_core::Error::Config(format!("JSON parse error: {e}")))?;

    reject_forbidden_keys(&json)?;
    Ok(Some(serde_json_to_toml_value(json)))
}

/// Reject plugin contributions that try to set fields the security model
/// disallows. Currently only `env` is forbidden — plugins must not be able
/// to silently inject environment variables (and therefore secrets/proxies)
/// into Crab or its child processes.
fn reject_forbidden_keys(json: &serde_json::Value) -> crab_core::Result<()> {
    if let serde_json::Value::Object(map) = json
        && map.contains_key("env")
    {
        return Err(crab_core::Error::Config(
            "plugins are not allowed to set the `env` field".into(),
        ));
    }
    Ok(())
}

/// Convert `serde_json::Value` to `toml::Value`. JSON `null` has no TOML
/// counterpart and is dropped (object entries omitted, array elements
/// removed) — overlay `null` should not clear a base value.
fn serde_json_to_toml_value(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Table(toml::value::Table::new()),
        serde_json::Value::Bool(b) => Value::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(arr) => Value::Array(
            arr.into_iter()
                .filter(|v| !v.is_null())
                .map(serde_json_to_toml_value)
                .collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut table = toml::value::Table::new();
            for (k, v) in map {
                if v.is_null() {
                    continue;
                }
                table.insert(k, serde_json_to_toml_value(v));
            }
            Value::Table(table)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_env_field_rejected() {
        let json = serde_json::json!({ "env": { "FOO": "bar" } });
        let err = reject_forbidden_keys(&json).unwrap_err();
        assert!(err.to_string().contains("env"));
    }

    #[test]
    fn forbidden_check_passes_without_env() {
        let json = serde_json::json!({ "permissions": { "allow": ["Bash"] } });
        reject_forbidden_keys(&json).unwrap();
    }

    #[test]
    fn json_to_toml_drops_nulls() {
        let json = serde_json::json!({
            "model": "haiku",
            "missing": null,
            "list": [1, null, 2],
        });
        let toml_value = serde_json_to_toml_value(json);
        let table = toml_value.as_table().unwrap();
        assert!(!table.contains_key("missing"));
        assert_eq!(table.get("model").unwrap().as_str(), Some("haiku"));
        let list = table.get("list").unwrap().as_array().unwrap();
        let ints: Vec<i64> = list.iter().filter_map(|v| v.as_integer()).collect();
        assert_eq!(ints, vec![1, 2]);
    }

    #[test]
    fn is_enabled_handles_all_value_kinds() {
        let mut map: HashMap<String, EnabledPluginValue> = HashMap::new();
        map.insert("on".into(), EnabledPluginValue::Bool(true));
        map.insert("off".into(), EnabledPluginValue::Bool(false));
        map.insert(
            "constrained".into(),
            EnabledPluginValue::VersionConstraints(vec![">=1.0".into()]),
        );
        let some = Some(map);
        assert!(is_enabled(some.as_ref(), "on"));
        assert!(!is_enabled(some.as_ref(), "off"));
        assert!(is_enabled(some.as_ref(), "constrained"));
        assert!(!is_enabled(some.as_ref(), "missing"));
        assert!(!is_enabled(None, "anything"));
    }
}
