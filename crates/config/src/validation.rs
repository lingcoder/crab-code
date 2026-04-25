//! Config schema validation backed by `jsonschema` (Draft 2020-12).
//!
//! The schema lives at `assets/config.schema.json` and is embedded into the
//! binary at compile time. All validation calls compile the schema lazily
//! and reuse a single `Validator` instance for the lifetime of the process,
//! so per-call overhead is just a single `iter_errors` traversal.
//!
//! # Public surface
//!
//! - [`ValidationError`] — owned, `Display`-friendly diagnostic carrying a
//!   JSON Pointer path and a human-readable message.
//! - [`validate_config_value`] — the primary entry point; takes a parsed
//!   `toml::Value` and returns every violation as a `Vec`. Empty vec ⇒ valid.
//! - [`validate_config`] — the same check applied to a `serde_json::Value`,
//!   kept for callers that have already converted.
//! - [`validate_all_config_files`] / [`validate_raw_file`] — convenience
//!   wrappers used by the CLI/TUI to surface per-file warnings.
//! - [`prune_invalid_field`] — graceful-degradation helper. Walks a JSON
//!   Pointer path and removes the offending leaf so resolve can keep the
//!   surrounding fields. Used by [`crate::loader::resolve`].

use std::fmt;
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value as JsonValue;

/// Embedded schema, included via `include_str!` so the binary always has it.
const SCHEMA_SRC: &str = include_str!("../assets/config.schema.json");

/// Lazily compiled `Validator`. Compilation is fallible (the schema is
/// hand-written and could in principle have a typo); on failure we fall
/// back to "no validation" so a broken developer-side schema never blocks
/// production users.
fn validator() -> Option<&'static Validator> {
    static CELL: OnceLock<Option<Validator>> = OnceLock::new();
    CELL.get_or_init(|| {
        let schema: JsonValue = serde_json::from_str(SCHEMA_SRC).ok()?;
        jsonschema::validator_for(&schema).ok()
    })
    .as_ref()
}

/// A single validation error with the offending field's JSON Pointer path.
///
/// `field` carries either a JSON Pointer (`"/permissions/allow/0"`) or a
/// dotted path (`permissions.allow[0]`) — both forms are accepted by the
/// rest of the codebase. `Display` renders `field: message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// JSON Pointer path of the offending leaf, e.g. `/permissions/allow/0`.
    /// Empty string for whole-document errors.
    pub field: String,
    /// Human-readable error message, taken from the schema's `Display` impl.
    pub message: String,
    /// Optional remediation suggestion (kept for back-compat with callers
    /// that pattern-match on the field).
    pub suggestion: Option<String>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = if self.field.is_empty() {
            "<root>"
        } else {
            self.field.as_str()
        };
        write!(f, "{label}: {}", self.message)?;
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " (suggestion: {suggestion})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// Validate a parsed TOML value against the embedded schema.
///
/// Returns every violation. An empty `Vec` means the value is valid.
/// Each error's `field` is a JSON Pointer string suitable for passing to
/// [`prune_invalid_field`].
#[must_use]
pub fn validate_config_value(value: &toml::Value) -> Vec<ValidationError> {
    let json = crate::config::toml_value_to_json_for_validation(value.clone());
    validate_config(&json)
}

/// Validate a JSON value against the embedded schema.
///
/// Mirrors [`validate_config_value`] for callers that already have a
/// `serde_json::Value` in hand (e.g. plugin loader, CLI `config show`).
#[must_use]
pub fn validate_config(value: &JsonValue) -> Vec<ValidationError> {
    let Some(validator) = validator() else {
        // Schema itself failed to compile — degrade silently so a broken
        // dev-side schema doesn't take down production users. The CI test
        // `schema_compiles` catches this scenario.
        return Vec::new();
    };
    validator
        .iter_errors(value)
        .map(|e| ValidationError {
            field: e.instance_path().to_string(),
            message: e.to_string(),
            suggestion: None,
        })
        .collect()
}

/// Validate a raw config file from disk.
///
/// Reads the file, parses TOML, and validates the resulting object.
/// Each error's `field` is prefixed with the source label (e.g. `[global]`).
/// Returns an empty `Vec` if the file does not exist or is empty.
#[must_use]
pub fn validate_raw_file(path: &std::path::Path, source_label: &str) -> Vec<ValidationError> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            return vec![ValidationError {
                field: format!("[{source_label}]"),
                message: format!("cannot read file: {e}"),
                suggestion: None,
            }];
        }
    };

    let content = content.trim();
    if content.is_empty() {
        return Vec::new();
    }

    let toml_value: toml::Value = match toml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            return vec![ValidationError {
                field: format!("[{source_label}]"),
                message: format!("parse error: {e}"),
                suggestion: None,
            }];
        }
    };

    validate_config_value(&toml_value)
        .into_iter()
        .map(|mut e| {
            e.field = format!("[{source_label}] {}", e.field);
            e
        })
        .collect()
}

