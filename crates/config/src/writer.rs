//! Comment-preserving config writer backed by `toml_edit`.
//!
//! Unlike a plain `serde + toml::to_string` round trip, this writer parses the
//! existing file as a `toml_edit::DocumentMut`, mutates the addressed key in
//! place, and writes the document back. Comments, key order, and whitespace
//! survive the mutation — strictly better than CCB's JSON write-back, which
//! discards every comment.
//!
//! The writer is the single entry point shared by every `crab config set`
//! invocation. Concretely it:
//!
//! 1. Reads the target file (if it exists) into a `DocumentMut`.
//! 2. Rejects writes to known secret-adjacent keys (e.g. `apiKey`); secrets
//!    must travel through the auth chain, never the persisted `Config`.
//! 3. Inserts the new value at the dotted `key_path`, creating intermediate
//!    tables as needed.
//! 4. Re-parses the resulting text as `toml::Value` and validates it against
//!    the embedded schema. Any violation aborts the write — we never leave a
//!    schema-invalid file on disk.
//! 5. Persists the file. When the target is `Local` and the file did not
//!    exist before, also calls into [`crate::gitignore`] so the project's
//!    `.gitignore` learns about `config.local.toml`.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use crab_core::{Error, Result};

use crate::config;
use crate::gitignore;
use crate::validation;

/// Which on-disk file `set_value` should mutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteTarget {
    /// `~/.crab/config.toml` — global, applies to every project.
    User,
    /// `<project>/.crab/config.toml` — committed alongside the codebase.
    Project,
    /// `<project>/.crab/config.local.toml` — gitignored, machine-local.
    Local,
}

/// Set a single config field, preserving comments and key order.
///
/// `raw_value` is parsed as TOML first (so callers can pass `"42"`, `"true"`,
/// `r#"["a","b"]"#`, `"{ key = \"val\" }"`, etc.) and falls back to a string
/// when parsing fails — the same convention `crab config set` has always used.
///
/// Returns `Err` when:
/// - `key_path` is empty or names a known secret-adjacent field.
/// - The file exists but cannot be parsed as a `DocumentMut`.
/// - Schema validation fails after the write — the file on disk is left
///   untouched (rollback is "don't ever fs::write").
pub fn set_value(target: WriteTarget, key_path: &str, raw_value: &str) -> Result<()> {
    if key_path.is_empty() {
        return Err(Error::Config("empty key_path".into()));
    }
    if is_secret_key(key_path) {
        return Err(Error::Config(format!(
            "refusing to write secret field '{key_path}' via config set; \
             use the auth commands instead"
        )));
    }

    let path = resolve_target_path(target);

    let mut doc = read_document(&path)?;
    let parsed = parse_toml_value(raw_value);
    insert_at_path(&mut doc, key_path, parsed)?;

    let rendered = doc.to_string();
    let as_value: toml::Value = toml::from_str(&rendered)
        .map_err(|e| Error::Config(format!("post-write TOML re-parse failed: {e}")))?;

    let errors = validation::validate_config_value(&as_value);
    if !errors.is_empty() {
        let summary = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::Config(format!(
            "schema violation; refusing to write {}: {summary}",
            path.display()
        )));
    }

    let existed_before = path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Config(format!("failed to create {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, rendered)
        .map_err(|e| Error::Config(format!("failed to write {}: {e}", path.display())))?;

    if target == WriteTarget::Local && !existed_before {
        gitignore::ensure_local_config_ignored(&path)?;
    }

    Ok(())
}

/// Resolve the on-disk path for the given `WriteTarget`. Project/local
/// resolve relative to the current working directory — callers that need a
/// custom project root should `std::env::set_current_dir` before invoking.
fn resolve_target_path(target: WriteTarget) -> PathBuf {
    match target {
        WriteTarget::User => config::global_config_dir().join(config::config_file_name()),
        WriteTarget::Project => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            config::project_config_dir(&cwd).join(config::config_file_name())
        }
        WriteTarget::Local => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            config::project_config_dir(&cwd).join(config::local_config_file_name())
        }
    }
}

