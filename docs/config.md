# Crab Code Configuration Loading & Resolution

> Updated: 2026-04-25
> Scope: How Crab resolves the effective `Config` from files, environment variables, and CLI flags. Covers source precedence, merge semantics, file formats, and the separation between the persisted `file` layer and the ephemeral `runtime` layer.

---

## 1. Overview

Crab uses a **two-layer configuration model**:

```
runtime layer (in-memory, not persisted)
    env (non-secret)  <  CLI flag (field-level)
    |
    v
file layer (persisted, file-driven)
    defaults  <  plugin  <  user  <  project  <  local  <  --config <file>
```

**Why two layers.** The `file` layer expresses the user's persisted intent (editable, diffable, exportable); the `runtime` layer is a transient override for a single invocation (env injection, CLI flag). They are loaded independently and merged only at resolve time, ensuring runtime overrides never leak into persisted files.

---

## 2. Load Sources (Low to High Priority)

| # | Layer | Source | Path / Form | Format |
|---|-------|--------|-------------|--------|
| 1 | file | **defaults** | Compiled into the binary | Rust code |
| 2 | file | **plugin** | `$CRAB_CONFIG_DIR/plugins/<name>/config.json` (every enabled plugin, merged in alphabetical order of `<name>`) | JSON |
| 3 | file | **user** | `$CRAB_CONFIG_DIR/config.toml` (defaults to `~/.crab/config.toml`) | TOML |
| 4 | file | **project** | `$PWD/.crab/config.toml` (intended to be committed) | TOML |
| 5 | file | **local** | `$PWD/.crab/config.local.toml` (auto-added to `.gitignore`) | TOML |
| 6 | file | **`--config <path>`** | Whole-file injection from CLI | TOML |
| 7 | runtime | **env (non-secret)** | `CRAB_MODEL`, `CRAB_API_PROVIDER`, `CRAB_API_BASE_URL`, etc. | OS env → partial `Value` |
| 8 | runtime | **CLI flag** | `--model`, `--permission-mode`, `-c key.path=value` | clap → partial `Value` |

### Notes on the Plugin Layer

- **Gating**: the `enabledPlugins` field in the main `config.toml` determines which plugins' `config.json` files get loaded. Schema (aligned with CCB `types.ts:566-574`):
  ```
  enabledPlugins: HashMap<String, Value>
  // key format: "plugin-id@marketplace-id"
  //   (e.g., "superpowers@claude-plugins-official")
  // value variants: true (enabled) / false (explicitly disabled) / Vec<String> (version constraints)
  // An explicit `false` is equivalent to omission; it exists only to override a
  // plugin that would otherwise be enabled by default (e.g., a bundled plugin).
  ```
- **Load timing**: at process startup, every enabled plugin's `config.json` is loaded **once** and merged into the file layer. Skill/command/subagent invocations never re-trigger config merging. "Plugin enabled" (contributes config) and "plugin used" (runs skill/wasm) are independent events.
- **Format**: JSON — same format as `plugin.json` manifest. Everything under a plugin directory is machine-distributed and not hand-edited, so JSON is the appropriate format. The loader parses each file as `serde_json::Value`, then converts to `toml::Value` to join the main merge chain.
- **Order**: plugins merged alphabetically by `<name>`. This differs from CCB's registration-order behavior — alphabetical order is more deterministic.
- **Optional**: a plugin is not required to ship a `config.json`. Most existing CCB plugins contribute settings via code registration; Crab plugins use a static file because WASM/Skill plugins cannot cleanly call host-side registration APIs.
- **Security**: plugins cannot set the `env` field via `config.json` (this would let them silently inject secrets or proxies). The schema enforces a reduced subset for plugin contributions.
- **First version stance**: the directory layout borrows from Anthropic's plugin convention (`plugins/<name>/plugin.json` + `skills/` + `commands/` + `agents/`) so Crab can consume the Anthropic plugin repo. The `config.json` contribution file is Crab's own addition.

**Loader root.** `CRAB_CONFIG_DIR` overrides the user-level root (defaults to `~/.crab/`). It is **not a configuration layer** — it only relocates source 2 and the `auth/` directory. Useful for containers, integration tests, and multi-identity setups.

**Project directory lookup.** Crab only looks at `$PWD/.crab/`; it does **not** walk up to the git root. This keeps each setting's origin predictable. If monorepo support becomes necessary later, it can be added behind an opt-in flag.

---

