//! Integration tests for the comment-preserving config writer.
//!
//! Each test runs against a temporary directory so they never touch the
//! user's real `~/.crab/` or any project on disk. Tests that exercise the
//! `Local` write target also flip the process-wide CWD; serialise them
//! through a mutex so a parallel runner cannot interleave the changes.

use std::path::Path;
use std::sync::Mutex;

use crab_config::writer::{WriteTarget, set_value};
use tempfile::TempDir;

static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with `dir` as the process working directory. Restores the
/// previous CWD afterward even if `f` panics.
fn with_cwd<R>(dir: &Path, f: impl FnOnce() -> R) -> R {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::env::set_current_dir(prev).unwrap();
    match result {
        Ok(v) => v,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[test]
fn preserves_top_and_inline_comments() {
    let tmp = TempDir::new().unwrap();
    let crab_dir = tmp.path().join(".crab");
    std::fs::create_dir_all(&crab_dir).unwrap();
    let config = crab_dir.join("config.toml");
    std::fs::write(
        &config,
        "# my main comment\nmodel = \"opus\" # inline comment\n",
    )
    .unwrap();

    with_cwd(tmp.path(), || {
        set_value(WriteTarget::Project, "model", "sonnet").unwrap();
    });

    let after = std::fs::read_to_string(&config).unwrap();
    assert!(
        after.contains("# my main comment"),
        "top comment dropped: {after}"
    );
    assert!(
        after.contains("# inline comment"),
        "inline comment dropped: {after}"
    );
    assert!(after.contains("\"sonnet\""), "value not updated: {after}");
}

#[test]
fn rejects_secret_field() {
    let tmp = TempDir::new().unwrap();
    with_cwd(tmp.path(), || {
        let err = set_value(WriteTarget::Project, "apiKey", "sk-test").unwrap_err();
        assert!(err.to_string().contains("refusing to write secret"));
    });
}

#[test]
fn rejects_secret_field_lowercase() {
    let tmp = TempDir::new().unwrap();
    with_cwd(tmp.path(), || {
        let err = set_value(WriteTarget::Project, "apikey", "sk-test").unwrap_err();
        assert!(err.to_string().contains("refusing to write secret"));
    });
}

#[test]
fn writes_nested_path() {
    let tmp = TempDir::new().unwrap();
    with_cwd(tmp.path(), || {
        set_value(
            WriteTarget::Project,
            "permissions.allow",
            r#"["Bash(git:*)"]"#,
        )
        .unwrap();
    });

    let config = tmp.path().join(".crab").join("config.toml");
    let parsed: toml::Value = toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    let allow = parsed["permissions"]["allow"].as_array().unwrap();
    assert_eq!(allow.len(), 1);
    assert_eq!(allow[0].as_str(), Some("Bash(git:*)"));
}

#[test]
fn schema_violation_rolls_back_when_file_missing() {
    let tmp = TempDir::new().unwrap();
    with_cwd(tmp.path(), || {
        // permissionMode only accepts a known enum — `"bogus"` fails the schema.
        let err = set_value(WriteTarget::Project, "permissionMode", "bogus").unwrap_err();
        assert!(err.to_string().contains("schema violation"), "{err}");
    });

    let config = tmp.path().join(".crab").join("config.toml");
    assert!(!config.exists(), "rollback should not leave a file behind");
}

#[test]
fn schema_violation_does_not_overwrite_existing_file() {
    let tmp = TempDir::new().unwrap();
    let crab_dir = tmp.path().join(".crab");
    std::fs::create_dir_all(&crab_dir).unwrap();
    let config = crab_dir.join("config.toml");
    let original = "model = \"opus\"\n";
    std::fs::write(&config, original).unwrap();

    with_cwd(tmp.path(), || {
        let err = set_value(WriteTarget::Project, "permissionMode", "bogus").unwrap_err();
        assert!(err.to_string().contains("schema violation"), "{err}");
    });

    let after = std::fs::read_to_string(&config).unwrap();
    assert_eq!(after, original, "rollback must not overwrite existing file");
}

#[test]
fn local_write_creates_dir_and_file() {
    let tmp = TempDir::new().unwrap();
    // Initialise as a Git repo so the gitignore hook can find a project root.
    std::fs::create_dir(tmp.path().join(".git")).unwrap();

    with_cwd(tmp.path(), || {
        set_value(WriteTarget::Local, "model", "opus").unwrap();
    });

    let local = tmp.path().join(".crab").join("config.local.toml");
    assert!(local.exists(), "local config not created");
    let parsed: toml::Value = toml::from_str(&std::fs::read_to_string(&local).unwrap()).unwrap();
    assert_eq!(parsed["model"].as_str(), Some("opus"));
}

#[test]
fn first_local_write_appends_gitignore_entry() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();

    with_cwd(tmp.path(), || {
        set_value(WriteTarget::Local, "model", "opus").unwrap();
    });

    let gi = tmp.path().join(".gitignore");
    assert!(gi.exists(), ".gitignore should be created");
    let content = std::fs::read_to_string(&gi).unwrap();
    assert!(
        content.contains("config.local.toml"),
        "gitignore missing entry: {content}"
    );
}

#[test]
fn repeated_local_writes_do_not_duplicate_gitignore_entry() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();

    with_cwd(tmp.path(), || {
        set_value(WriteTarget::Local, "model", "opus").unwrap();
        set_value(WriteTarget::Local, "model", "sonnet").unwrap();
        set_value(WriteTarget::Local, "model", "haiku").unwrap();
    });

    let gi = tmp.path().join(".gitignore");
    let content = std::fs::read_to_string(&gi).unwrap();
    assert_eq!(
        content.matches("config.local.toml").count(),
        1,
        "expected exactly one entry, got: {content}"
    );
}

#[test]
fn writer_uses_dotted_path_for_existing_table() {
    let tmp = TempDir::new().unwrap();
    let crab_dir = tmp.path().join(".crab");
    std::fs::create_dir_all(&crab_dir).unwrap();
    let config = crab_dir.join("config.toml");
    std::fs::write(
        &config,
        "# header\n\n[permissions]\n# inside permissions\nallow = [\"Read\"]\n",
    )
    .unwrap();

    with_cwd(tmp.path(), || {
        set_value(WriteTarget::Project, "permissions.deny", r#"["Write"]"#).unwrap();
    });

    let after = std::fs::read_to_string(&config).unwrap();
    assert!(after.contains("# header"));
    assert!(after.contains("# inside permissions"));
    assert!(after.contains("Read"));
    assert!(after.contains("Write"));
}
