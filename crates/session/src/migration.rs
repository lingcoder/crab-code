//! Data migration system for settings/session format changes.
//!
//! As Crab Code evolves, the on-disk formats for settings files, session
//! transcripts, and memory entries may change. This module provides a
//! versioned migration pipeline that upgrades data from any historical
//! version to the current version.
//!
//! Each migration is a pure function that transforms a `serde_json::Value`
//! in place. Migrations are applied sequentially from the data's current
//! version to the target version.

use serde_json::Value;

// ── Types ─────────────────────────────────────────────────────────────

/// A single data migration step.
pub struct Migration {
    /// Target version number after this migration runs.
    pub version: u32,
    /// Human-readable name for logging (e.g. "`add_model_field`").
    pub name: &'static str,
    /// The migration function. Receives a mutable reference to the data
    /// and returns `Ok(())` on success or `Err(description)` on failure.
    pub migrate: fn(&mut Value) -> Result<(), String>,
}

// ── Registry ──────────────────────────────────────────────────────────

/// Return the full list of available migrations, ordered by version.
///
/// Each migration upgrades the data from `version - 1` to `version`.
#[must_use]
pub fn available_migrations() -> Vec<Migration> {
    // No migrations defined yet. As the on-disk format evolves,
    // migration entries will be added here in version order.
    Vec::new()
}

/// Read the current schema version from a data blob.
///
/// Looks for a `"version"` key at the top level. Returns `0` if the
/// key is missing (pre-versioning data).
#[must_use]
pub fn current_version(data: &Value) -> u32 {
    data.get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32
}

// ── Execution ─────────────────────────────────────────────────────────

/// Run all applicable migrations to bring `data` up to `target_version`.
///
/// Applies migrations sequentially from `current_version(data) + 1`
/// through `target_version`. Each successful migration's name is
/// collected in the returned `Vec<String>`.
///
/// # Errors
///
/// Returns `Err` with a description if any migration step fails.
/// The data may be partially migrated; callers should not persist it
/// on error.
pub fn run_migrations(data: &mut Value, target_version: u32) -> Result<Vec<String>, String> {
    let current = current_version(data);
    let mut applied = Vec::new();

    for m in available_migrations() {
        if m.version <= current || m.version > target_version {
            continue;
        }
        (m.migrate)(data)
            .map_err(|e| format!("migration '{}' (v{}) failed: {e}", m.name, m.version))?;
        // Stamp the new version into the data.
        if let Some(obj) = data.as_object_mut() {
            obj.insert("version".into(), Value::from(u64::from(m.version)));
        }
        applied.push(m.name.to_string());
    }

    Ok(applied)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_struct_fields() {
        let m = Migration {
            version: 1,
            name: "initial",
            migrate: |_data| Ok(()),
        };
        assert_eq!(m.version, 1);
        assert_eq!(m.name, "initial");
        assert!((m.migrate)(&mut Value::Null).is_ok());
    }
}
