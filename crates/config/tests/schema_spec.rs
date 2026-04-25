//! Schema-level integration tests.
//!
//! Two layers of protection against drift:
//!
//! 1. [`example_configs_conform_to_schema`] — every fixture under
//!    `tests/fixtures/config_examples/*.toml` must validate cleanly. New
//!    Config fields land here first; the test fails loudly if a fixture
//!    references an unknown field or an outdated enum value.
//! 2. [`rust_defaults_match_schema_defaults`] — every leaf with a `default`
//!    keyword in the embedded schema must match what `Config::default()`
//!    serializes for the same JSON Pointer path. Catches the case where
//!    Rust adds a default and forgets to update the schema (or vice
//!    versa).
//!
//! Dynamic defaults (e.g. `env::current_dir()`-derived) are intentionally
//! NOT declared in the schema; the drift guard skips paths whose schema
//! `default` is `null` and whose Rust value is `null`/`Option::None`.

use std::fs;
use std::path::PathBuf;

use crab_config::Config;

const SCHEMA_SRC: &str = include_str!("../assets/config.schema.json");

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("config_examples")
}

#[test]
fn example_configs_conform_to_schema() {
    let dir = fixtures_dir();
    assert!(
        dir.is_dir(),
        "fixtures dir is missing: {}",
        dir.display(),
    );

    let mut checked = 0usize;
    for entry in fs::read_dir(&dir).expect("read fixtures dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read fixture");
        let value: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("{} parse failed: {e}", path.display()));
        let errors = crab_config::validate_config(
            &serde_json::to_value(&value).expect("toml→json"),
        );
        assert!(
            errors.is_empty(),
            "{} violates schema:\n{}",
            path.display(),
            errors
                .iter()
                .map(|e| format!("  - {e}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected at least 3 fixtures, found {checked}",
    );
}

/// Walk the schema and collect every `(json_pointer, default_value)` pair
/// declared on a leaf. Object/property defaults are NOT collected (their
/// children are walked instead) — this matches the "leaf-by-leaf" spirit
/// of the drift guard.
fn collect_schema_defaults(
    schema: &serde_json::Value,
    pointer: &mut String,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    if let Some(obj) = schema.as_object() {
        if let Some(default) = obj.get("default") {
            // Record the default at this node's pointer. Recursing into
            // `properties` from here is still valuable when both this node
            // AND its children declare defaults (e.g. `gitContext` carries
            // `default: null` while its `enabled`/`maxDiffLines` carry real
            // defaults — both are recorded).
            out.push((pointer.clone(), default.clone()));
        }
        if let Some(props) = obj.get("properties").and_then(|v| v.as_object()) {
            for (key, sub) in props {
                let saved_len = pointer.len();
                pointer.push('/');
                pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
                collect_schema_defaults(sub, pointer, out);
                pointer.truncate(saved_len);
            }
        }
    }
}

/// Resolve a JSON Pointer in `value`, returning the leaf or `None`.
fn json_pointer_get<'a>(
    value: &'a serde_json::Value,
    pointer: &str,
) -> Option<&'a serde_json::Value> {
    if pointer.is_empty() {
        return Some(value);
    }
    value.pointer(pointer)
}

#[test]
fn rust_defaults_match_schema_defaults() {
    let schema: serde_json::Value =
        serde_json::from_str(SCHEMA_SRC).expect("schema parses as JSON");
    let rust = serde_json::to_value(Config::default()).expect("Config serializes");

    let mut defaults = Vec::new();
    let mut pointer = String::new();
    collect_schema_defaults(&schema, &mut pointer, &mut defaults);

    assert!(
        !defaults.is_empty(),
        "no `default` keywords found in schema — has the asset been emptied?"
    );

    let mut mismatches = Vec::new();
    for (path, schema_default) in &defaults {
        // Skip the root-level `default` (none declared) and any path that
        // does not exist in the Rust serialization (those are typically
        // sub-properties of an `Option<…>` that defaults to None — the
        // schema documents the substructure for IDEs but Rust collapses
        // the whole branch to `null`).
        if path.is_empty() {
            continue;
        }

        let rust_value = json_pointer_get(&rust, path).cloned();

        match (rust_value, schema_default) {
            // Schema says null, Rust has the field absent (None) — fine.
            (None, serde_json::Value::Null) => {}
            // Schema says null, Rust has it explicitly null — fine.
            (Some(serde_json::Value::Null), serde_json::Value::Null) => {}
            // Schema-declared default for a sub-property of an Option<T>
            // that defaults to None: Rust collapses the parent to null,
            // so the path is absent. The schema's default still serves as
            // documentation. Accept this asymmetry.
            (None, _) => {}
            // Same value — drift-free.
            (Some(rv), sv) if &rv == sv => {}
            // Anything else is a real mismatch.
            (Some(rv), sv) => {
                mismatches.push(format!(
                    "  {path}: schema default = {sv}, Rust serialized = {rv}",
                ));
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "schema/Rust default drift detected:\n{}",
        mismatches.join("\n"),
    );
}