/// Validate every config file in the merge chain.
///
/// Each raw file is validated independently so that absent fields never
/// appear as false-positive errors. Returns warnings with source-prefixed
/// field paths.
#[must_use]
pub fn validate_all_config_files(project_dir: Option<&std::path::Path>) -> Vec<ValidationError> {
    let mut warnings = Vec::new();

    let global_path = crate::config::global_config_dir().join(crate::config::config_file_name());
    warnings.extend(validate_raw_file(&global_path, "global"));

    if let Some(dir) = project_dir {
        let project_path =
            crate::config::project_config_dir(dir).join(crate::config::config_file_name());
        warnings.extend(validate_raw_file(&project_path, "project"));

        let local_path =
            crate::config::project_config_dir(dir).join(crate::config::local_config_file_name());
        warnings.extend(validate_raw_file(&local_path, "local"));
    }

    warnings
}

/// Remove the leaf identified by a JSON Pointer path from `value`.
///
/// Used by [`crate::loader::resolve`] to graceful-degrade: when a field
/// fails schema validation, the loader strips just that field and keeps
/// the rest of the merged config. Returns `true` if a leaf was removed.
///
/// Empty paths and paths that no longer exist are no-ops.
pub fn prune_invalid_field(value: &mut toml::Value, json_pointer: &str) -> bool {
    if json_pointer.is_empty() || json_pointer == "/" {
        return false;
    }
    let segments: Vec<String> = json_pointer
        .strip_prefix('/')
        .unwrap_or(json_pointer)
        .split('/')
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    if segments.is_empty() {
        return false;
    }

    let (last, parents) = segments.split_last().expect("non-empty after check");
    let mut cursor: &mut toml::Value = value;
    for seg in parents {
        match cursor {
            toml::Value::Table(t) => match t.get_mut(seg) {
                Some(child) => cursor = child,
                None => return false,
            },
            toml::Value::Array(a) => {
                let Ok(idx) = seg.parse::<usize>() else {
                    return false;
                };
                match a.get_mut(idx) {
                    Some(child) => cursor = child,
                    None => return false,
                }
            }
            _ => return false,
        }
    }

    match cursor {
        toml::Value::Table(t) => t.remove(last).is_some(),
        toml::Value::Array(a) => {
            if let Ok(idx) = last.parse::<usize>()
                && idx < a.len()
            {
                a.remove(idx);
                return true;
            }
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_compiles() {
        // If the embedded schema is malformed the validator is `None` and
        // every other test below would silently pass with an empty error
        // list. This test fails loudly when that happens.
        assert!(
            validator().is_some(),
            "embedded JSON schema failed to compile; check assets/config.schema.json"
        );
    }

    #[test]
    fn empty_config_is_valid() {
        let value: toml::Value = toml::from_str("").unwrap();
        assert!(validate_config_value(&value).is_empty());
    }

    #[test]
    fn known_fields_pass() {
        let value: toml::Value = toml::from_str(
            r#"
apiProvider = "openai"
model = "gpt-4o"
maxTokens = 4096
"#,
        )
        .unwrap();
        let errors = validate_config_value(&value);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn unknown_top_level_field_fails() {
        let value: toml::Value = toml::from_str(r#"unknownField = true"#).unwrap();
        let errors = validate_config_value(&value);
        assert!(!errors.is_empty());
        assert!(
            errors.iter().any(|e| e.message.contains("unknownField")
                || e.message.contains("additionalProperties")
                || e.message.contains("not allowed")),
            "expected unknown-field rejection, got: {errors:?}"
        );
    }

    #[test]
    fn bad_max_tokens_range_rejected() {
        let value: toml::Value = toml::from_str(r#"maxTokens = 0"#).unwrap();
        let errors = validate_config_value(&value);
        assert!(!errors.is_empty(), "expected range violation");
    }

    #[test]
    fn bad_permission_mode_rejected() {
        let value: toml::Value = toml::from_str(r#"permissionMode = "invalid""#).unwrap();
        let errors = validate_config_value(&value);
        assert!(!errors.is_empty());
    }

    #[test]
    fn bad_provider_rejected() {
        let value: toml::Value = toml::from_str(r#"apiProvider = "definitely-not-real""#).unwrap();
        let errors = validate_config_value(&value);
        assert!(!errors.is_empty());
    }

    #[test]
    fn bad_permission_rule_pattern_rejected() {
        let value: toml::Value = toml::from_str(
            r#"
[permissions]
allow = ["Bash garbage with spaces"]
"#,
        )
        .unwrap();
        let errors = validate_config_value(&value);
        assert!(
            !errors.is_empty(),
            "permission rule pattern should be enforced"
        );
    }

    #[test]
    fn permission_rule_with_args_passes() {
        let value: toml::Value = toml::from_str(
            r#"
[permissions]
allow = ["Bash(git:*)", "Edit", "*"]
"#,
        )
        .unwrap();
        let errors = validate_config_value(&value);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn enabled_plugins_accepts_bool_or_array() {
        let value: toml::Value = toml::from_str(
            r#"
[enabledPlugins]
"superpowers@official" = true
"foo@bar" = [">=1.0", "<2.0"]
"#,
        )
        .unwrap();
        let errors = validate_config_value(&value);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn enabled_plugins_rejects_non_string_array_items() {
        let value: toml::Value = toml::from_str(
            r#"
[enabledPlugins]
"foo@bar" = [1, 2]
"#,
        )
        .unwrap();
        let errors = validate_config_value(&value);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validation_error_display_with_field() {
        let err = ValidationError {
            field: "/maxTokens".into(),
            message: "must be >= 1".into(),
            suggestion: None,
        };
        let s = err.to_string();
        assert!(s.contains("/maxTokens"));
        assert!(s.contains("must be"));
    }

    #[test]
    fn validation_error_display_root() {
        let err = ValidationError {
            field: String::new(),
            message: "wrong type".into(),
            suggestion: None,
        };
        let s = err.to_string();
        assert!(s.contains("<root>"));
    }

    #[test]
    fn prune_invalid_field_removes_leaf() {
        let mut value: toml::Value = toml::from_str(r#"model = "x""#).unwrap();
        let removed = prune_invalid_field(&mut value, "/model");
        assert!(removed);
        assert!(value.get("model").is_none());
    }

    #[test]
    fn prune_invalid_field_removes_nested_leaf() {
        let mut value: toml::Value = toml::from_str(
            r#"
[permissions]
allow = ["Bash"]
deny = ["Write"]
"#,
        )
        .unwrap();
        let removed = prune_invalid_field(&mut value, "/permissions/allow");
        assert!(removed);
        assert!(value["permissions"].get("allow").is_none());
        assert!(value["permissions"].get("deny").is_some());
    }

    #[test]
    fn prune_invalid_field_removes_array_element() {
        let mut value: toml::Value = toml::from_str(
            r#"
[permissions]
allow = ["Bash", "Edit", "Read"]
"#,
        )
        .unwrap();
        let removed = prune_invalid_field(&mut value, "/permissions/allow/1");
        assert!(removed);
        let allow: Vec<&str> = value["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(allow, vec!["Bash", "Read"]);
    }

    #[test]
    fn prune_invalid_field_no_op_for_missing_path() {
        let mut value: toml::Value = toml::from_str(r#"model = "x""#).unwrap();
        let removed = prune_invalid_field(&mut value, "/nonexistent");
        assert!(!removed);
        assert!(value.get("model").is_some());
    }

    #[test]
    fn prune_invalid_field_no_op_for_empty_path() {
        let mut value: toml::Value = toml::from_str(r#"model = "x""#).unwrap();
        assert!(!prune_invalid_field(&mut value, ""));
        assert!(!prune_invalid_field(&mut value, "/"));
    }

    #[test]
    fn permission_rule_pruning_is_element_wise() {
        // End-to-end: schema must emit element-level errors and prune must
        // drop only the bad element, leaving valid siblings intact.
        // Per `docs/config.md` §10.1, one bad rule cannot poison the whole
        // `permissions.allow` array.
        let mut value: toml::Value = toml::from_str(
            r#"
[permissions]
allow = ["Bash garbage with spaces", "Edit", "Read"]
"#,
        )
        .unwrap();

        let errors = validate_config_value(&value);
        assert!(!errors.is_empty(), "schema must reject the malformed rule");

        // Drop every element the schema flagged. Walk in reverse so that
        // earlier indices stay valid as later ones are removed (matches the
        // descending-index strategy `loader::resolve` uses for arrays).
        let mut indices: Vec<usize> = errors
            .iter()
            .filter_map(|e| {
                let s = e.field.strip_prefix("/permissions/allow/")?;
                s.parse::<usize>().ok()
            })
            .collect();
        indices.sort_unstable();
        indices.dedup();
        for idx in indices.into_iter().rev() {
            let removed = prune_invalid_field(&mut value, &format!("/permissions/allow/{idx}"));
            assert!(removed, "expected to prune /permissions/allow/{idx}");
        }

        // Valid rules survive; bad rule is gone.
        let allow: Vec<&str> = value["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(allow, vec!["Edit", "Read"]);
    }

    #[test]
    fn validate_raw_file_missing_is_empty() {
        let warnings = validate_raw_file(
            std::path::Path::new("/definitely/does/not/exist.toml"),
            "global",
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_raw_file_parse_error_surfaces_label() {
        let dir = std::env::temp_dir().join("crab-config-validation-parse-err");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        std::fs::write(&file, "not = valid = toml").unwrap();
        let warnings = validate_raw_file(&file, "global");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].field.starts_with("[global]"));
        assert!(warnings[0].message.contains("parse error"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