## 3. Sources Outside the Merge Chain

The following values never participate in the file/runtime merge chain, never appear in the resolved `Config`, are never printed by `crab config show`, and are never diffed against other layers.

| Kind | Location | Purpose |
|------|----------|---------|
| **OAuth tokens + subscription info** | `$CRAB_CONFIG_DIR/auth/tokens.json` (mode `0600`) | Written during OAuth flow; machine-written and machine-read, hence JSON. The `auth/` directory is reserved for future per-provider credential files |
| **Secret env vars** | `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, `CRAB_API_KEY` | Read directly from OS environment by the `auth` module at request time |
| **System keychain / credential manager** | macOS Keychain, Windows Credential Manager (future), Linux Secret Service (future) | Optional secure storage for API keys |
| **`apiKeyHelper` script output** | Script path declared in `config.toml`; stdout consumed as the key | The path is configuration; the returned secret never enters `Config` |

### Authentication Resolution Order

```
ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY env
    → apiKeyHelper script
    → system keychain
    → auth/tokens.json (OAuth)
    → error
```

This chain is orthogonal to the file-layer merge chain. A user may configure `apiKeyHelper` in `config.toml`, but the secret it returns never round-trips through `Config`.

---

## 4. Merge Semantics

### 4.1 Core Function

All merging happens at the `toml::Value` layer before deserialization. Following the OpenAI Codex CLI approach, a single recursive function defines the semantics for every layer:

```rust
fn merge_toml_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Table(b), Table(o)) => {
            for (k, v) in o {
                merge_toml_values(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (Array(b), Array(o)) => {
            b.extend(o);
            dedup_preserving_order(b);
        }
        (b @ Value::Null, o) => *b = o,
        (b, o) => *b = o,
    }
}
```

### 4.2 Field-Type Semantics

| Field kind | Behavior |
|------------|----------|
| Table / object (`permissions`, `hooks`, `mcpServers`, …) | **Deep merge** — recurse into nested keys |
| Array (`permissions.allow`, `additionalDirectories`, …) | **Concatenate + deduplicate** (insertion order preserved) |
| Scalar (string, number, bool) | Later wins |
| `Value::Null` in overlay | Skipped (does not clear an existing value) |

The concat+dedup rule ensures that, for example, `permissions.allow` accumulates entries across user/project/local layers instead of each layer replacing the previous one wholesale.

### 4.3 Permissions Resolution (Deny Always Wins)

After `permissions.allow` and `permissions.deny` have been merged (concat+dedup), tool-invocation authorization follows this order (aligned with CCB `permissions.ts:1169-1297`):

1. **Check deny first**: if any deny rule matches, return denial immediately — subsequent allow rules are **never evaluated**.
2. Check allow: a match grants permission.
3. No match: fall through to `permission-mode` (`plan` / `acceptEdits` / `ask` / `dontAsk` / `bypassPermissions`).

Within a single rule category (allow/deny/ask), source precedence is:
`user < project < local < policy < cliArg < command < session`

Within a single source, tool-level rules (no content qualifier, e.g. `Bash`) are matched before content-level rules (with pattern, e.g. `Bash(git:*)`).

**Invariant**: **no allow rule can ever override a deny rule**. If a user denies a tool in `~/.crab/config.toml`, no project-level, local-level, or CLI-level allow can bypass it. This is a security boundary.

### 4.4 MCP Servers Merge Example

`mcpServers` is a table keyed by server name; it follows the general deep-merge rule, meaning **fields inside the same-named server are merged recursively** rather than replaced wholesale.

```toml
# ~/.crab/config.toml (user layer)
[mcpServers.github]
url = "https://mcp.github.com"

# $PWD/.crab/config.toml (project layer)
[mcpServers.github]
auth = "oauth"
```

Merge result:

```toml
[mcpServers.github]
url = "https://mcp.github.com"
auth = "oauth"
```

When the same field appears in multiple layers (e.g., both set `url`), later source wins — the project layer wins over the user layer. Array fields (such as `args`) follow concat+dedup.

### 4.5 Resolution Pipeline

```rust
pub fn resolve() -> Result<Config> {
    // file layer: accumulate from lowest to highest priority
    let mut config_value = defaults_as_value();

    // plugin layer: merge each enabled plugin's config.json in alphabetical order.
    // JSON → toml::Value conversion happens inside enabled_plugin_configs().
    for plugin_cfg in enabled_plugin_configs()? {
        merge_toml_values(&mut config_value, plugin_cfg);
    }

    for source in [user, project, local, cli_config_file] {
        if let Some(v) = source.load()? {
            merge_toml_values(&mut config_value, v);
        }
    }

    // runtime layer: env first, then CLI flags
    let mut runtime_value = Value::Table(Default::default());
    merge_toml_values(&mut runtime_value, env_to_value()?);
    merge_toml_values(&mut runtime_value, cli_flags_to_value()?);

    // cross-layer: runtime overrides file
    merge_toml_values(&mut config_value, runtime_value);

    // single deserialization into the business struct
    Ok(config_value.try_into()?)
}
```

Business code consumes a flat `Config`; it does not know which layer any given field came from. Observability tooling (e.g. `crab config show --source`) can track per-key origin in an auxiliary map during the merge — the hot path still produces a single value.

---

## 5. CLI Flag Classification

Not every CLI flag belongs in the runtime layer. Crab classifies flags by semantics:

| Flag pattern | Example | Treated as |
|--------------|---------|-----------|
| Whole-file injection | `--config /path/to.toml` | **file layer**, position 6 (above `local`) |
| Field override | `--model opus`, `--permission-mode acceptEdits` | **runtime layer**, mapped to the corresponding `Value` path |
| Dotted override | `-c permissions.allow='["Bash(git:*)"]'` | **runtime layer**, parsed as TOML and inserted at the nested path |
| Loader control | `--config-dir <dir>`, `--cwd <dir>` | **Not merged** — changes which files are loaded |

The dotted override form mirrors `codex -c key.path=value` and lets users tweak any field without writing a full config file. Values are parsed as TOML first and fall back to strings.

---

## 6. The `env` Field

The `Config` struct carries an `env: HashMap<String, String>` field that mirrors Claude Code's behavior. This looks like it contradicts the "secrets must not live in `Config`" rule, but the semantics are different:

```toml
# ~/.crab/config.toml
[env]
ANTHROPIC_BASE_URL = "https://proxy.corp.internal/api"
HTTP_PROXY = "http://proxy:8080"
```

**Interpretation.** The `env` field declares environment variables to **inject into child processes Crab spawns**, and to set on Crab's own process at startup. It is not a business field read by the agent — it is a small startup-script fragment expressed as data.

**Implication for secrets.** A user may place `ANTHROPIC_AUTH_TOKEN` under `[env]`. When Crab starts, it exports the variable, and the `auth` module reads it from the OS environment exactly as if it had been set by the shell. The secret therefore flows through the OS env path, not through a business `Config` field. This matches how Claude Code users configure third-party proxies today.

**What is still removed.** Any direct secret field on `Config` (e.g., the current `apiKey: Option<String>`) is removed. The only settings-level hooks into authentication that remain are the indirect `env` channel and the `apiKeyHelper` pointer.

---

## 7. File Formats

| File | Format | Rationale |
|------|--------|-----------|
| `config.toml`, `config.local.toml` | **TOML** | Human-edited, supports comments, aligns with `-c key=value` grammar, matches codex |
| `auth/tokens.json` | **JSON** | Machine-written (OAuth flow) and machine-read; no comments needed; discourages hand-editing; compact |
| `plugins/<name>/plugin.json`, `plugins/<name>/config.json` | **JSON** | Everything under a plugin directory is machine-distributed and not hand-edited; keeping it the same format as the manifest is consistent |

This asymmetry is deliberate. TOML's ergonomics are valuable for files users edit; they are irrelevant for credential blobs and plugin artifacts the agent never asks humans to open.

---

## 8. Directory Layout

```
$CRAB_CONFIG_DIR/                (default: ~/.crab/)
  config.toml                    # user layer (chain position 3)
  auth/
    tokens.json                  # OAuth token (independent, 0600;
                                 #   future: split per provider)
  sessions/                      # conversation history
  commands/                      # user-level slash commands
  agents/                        # user-level subagent definitions
  hooks/                         # user-level hooks
  plugins/                       # installed plugins (first version mirrors Anthropic layout)
    <plugin-name>/
      plugin.json                # plugin manifest (identity, version, capabilities)
      config.json                # plugin's contributed defaults (optional, chain position 2, JSON)
      skills/                    # plugin-provided skills (markdown)
      commands/                  # plugin-provided slash commands
      agents/                    # plugin-provided subagents
  logs/                          # local-only telemetry

$PWD/.crab/
  config.toml                    # project layer (chain position 4, committed)
  config.local.toml              # local layer (chain position 5, gitignored)
  commands/
  agents/
  hooks/

$PWD/
  AGENTS.md                      # project-level memory (CLAUDE.md also
                                 #   recognized for CC migrators)
```

Caches live in the OS-standard per-user cache directory (Linux: `$XDG_CACHE_HOME/crab/` or `~/.cache/crab/`; macOS: `~/Library/Caches/crab/`; Windows: `%LOCALAPPDATA%\crab\cache\`). They are not under `$CRAB_CONFIG_DIR` because the cache is safe to delete and configuration is not.

---

## 9. Precedence Summary

Reading the merge chain from bottom to top:

```
runtime layer (this invocation only)
+------------------------------+
| CLI flag (--model, -c x=y)  |  runtime highest
| env (CRAB_*, non-secret)    |
+------------------------------+
              ^
file layer (persisted)
+--------------------------------------+
| --config <file>                      |  file highest
| .crab/config.local.toml              |
| .crab/config.toml                    |
| ~/.crab/config.toml                  |
| plugins/<name>/config.json (alpha)   |
| defaults (compiled-in)               |  lowest overall
+--------------------------------------+
```

**Within-layer rule**: later source wins; tables deep-merge; arrays concat+deduplicate.
**Cross-layer rule**: runtime overrides file.
**Out-of-chain**: auth credentials, secret env vars, keychain entries, and `apiKeyHelper` output are read by the `auth` module independently and never appear in the merged `Config`.

---

## 10. Error Handling & Edge Cases

Crab's philosophy aligns with CCB: **fall back gracefully, never crash on malformed config**. Bad data is logged as a warning; the user gets a running tool instead of a dead one.

### 10.1 Malformed Files

| Failure | Behavior | Reference (CCB) |
|---------|----------|-----------------|
| `config.toml` has a TOML/JSON parse error | That source returns nothing (treated as empty). Warning logged. Subsequent layers unaffected. | `settings.ts:213`, `json.ts:31-40` |
| A single invalid permission rule | Only the bad rule is filtered out; the rest of that layer is applied. | `validation.ts:224-265` |
| A plugin's `config.json` is unreadable or fails schema | That plugin is skipped with a warning; other plugins continue to load. | (same pattern as above) |
| Schema validation fails on a specific field | The field is discarded with a warning; unrelated valid fields in the same file are retained. | — |

**No crash, no hard error on startup.** The crab process always starts with at least the compiled-in defaults, even if every file on disk is corrupted.

### 10.2 First Run (Bootstrap)

- On a fresh system (no `~/.crab/config.toml`), Crab runs with the compiled-in defaults.
- Crab **does not auto-create** any config file and **does not prompt** the user to create one.
- The `~/.crab/` directory and `config.toml` are created **only** when the user first runs a mutating command — `crab config set …`, `crab auth login`, etc.
- This matches CCB (`envUtils.ts:10`, `settings.ts:309-317`): new installs cost the user zero actions until they actually want to change something.

### 10.3 OS-Specific Paths

Crab uses the same path convention on all platforms (aligned with CCB `envUtils.ts:7-14`):

```
$CRAB_CONFIG_DIR  ?? join(homedir(), ".crab")
```

- Linux / macOS: `~/.crab/`
- Windows: `C:\Users\<Name>\.crab\` (from `homedir()`, which returns `%USERPROFILE%` — **not** `%APPDATA%`)

Rationale: cross-platform consistency. Users migrating between machines or OSes find their config in the same relative location. `CRAB_CONFIG_DIR` overrides everywhere.

### 10.4 `.gitignore` Auto-Maintenance

When Crab writes to `config.local.toml` for the first time, it attempts to register the file in `.gitignore`. The check is two-layered (aligned with CCB `gitignore.ts:62-83`):

1. Run `git check-ignore <path>` — if Git already ignores the file (via any `.gitignore` in the repo, or via a global `~/.config/git/ignore` rule), skip.
2. Inspect the global gitignore for a matching `**/.crab/config.local.toml` entry; if present, skip.

Only when both checks pass does Crab append an entry to the project's `.gitignore`. This makes repeated writes idempotent and avoids duplicating rules for users who already have global rules set up.

---

## 11. Write-Back (Config Mutation)

Crab can mutate persisted config via `crab config set <key> <value>` (and analogous commands). The mechanism mirrors CCB's `updateSettingsForSource()` (`settings.ts:416-524`) with one upgrade.

| Aspect | Behavior |
|--------|----------|
| Writable layers | `user`, `project`, `local` |
| Non-writable layers | `defaults`, `plugin`, `--config` (ephemeral), runtime (env/flag) |
| Target layer selection | `--global` → user; `--local` → local; default → project |
| Comment preservation | **Yes, via `toml_edit`** — original comments, key order, and formatting are preserved through round-trip. This is strictly better than CCB, whose JSON serializer discards comments. |
| Directory creation | `.crab/` created on demand if absent |
| `.gitignore` hook | Triggered automatically when first writing `config.local.toml` (see 10.4) |
| Post-write validation | The resulting file is re-parsed and re-validated against the schema; if validation fails, the write is rolled back and an error is surfaced — this prevents leaving a broken config on disk |

**Secret fields rejected at write time.** `crab config set apiKey …` (or any path the schema marks as secret-adjacent) is rejected. Secrets must go through the auth module or `[env]`, never directly into a persisted `Config` field.

---

## 12. Schema & Validation

### 12.1 Schema Location & Strategy

- **Asset path**: `crates/config/assets/config.schema.json`
- **Maintenance**: **hand-written, with schemars as an optional scaffolder.** Humans maintain `description`, `examples`, `default`, `pattern`, etc. For large structural changes, running `cargo run --example gen-schema > .raw` (see `crates/config/examples/gen-schema.rs`, which depends on `#[derive(JsonSchema)]` on `Config`) produces a mechanical skeleton that a human then ports into the real schema file. The `.raw` file is not committed. **Not part of CI, not auto-committed.**
- **Embedding**: `include_str!("../assets/config.schema.json")` at compile time.
- **Distribution**: committed to git; external tools (IDEs, CI, third-party validators) can reference it directly.
- **Defaults**: every leaf field in the schema carries the JSON Schema `default` keyword, kept in sync with `Config::default()`. Rust is the runtime source of truth; the schema's `default` serves as **documentation and IDE hints** only (`jsonschema` crate does not automatically apply defaults, only validates). Dynamic defaults (e.g., `env::current_dir()`, platform detection) are not declared in the schema — Rust computes them. Drift is caught by the `rust_defaults_match_schema_defaults` test (see 12.5).

Reasons for hand-written over auto-generated:

1. Schema changes infrequently (only when fields are added/removed); manual cost is below tooling cost.
2. Hand-written schemas deliver better `description` / `enum` / `examples` / `default` quality than `schemars` derive.
3. Schema is a user-facing contract and should live independently of the Rust struct's internal structure.
4. Matches Rust CLI ecosystem convention (helix, alacritty, wrangler, etc.).

### 12.2 Load-Time Validation Pipeline

```rust
use jsonschema::JSONSchema;

const SCHEMA: &str = include_str!("../assets/config.schema.json");

fn load_and_validate(path: &Path) -> Result<Config> {
    // 1. Parse TOML
    let raw: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;

    // 2. Convert to serde_json::Value (accepted by the jsonschema crate)
    let json: serde_json::Value = serde_json::to_value(&raw)?;

    // 3. Validate against schema; on failure, emit errors with JSON Pointer paths
    let schema: serde_json::Value = serde_json::from_str(SCHEMA)?;
    let compiled = JSONSchema::compile(&schema)?;
    if let Err(errors) = compiled.validate(&json) {
        bail!(format_errors(errors, path));
    }

    // 4. Deserialize only after validation passes
    Ok(raw.try_into()?)
}
```

### 12.3 Error Types Caught

| Source | Catches |
|--------|---------|
| `serde` deserialization | Type mismatches, missing required fields, unknown fields (in strict mode) |
| `jsonschema` validation | Illegal enum values, pattern mismatches, array length constraints, numeric ranges, compound constraints (`oneOf`/`allOf`) |

Example: `permissions.allow = ["Bash(git:*)"]` passes serde (it's just a `String`), but the schema's `pattern` constraint rejects malformed rules like `permissions.allow = ["Bash git:*"]`.

### 12.4 Recommended Validation Crates

| Crate | Status | Recommendation |
|-------|--------|----------------|
| `jsonschema` | Mature; supports draft 7 / 2019-09 / 2020-12 | **Preferred** |
| `boon` | Newer, reportedly faster | Alternative |
| `valico` | Maintenance inactive | Not recommended |

### 12.5 Preventing Schema Drift via Fixture Tests

No automated CI diff is needed. A handful of representative `config.toml` examples live under `crates/config/tests/fixtures/`; tests require them to pass the schema:

```rust
#[test]
fn example_configs_conform_to_schema() {
    for path in glob("tests/fixtures/config_examples/*.toml") {
        load_and_validate(&path).expect("example config must pass schema");
    }
}

#[test]
fn rust_defaults_match_schema_defaults() {
    let from_rust = serde_json::to_value(&Config::default()).unwrap();
    let from_schema = extract_defaults_from_schema(SCHEMA);
    // Every `default` declared in the schema must equal Rust's default value.
    // Rust may have "derived" default fields that the schema does not declare
    // (unidirectional consistency).
    assert_schema_defaults_match(&from_rust, &from_schema);
}
```

When new fields are added, the fixtures and schema defaults are updated in the same commit — any oversight is caught by these tests.

### 12.6 IDE Integration

Add a pragma at the top of `config.toml` (taplo, VSCode's Even Better TOML extension, helix, and others recognize it):

```toml
#:schema https://raw.githubusercontent.com/<org>/crab/main/crates/config/assets/config.schema.json

provider = "anthropic"
model = "claude-opus-4-7"
```

For local-only installs, point at a local path:

```toml
#:schema ~/.crab/config.schema.json
```

(Crab may copy the schema to `~/.crab/config.schema.json` on `crab auth login` or first run as a convenience.)

---

## 13. Design Decisions Recap

1. **Two explicit layers, not a flat N-source chain.** Persisted intent and ephemeral overrides have different lifecycles; merging them separately makes the model honest.
2. **Merge at the `toml::Value` layer, not the struct layer.** Adding a field requires no merge-logic change; array concat+dedup is defined once.
3. **Codex-style `merge_toml_values` instead of `figment` / `config-rs`.** Crab's required semantics (array concat+dedup, runtime/file split) would need custom providers and customizers in those frameworks anyway; a 20-line recursive function is smaller and more transparent.
4. **Secrets never enter `Config`.** The only settings-level paths into authentication are the indirect `env` channel and the `apiKeyHelper` pointer; direct secret fields are rejected at write time.
5. **TOML for human-edited files, JSON for machine-written files.** Format follows the reader, not uniformity.
6. **`CRAB_CONFIG_DIR` supported from day one.** Enables containers, tests, and multi-identity scenarios without layer-rule changes.
7. **Hand-written schema, not auto-generated.** Higher quality, low churn, matches ecosystem convention; drift is caught by fixture tests.
8. **Plugin layer borrows the Anthropic directory layout.** First version follows CCB's convention (`plugins/<name>/plugin.json` + `skills/` + `commands/` + `agents/`), enabling direct consumption of the Anthropic plugin repo. The `config.json` contribution channel is Crab's own addition because Crab's WASM/Skill plugin model cannot register settings via host-side code the way CCB's TypeScript plugins can.
9. **Plugin directories use JSON; the main chain uses TOML.** Plugin content is machine-distributed, so JSON (consistent with `plugin.json`) is the right format; the main chain is human-edited, so TOML wins. At load time, plugin JSON is converted to `toml::Value` to join the main merge chain. Plugins merge in alphabetical order (more deterministic than CCB's registration order) and cannot set `[env]` (security constraint).
10. **The schema is both a contract and a defaults document.** `Config::default()` is the runtime source; the schema's `default` keyword on every leaf field lets IDEs, documentation, and users see the same defaults — drift is guarded by the `rust_defaults_match_schema_defaults` test. Dynamic defaults stay Rust-only; they are not declared in the schema.
11. **Graceful-degradation error handling.** Malformed files, bad plugin configs, and schema-invalid fields are logged and skipped, not fatal. The user always gets a running tool, even with everything on disk corrupted. Aligns with CCB.
12. **Permissions: deny always wins.** No allow rule from any source, at any level, can override a deny. This is a non-negotiable security boundary.
13. **Zero-config first run.** Nothing is created on disk until the user first mutates config. No auto-generated templates, no prompts.
14. **`~/.crab/` on all platforms.** Windows does not follow `%APPDATA%` convention; consistency with Linux/macOS wins. `CRAB_CONFIG_DIR` overrides for edge cases.
15. **Write-back preserves TOML comments.** Via `toml_edit`, Crab retains user-authored comments and key order across mutations — strictly better than CCB's JSON-serializer round-trip which drops them.
