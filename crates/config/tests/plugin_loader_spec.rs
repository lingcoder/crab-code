//! Integration tests for the plugin layer loader.
//!
//! Each test sets up a temporary `$CRAB_CONFIG_DIR` populated by copying
//! fixtures from `tests/fixtures/plugins/`. The fixtures stay read-only on
//! disk so the test can run in parallel; copying into a per-test temp dir
//! keeps the layout the loader expects.

use std::fs;
use std::path::{Path, PathBuf};

use crab_config::loader::ResolveContext;
use crab_config::plugin_loader::load_enabled_plugin_configs;

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("plugins")
        .join(name)
}

fn copy_plugin_into(config_dir: &Path, plugin_name: &str) {
    let src = fixture_dir(plugin_name);
    let dst = config_dir.join("plugins").join(plugin_name);
    fs::create_dir_all(&dst).unwrap();
    for entry in fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        fs::copy(&from, &to).unwrap();
    }
}

fn write_user_config(config_dir: &Path, body: &str) {
    fs::create_dir_all(config_dir).unwrap();
    fs::write(config_dir.join("config.toml"), body).unwrap();
}

fn isolated_config_dir(scope: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("crab-plugin-loader-tests")
        .join(scope);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn all_enabled_merge_in_alphabetical_order() {
    let dir = isolated_config_dir("all-enabled");
    copy_plugin_into(&dir, "alpha");
    copy_plugin_into(&dir, "beta");
    copy_plugin_into(&dir, "zeta");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = true
beta = true
zeta = true
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    assert_eq!(configs.len(), 3);
    assert_eq!(
        configs[0].get("model").and_then(|v| v.as_str()),
        Some("alpha-model")
    );
    assert_eq!(
        configs[1].get("model").and_then(|v| v.as_str()),
        Some("beta-model")
    );
    assert_eq!(
        configs[2].get("model").and_then(|v| v.as_str()),
        Some("zeta-model")
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn disabled_plugin_is_skipped() {
    let dir = isolated_config_dir("beta-disabled");
    copy_plugin_into(&dir, "alpha");
    copy_plugin_into(&dir, "beta");
    copy_plugin_into(&dir, "zeta");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = true
beta = false
zeta = true
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    let models: Vec<&str> = configs
        .iter()
        .filter_map(|c| c.get("model")?.as_str())
        .collect();
    assert_eq!(models, vec!["alpha-model", "zeta-model"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn broken_plugin_is_skipped_with_warning() {
    let dir = isolated_config_dir("broken-skip");
    copy_plugin_into(&dir, "alpha");
    copy_plugin_into(&dir, "broken");
    copy_plugin_into(&dir, "zeta");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = true
broken = true
zeta = true
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    // alpha and zeta succeed, broken JSON is skipped (not propagated)
    let models: Vec<&str> = configs
        .iter()
        .filter_map(|c| c.get("model")?.as_str())
        .collect();
    assert_eq!(models, vec!["alpha-model", "zeta-model"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn plugin_setting_env_field_is_rejected() {
    let dir = isolated_config_dir("gamma-rejected");
    copy_plugin_into(&dir, "alpha");
    copy_plugin_into(&dir, "gamma");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = true
gamma = true
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    // alpha succeeds, gamma is rejected for setting `env` (skip-and-warn)
    let models: Vec<&str> = configs
        .iter()
        .filter_map(|c| c.get("model")?.as_str())
        .collect();
    assert_eq!(models, vec!["alpha-model"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn missing_plugins_dir_returns_empty_list() {
    let dir = isolated_config_dir("no-plugins-dir");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = true
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    assert!(configs.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn version_constraints_value_enables_plugin() {
    let dir = isolated_config_dir("version-constraints");
    copy_plugin_into(&dir, "alpha");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = [">=1.0", "<2.0"]
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    assert_eq!(configs.len(), 1);
    assert_eq!(
        configs[0].get("model").and_then(|v| v.as_str()),
        Some("alpha-model")
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn no_user_config_means_no_plugins_enabled() {
    let dir = isolated_config_dir("no-user-config");
    copy_plugin_into(&dir, "alpha");
    copy_plugin_into(&dir, "beta");
    // Note: no config.toml written.

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let configs = load_enabled_plugin_configs(&ctx).expect("load ok");
    assert!(configs.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn alphabetical_merge_through_resolve() {
    // End-to-end check: the value-layer merge engine consumes the plugin
    // contributions in order, so `permissions.allow` should accumulate
    // across alpha/beta/zeta with no duplicates.
    let dir = isolated_config_dir("e2e-resolve");
    copy_plugin_into(&dir, "alpha");
    copy_plugin_into(&dir, "beta");
    copy_plugin_into(&dir, "zeta");
    write_user_config(
        &dir,
        r#"[enabledPlugins]
alpha = true
beta = true
zeta = true
"#,
    );

    let ctx = ResolveContext::new().with_config_dir(dir.clone());
    let cfg = crab_config::loader::resolve(&ctx).expect("resolve ok");

    // zeta is alphabetically last and sets `model = "zeta-model"` and
    // `theme = "zeta-theme"`; later contributions win for scalars.
    assert_eq!(cfg.model.as_deref(), Some("zeta-model"));
    assert_eq!(cfg.theme.as_deref(), Some("zeta-theme"));

    let allow = cfg.permissions.expect("permissions present").allow;
    assert_eq!(allow, vec!["Bash", "Edit", "Read"]);

    let _ = fs::remove_dir_all(&dir);
}
