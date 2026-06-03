//! Specification-level tests for the value-layer merge engine.
//!
//! Each test corresponds to a row in `docs/config-design.md` §4 (Merge Semantics)
//! or §9 (Precedence Summary). They exercise [`merge_toml_values`]
//! directly so the contract is independent of the loader pipeline.

use crab_config::merge::{dedup_preserving_order, merge_toml_values};
use crab_config::{Config, PermissionsConfig, ResolveContext, resolve};
use toml::Value;
use toml::value::Table;

fn parse(s: &str) -> Value {
    toml::from_str(s).expect("valid TOML")
}

fn write(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
}

// ── §4.2 row: scalar — later wins ───────────────────────────────────────

#[test]
fn scalar_later_wins() {
    let mut base = parse("model = \"opus\"\nmaxTokens = 1024");
    merge_toml_values(&mut base, parse("model = \"sonnet\""));
    assert_eq!(base.get("model").unwrap().as_str(), Some("sonnet"));
    // Untouched scalar survives:
    assert_eq!(base.get("maxTokens").unwrap().as_integer(), Some(1024));
}

// ── §4.2 row: empty overlay no-op ───────────────────────────────────────

#[test]
fn empty_overlay_does_not_clear_base() {
    let mut base = parse("model = \"opus\"\ntheme = \"dark\"\n");
    merge_toml_values(&mut base, Value::Table(Table::new()));
    assert_eq!(base.get("model").unwrap().as_str(), Some("opus"));
    assert_eq!(base.get("theme").unwrap().as_str(), Some("dark"));
}

// ── §4.2 row: table deep-merge (two and three levels) ───────────────────

#[test]
fn table_deep_merge_two_levels() {
    let mut base = parse(
        r"
[gitContext]
enabled = true
maxDiffLines = 200
",
    );
    merge_toml_values(
        &mut base,
        parse(
            r"
[gitContext]
maxDiffLines = 50
",
        ),
    );
    let g = base.get("gitContext").unwrap().as_table().unwrap();
    assert_eq!(g.get("enabled").unwrap().as_bool(), Some(true));
    assert_eq!(g.get("maxDiffLines").unwrap().as_integer(), Some(50));
}

#[test]
fn table_deep_merge_three_levels() {
    let mut base = parse(
        r#"
[mcp_servers.github]
url = "https://example.com"

[mcp_servers.github.headers]
"X-User" = "alice"
"X-Org" = "team"
"#,
    );
    merge_toml_values(
        &mut base,
        parse(
            r#"
[mcp_servers.github.headers]
"X-Org" = "newteam"
"X-Trace" = "abc"
"#,
        ),
    );
    let headers = base
        .get("mcp_servers")
        .unwrap()
        .get("github")
        .unwrap()
        .get("headers")
        .unwrap()
        .as_table()
        .unwrap();
    assert_eq!(headers.get("X-User").unwrap().as_str(), Some("alice"));
    assert_eq!(headers.get("X-Org").unwrap().as_str(), Some("newteam"));
    assert_eq!(headers.get("X-Trace").unwrap().as_str(), Some("abc"));
}

// ── §4.2 row: array concat+dedup with insertion order ───────────────────