/// Read a `DocumentMut` from disk, returning a fresh empty document when
/// the file does not exist. Parse errors are surfaced — silently dropping
/// a corrupt file would lose the user's hand-edits.
fn read_document(path: &Path) -> Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|e| Error::Config(format!("failed to parse {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(Error::Config(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// Insert `new_value` into `doc` at the dotted `key_path`. Intermediate
/// segments that are not yet present become inline-friendly tables; existing
/// tables are reused so their comments/order survive. An intermediate
/// segment that resolves to a non-table value is an error — refusing to
/// silently overwrite scalars with tables avoids data loss.
fn insert_at_path(doc: &mut DocumentMut, key_path: &str, new_value: toml::Value) -> Result<()> {
    let segments: Vec<&str> = key_path.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(Error::Config(format!(
            "invalid key path '{key_path}': empty segment"
        )));
    }
    let (last, parents) = segments
        .split_last()
        .ok_or_else(|| Error::Config("empty key_path".into()))?;

    let mut cursor: &mut Table = doc.as_table_mut();
    for seg in parents {
        let entry = cursor
            .entry(seg)
            .or_insert_with(|| Item::Table(Table::new()));
        if !entry.is_table() {
            return Err(Error::Config(format!(
                "cannot descend into '{seg}' at path '{key_path}': not a table"
            )));
        }
        cursor = entry.as_table_mut().expect("verified is_table() above");
    }

    let new_edit = toml_value_to_edit(&new_value);
    if let Some(existing) = cursor.get_mut(last)
        && let Item::Value(existing_value) = existing
    {
        // Preserve the surrounding key/value decor (comments, whitespace) by
        // mutating the existing Value's payload in place. `toml_edit` keeps
        // prefix/suffix decor on the Value itself, so reusing the slot is
        // strictly better than `Table::insert` which formats anew.
        let prefix = existing_value.decor().prefix().cloned();
        let suffix = existing_value.decor().suffix().cloned();
        let mut replacement = new_edit;
        if let Some(p) = prefix {
            replacement.decor_mut().set_prefix(p);
        }
        if let Some(s) = suffix {
            replacement.decor_mut().set_suffix(s);
        }
        *existing_value = replacement;
    } else {
        cursor.insert(last, value(new_edit));
    }
    Ok(())
}

/// Convert a parsed `toml::Value` into a `toml_edit::Value` for insertion.
/// Falls back to a string when an unsupported variant (datetimes do not
/// round-trip through `toml::Value::to_string`) is encountered.
fn toml_value_to_edit(v: &toml::Value) -> toml_edit::Value {
    match v {
        toml::Value::String(s) => toml_edit::Value::from(s.as_str()),
        toml::Value::Integer(i) => toml_edit::Value::from(*i),
        toml::Value::Float(f) => toml_edit::Value::from(*f),
        toml::Value::Boolean(b) => toml_edit::Value::from(*b),
        toml::Value::Datetime(dt) => toml_edit::Value::from(dt.to_string()),
        toml::Value::Array(arr) => {
            let mut out = toml_edit::Array::new();
            for item in arr {
                out.push(toml_value_to_edit(item));
            }
            toml_edit::Value::Array(out)
        }
        toml::Value::Table(tbl) => {
            let mut out = toml_edit::InlineTable::new();
            for (k, v) in tbl {
                out.insert(k, toml_value_to_edit(v));
            }
            toml_edit::Value::InlineTable(out)
        }
    }
}

/// Try to parse `raw_value` as a typed TOML scalar/array/table. Plain strings
/// and unparseable input fall back to `Value::String`.
fn parse_toml_value(raw: &str) -> toml::Value {
    if let Ok(n) = raw.parse::<i64>() {
        return toml::Value::Integer(n);
    }
    if let Ok(f) = raw.parse::<f64>()
        && raw.contains('.')
    {
        return toml::Value::Float(f);
    }
    match raw {
        "true" => return toml::Value::Boolean(true),
        "false" => return toml::Value::Boolean(false),
        _ => {}
    }
    let trimmed = raw.trim_start();
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(parsed) = toml::from_str::<toml::Table>(&format!("__v = {raw}\n"))
        && let Some(v) = parsed.get("__v")
    {
        return v.clone();
    }
    toml::Value::String(raw.to_string())
}

/// Top-level field paths that must never reach a persisted config file.
///
/// Match is case-insensitive on the leading segment so `apikey`, `APIKEY`,
/// and `apiKey` are all rejected.
fn is_secret_key(key_path: &str) -> bool {
    const SECRET_LEAD: &[&str] = &["apikey"];
    let lead = key_path
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    SECRET_LEAD.contains(&lead.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_kinds() {
        assert_eq!(parse_toml_value("42"), toml::Value::Integer(42));
        assert_eq!(parse_toml_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_toml_value("false"), toml::Value::Boolean(false));
        assert_eq!(
            parse_toml_value("anthropic"),
            toml::Value::String("anthropic".into())
        );
        let arr = parse_toml_value(r#"["a", "b"]"#);
        assert!(arr.is_array());
        let f = parse_toml_value("3.5");
        assert!(matches!(f, toml::Value::Float(_)));
    }

    #[test]
    fn is_secret_key_rejects_apikey_variants() {
        assert!(is_secret_key("apiKey"));
        assert!(is_secret_key("apikey"));
        assert!(is_secret_key("APIKEY"));
        assert!(!is_secret_key("apiKeyHelper"));
        assert!(!is_secret_key("apiProvider"));
        assert!(!is_secret_key("model"));
    }

    #[test]
    fn insert_at_path_top_level() {
        let mut doc: DocumentMut = "".parse().unwrap();
        insert_at_path(&mut doc, "model", toml::Value::String("opus".into())).unwrap();
        let rendered = doc.to_string();
        assert!(rendered.contains("model = \"opus\""));
    }

    #[test]
    fn insert_at_path_creates_nested_table() {
        let mut doc: DocumentMut = "".parse().unwrap();
        insert_at_path(
            &mut doc,
            "permissions.allow",
            toml::Value::Array(vec![toml::Value::String("Bash".into())]),
        )
        .unwrap();
        let rendered = doc.to_string();
        assert!(rendered.contains("permissions"));
        assert!(rendered.contains("allow"));
        assert!(rendered.contains("Bash"));
    }

    #[test]
    fn insert_at_path_rejects_descent_into_scalar() {
        let mut doc: DocumentMut = "model = \"opus\"\n".parse().unwrap();
        let err =
            insert_at_path(&mut doc, "model.sub", toml::Value::String("x".into())).unwrap_err();
        assert!(err.to_string().contains("not a table"));
    }

    #[test]
    fn insert_at_path_rejects_empty_segment() {
        let mut doc: DocumentMut = "".parse().unwrap();
        let err =
            insert_at_path(&mut doc, "permissions..allow", toml::Value::Boolean(true)).unwrap_err();
        assert!(err.to_string().contains("empty segment"));
    }
}