#[test]
fn array_concat_dedup_preserves_first_occurrence_order() {
    let mut base = parse(r#"allow = ["Read", "Bash"]"#);
    merge_toml_values(&mut base, parse(r#"allow = ["Bash", "Edit", "Read"]"#));
    let allow = base.get("allow").unwrap().as_array().unwrap();
    let strs: Vec<&str> = allow.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(strs, vec!["Read", "Bash", "Edit"]);
}

#[test]
fn array_table_elements_are_dedeuplicated_structurally() {
    let mut base = parse(
        r#"
hooks = [
  { trigger = "pre_tool_use", command = "echo a" },
  { trigger = "post_tool_use", command = "echo b" },
]
"#,
    );
    merge_toml_values(
        &mut base,
        parse(
            r#"
hooks = [
  { trigger = "pre_tool_use", command = "echo a" },
  { trigger = "stop", command = "echo c" },
]
"#,
        ),
    );
    let arr = base.get("hooks").unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 3);
    let triggers: Vec<&str> = arr
        .iter()
        .map(|v| v.get("trigger").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(triggers, vec!["pre_tool_use", "post_tool_use", "stop"]);
}

// ── §4.2 row: type conflict (table vs scalar) — overlay wins ────────────

#[test]
fn table_overwritten_by_scalar_overlay() {
    let mut base = parse(
        r#"
[permissions]
allow = ["Bash"]
"#,
    );
    merge_toml_values(&mut base, parse(r#"permissions = "default""#));
    assert_eq!(
        base.get("permissions").unwrap().as_str(),
        Some("default"),
        "overlay scalar must replace the base table"
    );
}

#[test]
fn scalar_overwritten_by_table_overlay() {
    let mut base = parse(r#"permissions = "default""#);
    merge_toml_values(
        &mut base,
        parse(
            r#"
[permissions]
allow = ["Bash"]
"#,
        ),
    );
    let allow = base
        .get("permissions")
        .unwrap()
        .get("allow")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(allow.len(), 1);
}

// ── §4.4: mcpServers.<name> field-level merge ────────────────────────────

#[test]
fn mcp_servers_same_name_field_level_merge() {
    let mut base = parse(
        r#"
[mcp_servers.github]
url = "https://mcp.github.com"
args = ["--verbose"]
"#,
    );
    merge_toml_values(
        &mut base,
        parse(
            r#"
[mcp_servers.github]
auth = "oauth"
args = ["--quiet"]
"#,
        ),
    );
    let github = base
        .get("mcp_servers")
        .unwrap()
        .get("github")
        .unwrap()
        .as_table()
        .unwrap();
    assert_eq!(
        github.get("url").unwrap().as_str(),
        Some("https://mcp.github.com")
    );
    assert_eq!(github.get("auth").unwrap().as_str(), Some("oauth"));
    let args = github.get("args").unwrap().as_array().unwrap();
    let strs: Vec<&str> = args.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(strs, vec!["--verbose", "--quiet"]);
}

// ── dedup helper directly ────────────────────────────────────────────────

#[test]
fn dedup_preserves_first_occurrence_order() {
    let mut arr = vec![
        Value::String("a".into()),
        Value::Integer(1),
        Value::String("a".into()),
        Value::Integer(2),
        Value::Integer(1),
    ];
    dedup_preserving_order(&mut arr);
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0].as_str(), Some("a"));
    assert_eq!(arr[1].as_integer(), Some(1));
    assert_eq!(arr[2].as_integer(), Some(2));
}

// ── End-to-end: permissions.allow accumulates across user/project/local ─

#[test]
fn permissions_allow_accumulates_across_user_project_local() {
    let root = std::env::temp_dir().join("crab-merge-spec-allow-accum");
    let _ = std::fs::remove_dir_all(&root);
    let user_dir = root.join("user");
    let project_dir = root.join("project");
    let project_crab = project_dir.join(".crab");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(&project_crab).unwrap();

    write(
        &user_dir.join("config.toml"),
        r#"
[permissions]
allow = ["Bash"]
"#,
    );
    write(
        &project_crab.join("config.toml"),
        r#"
[permissions]
allow = ["Edit"]
"#,
    );
    write(
        &project_crab.join("config.local.toml"),
        r#"
[permissions]
allow = ["Read"]
"#,
    );

    let ctx = ResolveContext::new()
        .with_config_dir(user_dir)
        .with_project_dir(Some(project_dir));
    let cfg: Config = resolve(&ctx).unwrap();
    let allow = cfg.permissions.unwrap().allow;
    assert_eq!(
        allow,
        vec!["Bash".to_string(), "Edit".to_string(), "Read".to_string()],
        "all three layers must contribute and order must be preserved"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ── End-to-end: mcpServers.<name> field-level merge through resolve ────

#[test]
fn resolve_merges_mcp_server_fields_across_layers() {
    let root = std::env::temp_dir().join("crab-merge-spec-mcp");
    let _ = std::fs::remove_dir_all(&root);
    let user_dir = root.join("user");
    let project_dir = root.join("project");
    let project_crab = project_dir.join(".crab");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(&project_crab).unwrap();

    write(
        &user_dir.join("config.toml"),
        r#"
[mcp_servers.github]
url = "https://mcp.github.com"
"#,
    );
    write(
        &project_crab.join("config.toml"),
        r#"
[mcp_servers.github]
auth = "oauth"
"#,
    );

    let ctx = ResolveContext::new()
        .with_config_dir(user_dir)
        .with_project_dir(Some(project_dir));
    let cfg: Config = resolve(&ctx).unwrap();
    let github = cfg
        .mcp_servers
        .unwrap()
        .get("github")
        .cloned()
        .expect("github server present");
    assert_eq!(
        github.get("url").and_then(|v| v.as_str()),
        Some("https://mcp.github.com")
    );
    assert_eq!(github.get("auth").and_then(|v| v.as_str()), Some("oauth"));

    let _ = std::fs::remove_dir_all(&root);
}

// ── Regression: PermissionsConfig overlay through `Config` round-trip ──

#[test]
fn config_overlay_preserves_concat_dedup_for_allow() {
    let base = Config {
        permissions: Some(PermissionsConfig {
            allow: vec!["Bash".into(), "Read".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let overlay = Config {
        permissions: Some(PermissionsConfig {
            allow: vec!["Read".into(), "Edit".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let merged = crab_config::overlay_config(&base, &overlay).unwrap();
    let allow = merged.permissions.unwrap().allow;
    assert_eq!(
        allow,
        vec!["Bash".to_string(), "Read".to_string(), "Edit".to_string()]
    );
}
