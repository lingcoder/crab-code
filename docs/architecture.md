# Crab Code Architecture


> Updated: 2026-04-27
> Changelog: Tool trait gains `is_concurrency_safe(input)` for input-dependent parallelism; PermissionMode expanded to 6 variants (+AcceptEdits/DontAsk/Plan); engine query loop adds PTL recovery, max-output-tokens retry, streaming fallback, plan model routing; plugin hooks gain `Stop` trigger and `Retry` action; multi-layer compaction pipeline (microcompact → LLM summary → heuristic fallback → compact boundary).

---

## 1. Architecture Overview

### Four-Layer Architecture

| Layer | Crate | Responsibility |
|-------|-------|----------------|
| **Layer 4** Entry Layer | `cli` `daemon` | CLI entry point (clap), background daemon |
| **Layer 3** Engine Layer | `agent` `engine` `session` `tui` `remote` | Query loop, multi-agent orchestration, session state, terminal UI, remote-control WebSocket server + client |
| **Layer 2** Service Layer | `api` `tools` `mcp` `acp` `fs` `process` `sandbox` `ide` `skill` `plugin` `memory` `swarm` `telemetry` `job` | Tool system, MCP stack, ACP server, LLM clients, file/process/sandbox, IDE client, skill system, plugins, persistent memory, multi-agent infrastructure, telemetry, unified job scheduling |
| **Layer 1** Foundation Layer | `core` `common` `config` `auth` | Domain model, layered config, authentication |

> Dependency direction: upper layers depend on lower layers; reverse dependencies are prohibited. `core` defines the `Tool` trait to avoid circular dependencies between `tools` and `agent`. See §5.3 for inner-layer rules (aggregator vs leaf service; Layer 3 Event-only control flow).

### Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────────────┐
│                         Layer 4: Entry Layer                             │
│  ┌──────────────┐                                    ┌────────────────┐  │
│  │  crates/cli  │                                    │ crates/daemon  │  │
│  │  clap + TUI  │                                    │  headless svc  │  │
│  └──────┬───────┘                                    └────────┬───────┘  │
├─────────┼──────────────────────────────────────────────────────┼────────┤
│         │                 Layer 3: Engine Layer                │        │
│  ┌──────▼──────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ ┌───────▼─────┐ │
│  │    agent    │ │  engine  │ │ session  │ │  tui   │ │   remote    │ │
│  │ orchestra + │ │ raw loop │ │ state +  │ │ ratatui│ │ WS server + │ │
│  │ swarm +     │ │ stream + │ │ compact  │ │ views  │ │ client +    │ │
│  │ proactive   │ │ tooluse  │ │ memory   │ │        │ │ crab-proto  │ │
│  └────┬────────┘ └────┬─────┘ └────┬─────┘ └───┬────┘ └──────┬──────┘ │
├───────┼────────────────┼─────────────┼───────────┼────────────┼────────┤
│       │                │  Layer 2: Service Layer │            │        │
│  ┌────▼─────┐ ┌────────▼──┐ ┌────────┐ ┌────────▼─┐ ┌────────▼────┐  │
│  │  tools   │ │   mcp     │ │  api   │ │ telemetry│ │   plugin    │  │
│  │ aggreg   │ │ JSON-RPC  │ │ Llm-   │ │  local   │ │ hooks+WASM+ │  │
│  │ 40+ buil │ │ +streams  │ │ Backend│ │  only    │ │ skill↔mcp   │  │
│  └──┬──┬──┬─┘ └───────────┘ └────────┘ └──────────┘ └─────────────┘  │
│     │  │  │                                                           │
│  ┌──▼┐┌▼─┐┌▼──────┐ ┌──────────┐ ┌────────┐ ┌───────┐ ┌──────┐ ┌───┐ │
│  │fs ││pr││sandbox│ │  remote  │ │  ide   │ │ skill │ │memory│ │.. │ │
│  │   ││oc││seat+  │ │claude.ai │ │IDE MCP │ │ reg + │ │store │ │   │ │
│  │   ││  ││landlk+│ │trigger + │ │ client │ │bundled│ │ rank │ │   │ │
│  │   ││  ││wsl    │ │ schedule │ │        │ │       │ │ age  │ │   │ │
│  └───┘└──┘└───────┘ └──────────┘ └────────┘ └───────┘ └──────┘ └───┘ │
├───────────────────────────────────────────────────────────────────────┤
│                       Layer 1: Foundation Layer                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐               │
│  │   core   │  │  common  │  │  config  │  │   auth   │               │
│  │Domain    │  │Error +   │  │Multi-    │  │OAuth +   │               │
│  │model +   │  │utility   │  │layer     │  │Keychain  │               │
│  │Tool trait│  │path/text │  │config    │  │          │               │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘               │
└───────────────────────────────────────────────────────────────────────┘
```

### Mapping to Claude Code's Five-Layer Architecture

| Claude Code (TS) | Path | Crab Code (Rust) | Notes |
|-------------------|------|-------------------|-------|
| **Entry Layer** entrypoints/ | `cli.tsx` `main.tsx` | `cli` `daemon` | CC uses React/Ink for rendering; Crab uses ratatui |
| **Command Layer** commands/ | `query.ts` `QueryEngine.ts` `coordinator/` | `engine` + `agent` | CC's `query.ts` ↔ crab `engine`; `QueryEngine.ts` ↔ `agent`; domain-pure swarm infra in `crates/swarm/` |
| **Tool Layer** tools/ | 52 Tool directories | `tools` + `mcp` | CC mixes tools and MCP in `services/`; Crab separates them |
| **Service Layer** services/ | `api/` `mcp/` `oauth/` `compact/` `memdir/` | `api` `mcp` `acp` `auth` `skill` `plugin` `memory` `telemetry` `sandbox` `ide` `job` | CC's service layer is flat; Crab splits by responsibility. `memdir/` → `memory`; CC `utils/sandbox/` → `sandbox`; CC IDE MCP client surface → `ide`; ACP server → `acp`; unified scheduling → `job` |
| **Bridge Layer** bridge/ | `bridgeMain.ts` `replBridge.ts` | `remote` (server + client) | CC's `src/bridge/` (inbound server) + `src/remote/` (outbound client) both land in crates/remote, which owns the full crab-proto stack (server + client + wire types, mirroring crab-mcp) |
| **Foundation Layer** utils/ types/ | `Tool.ts` `context.ts` | `core` `common` `config` | CC scatters types across files; Crab centralizes them in `core` |

### Core Design Philosophy

1. **core has zero I/O** -- Pure data structures and trait definitions, reusable by any frontend (CLI/GUI/WASM)
2. **Message loop driven** -- Everything revolves around the query loop: user input -> API call -> tool execution -> result return
3. **Workspace isolation** -- 23 library crates with orthogonal responsibilities (plus 2 bin + xtask = 26 total); incremental compilation only triggers on changed parts
4. **Feature flags control dependencies** -- No Bedrock? AWS SDK is not compiled. No WASM? wasmtime is not compiled.

---

## 2. Why Rust

### 2.1 Go vs Rust Comparison

| Dimension | Go | Rust | Conclusion |
|-----------|-----|------|------------|
| **Development speed** | Fast, low learning curve | 2-3x slower, lifetime/ownership friction | Go wins |
| **CLI ecosystem** | cobra is mature | clap is equally mature | Tie |
| **TUI** | Charm (bubbletea) is excellent | ratatui is excellent | Tie |
| **GUI extensibility** | Good (Wails WebView-based, fyne/gio native) | **Strong** (Tauri 2.0 desktop + mobile) | Slight Rust edge |
| **WASM** | Go->WASM ~10MB+, poor performance | **First-class citizen**, small output, native perf | **Rust wins** |
| **FFI/cross-language** | cgo has performance penalty | **Zero-overhead FFI**, native C ABI | **Rust wins** |
| **AI/ML ecosystem** | Few bindings | candle, burn, ort (ONNX) | **Rust wins** |
| **Serialization** | encoding/* is adequate | serde is **dominant** | **Rust wins** |
| **Compile speed** | 10-30s | 5-15min | Go wins |
| **Cross-compilation** | Extremely simple | Moderate (needs target toolchain) | Go wins |
| **Hiring** | Larger developer pool | Smaller developer pool | Go wins |

### 2.2 Five Core Reasons for Choosing Rust

1. **High ceiling for future expansion** -- CLI -> Tauri desktop -> browser WASM -> mobile, 100% core logic sharing
2. **Tauri ecosystem** -- Mainstream Electron alternative, 20-30MB memory vs 150MB+, 5-15MB bundle vs 100MB+
3. **Third-party library quality** -- serde, tokio, ratatui, clap are all top-tier implementations in their domains
4. **Local AI inference** -- Future integration of local models via candle/burn, no cgo bridging needed
5. **Plugin sandbox** -- wasmtime itself is written in Rust; WASM plugin system is a natural fit

### 2.3 Expected Performance Comparison

| Metric | TypeScript/Bun | Rust | Factor |
|--------|---------------|------|--------|
| **Cold start** | ~135ms | ~5-10ms | **15-25x** |
| **Memory usage (idle)** | ~80-150MB | ~5-10MB | **10-20x** |
| **API streaming** | Baseline | ~Equal | 1x (I/O bound) |
| **Terminal UI rendering** | Slower (React overhead) | Fast (ratatui zero-overhead) | **3-5x** |
| **JSON serialization** | Fast (V8 built-in) | Fastest (serde zero-copy) | **2-3x** |
| **Binary size** | ~100MB+ (including runtime) | ~10-20MB | **5-10x** |

---

## 3. Core Library Alternatives

28 TS -> Rust mappings in total, grouped by function. Versions are pinned in `Cargo.toml` and omitted here to avoid staleness.

### 3.1 CLI / UI

| # | Function | TypeScript Original | Rust Alternative | Docs |
|---|----------|---------------------|------------------|------|
| 1 | CLI framework | Commander.js | clap (derive) | [docs.rs/clap](https://docs.rs/clap) |
| 2 | Terminal UI | React/Ink | ratatui + crossterm | [ratatui.rs](https://ratatui.rs) |
| 3 | Terminal styling | chalk | crossterm Style | [docs.rs/crossterm](https://docs.rs/crossterm) |
| 4 | Markdown rendering | marked | pulldown-cmark | [docs.rs/pulldown-cmark](https://docs.rs/pulldown-cmark) |
| 5 | Syntax highlighting | highlight.js | syntect | [docs.rs/syntect](https://docs.rs/syntect) |
| 6 | Fuzzy search | Fuse.js | nucleo ** | [docs.rs/nucleo](https://docs.rs/nucleo) |

### 3.2 Network / API

| # | Function | TypeScript Original | Rust Alternative | Docs |
|---|----------|---------------------|------------------|------|
| 7 | HTTP client | axios/undici | reqwest | [docs.rs/reqwest](https://docs.rs/reqwest) |
| 8 | WebSocket | ws | tokio-tungstenite | [docs.rs/tokio-tungstenite](https://docs.rs/tokio-tungstenite) |
| 9 | Streaming SSE | Anthropic SDK | eventsource-stream | [docs.rs/eventsource-stream](https://docs.rs/eventsource-stream) |
| 10 | OAuth | google-auth-library | oauth2 | [docs.rs/oauth2](https://docs.rs/oauth2) |

### 3.3 Serialization / Validation

| # | Function | TypeScript Original | Rust Alternative | Docs |
|---|----------|---------------------|------------------|------|
| 11 | JSON | Built-in JSON | serde + serde_json | [serde.rs](https://serde.rs) |
| 12 | YAML | yaml | serde_yaml_ng | [docs.rs/serde_yaml_ng](https://docs.rs/serde_yaml_ng) |
| 13 | TOML | -- | toml | [docs.rs/toml](https://docs.rs/toml) |
| 14 | Schema validation | Zod | schemars | [docs.rs/schemars](https://docs.rs/schemars) |

> Note: `serde_yaml_ng` is the community successor to the archived `serde_yaml` (dtolnay). It is the correct modern choice.

### 3.4 File System / Search

| # | Function | TypeScript Original | Rust Alternative | Docs |
|---|----------|---------------------|------------------|------|
| 15 | Glob | glob | globset | [docs.rs/globset](https://docs.rs/globset) |
| 16 | Grep/search | ripgrep bindings | grep-searcher + grep-regex + ignore | [docs.rs/grep-searcher](https://docs.rs/grep-searcher) |
| 17 | Gitignore | -- | ignore | [docs.rs/ignore](https://docs.rs/ignore) |
| 18 | File watching | chokidar | notify | [docs.rs/notify](https://docs.rs/notify) |
| 19 | Diff | diff | similar | [docs.rs/similar](https://docs.rs/similar) |
| 20 | File locking | proper-lockfile | fd-lock | [docs.rs/fd-lock](https://docs.rs/fd-lock) |

> Note on #16: ripgrep is built from a family of crates by BurntSushi: `grep-searcher` (streaming search with binary detection), `grep-regex` (regex adapter), `grep-matcher` (abstract trait), `ignore` (gitignore-aware walker), `regex` (pattern engine). We use the full `grep-searcher` + `grep-regex` + `ignore` stack — the same core as the `rg` command line tool.

### 3.5 System / Process

| # | Function | TypeScript Original | Rust Alternative | Docs |
|---|----------|---------------------|------------------|------|
| 21 | Subprocess | execa | tokio::process | [docs.rs/tokio](https://docs.rs/tokio) |
| 22 | Process tree | tree-kill | sysinfo | [docs.rs/sysinfo](https://docs.rs/sysinfo) |
| 23 | System directories | -- | directories | [docs.rs/directories](https://docs.rs/directories) |
| 24 | Keychain | Custom impl | keyring | [docs.rs/keyring](https://docs.rs/keyring) |

### 3.6 Observability / Cache

| # | Function | TypeScript Original | Rust Alternative | Docs |
|---|----------|---------------------|------------------|------|
| 25 | OpenTelemetry | @opentelemetry/* | opentelemetry | [docs.rs/opentelemetry](https://docs.rs/opentelemetry) |
| 26 | Logging/tracing | console.log | tracing | [docs.rs/tracing](https://docs.rs/tracing) |
| 27 | LRU cache | lru-cache | lru | [docs.rs/lru](https://docs.rs/lru) |
| 28 | Error handling | Error class | thiserror + anyhow | [docs.rs/thiserror](https://docs.rs/thiserror) |

---

## 4. Workspace Project Structure

### 4.1 Complete Directory Tree

```
crab-code/
├── Cargo.toml                         # workspace root
├── Cargo.lock
├── rust-toolchain.toml                # pinned toolchain
├── rustfmt.toml / clippy.toml         # lint config
│
├── crates/                            # 25 crates (details: §6.x)
│   │
│   │  # ── Layer 1: Foundation ──
│   ├── common/                        # shared utils (path/text/id/debug)
│   ├── core/                          # domain model, Tool trait, permission/, Event
│   ├── config/                        # multi-layer config load/merge/write
│   ├── auth/                          # OAuth + keychain + cloud credential chain
│   │
│   │  # ── Layer 2: Service (leaves) ──
│   ├── api/                           # LLM clients (anthropic/ + openai/ + bedrock/vertex)
│   ├── mcp/                           # MCP facade (client + server/ + transport/ + auth/)
│   ├── acp/                           # ACP stdio server (Zed/Neovim/Helix)
│   ├── fs/                            # glob, grep, diff, watch, lock, symlink
│   ├── process/                       # subprocess spawn, PTY, tree kill, signal
│   ├── sandbox/                       # Sandbox trait + backend/ (seatbelt/landlock/windows/noop)
│   ├── ide/                           # IDE MCP client + quirks/ (vscode/jetbrains/wsl)
│   ├── skill/                         # skill registry + bundled/ skills
│   ├── memory/                        # persistent memory store + ranking + AGENTS.md
│   ├── swarm/                         # multi-agent infra (bus/mailbox/roster/task/backend/)
│   ├── telemetry/                     # tracing + metrics + cost + OTLP export
│   ├── job/                           # unified scheduling (one-shot/interval/cron)
│   │
│   │  # ── Layer 2: Service (aggregators) ──
│   ├── tools/                         # tool registry + executor + builtin/ (45+ tools + computer_use/)
│   ├── plugin/                        # hook system + WASM runtime + MCP↔skill bridge
│   │
│   │  # ── Layer 3: Engine ──
│   ├── engine/                        # raw query loop + streaming + tool orchestration
│   ├── session/                       # conversation state + compaction + history + cost
│   ├── agent/                         # orchestrator (teams/ + coordinator/ + system_prompt/ + slash_commands/)
│   ├── tui/                           # ratatui terminal UI (components/ + keybindings/ + vim/ + theme/)
│   ├── remote/                        # crab-proto WS (protocol/ + auth/ + client/ + server/)
│   │
│   │  # ── Layer 4: Entry ──
│   ├── cli/                           # binary entry + clap + commands/
│   └── daemon/                        # headless binary + IPC + session pool
│
└── xtask/                             # build helpers (bench/ci/coverage/release)
```

> Per-file details for each crate are in the §6.x sections below. This tree intentionally omits leaf `.rs` files to avoid maintenance drift.

### 4.2 Crate Statistics

| Type | Count | Notes |
|------|-------|-------|
| Library crate | 23 | `crates/*` — includes `swarm`, `ide`, `memory`, `engine`, `remote`, `sandbox`, `acp`, `job` |
| Lib+Bin crate | 1 | `crates/daemon` (lib.rs + main.rs) |
| Binary crate | 1 | `crates/cli` |
| Helper crate | 1 | `xtask` |
| **Total** | **26** | -- |
| Total modules | ~300 | Across 24 library crates |
| Total tests | ~2700 | `cargo test --workspace` |


---

## 5. Crate Dependency Graph

### 5.1 Dependency Diagram

```
                               ┌────────────┐
                               │    cli     │ depends on all crates
                               └──┬───┬───┬─┘
              ┌───────────────────┘   │   └──────────────────┐
              │                       │                      │
        ┌─────▼────┐  ┌────────┐  ┌───▼────┐  ┌────────┐  ┌──▼─────┐
        │   tui    │  │ agent  │  │ engine │  │ remote │  │ daemon │
        └──────┬───┘  └──┬──┬──┘  └───┬────┘  └───┬────┘  └────┬───┘
               │         │  │         │           │            │
               │    ┌────▼──▼───┐     │           │            │
               │    │  session  │◄────┼───────────┴────────────┤
               │    └────┬──────┘     │                        │
               └────────►│            │                        │
                         ▼            ▼                        ▼
                    ┌────────────────────────────────────────────┐
                    │  tools (Layer 2 aggregator)                │
                    └──┬────┬────┬────┬────┬────┬────┬────┬──────┘
                       │    │    │    │    │    │    │    │
                  ┌────▼┐ ┌─▼─┐ ┌▼──┐ ┌▼─┐ ┌▼──┐ ┌▼──┐ ┌▼───┐ ...
                  │ fs  │ │pr │ │mcp│ │sb│ │rem│ │ide│ │skil│
                  │     │ │oc │ │   │ │   │ │   │ │   │ │    │
                  └─────┘ └───┘ └───┘ └──┘ └───┘ └───┘ └────┘
                                    │
                              ┌─────▼────┐   ┌────────┐   ┌────────┐
                              │   api    │   │ plugin │   │ memory │
                              └─────┬────┘   └────┬───┘   └────┬───┘
                                    │             │            │
                              ┌─────▼─────────────▼────────────▼──┐
                              │              config               │
                              └──────────────────┬────────────────┘
                              ┌──────┐    ┌──────▼──────┐
                              │ auth │◄───┤   config    │
                              └──┬───┘    └──────┬──────┘
                                 │               │
                              ┌──▼───────────────▼──┐
                              │        core         │
                              └──────────┬──────────┘
                                         │
                              ┌──────────▼──────────┐
                              │        common       │
                              └─────────────────────┘

                         ┌────────────┐
                         │ telemetry  │ ← sidecar, optional dep for any crate
                         └────────────┘
```

Legend: `sb` = sandbox, `rem` = remote, `skil` = skill, `proc` = process.

### 5.2 Dependency Manifest (Bottom-Up)

| # | Crate | Internal Dependencies | Notes |
|---|-------|-----------------------|-------|
| 1 | **common** | — | Zero-dependency foundation |
| 2 | **core** | — | Pure domain model |
| 3 | **config** | core, common | Layered merge |
| 4 | **auth** | common, core, config | Credential chain |
| 5 | **api** | core, config, auth | LlmBackend + Anthropic/OpenAI clients |
| 6 | **fs** | common, core | File system ops |
| 7 | **process** | common, core | Subprocess mgmt |
| 8 | **mcp** | core | MCP client/server |
| 9 | **telemetry** | common, core | Sidecar, optional |
| 10 | **sandbox** | core | Trait + platform backends (seatbelt/landlock/windows/noop) |
| 11 | **remote** | core, config, auth | crab-proto protocol + WS server + outbound client (inbound hinge for web/app/desktop entry points) |
| 12 | **acp** | core | Agent Client Protocol server (editor → crab, Zed/Neovim/Helix) |
| 13 | **ide** | core, mcp | Client to IDE-hosted MCP server (lockfile-based VSCode/JetBrains plugins) |
| 14 | **job** | core | Unified scheduler — one-shot / interval / cron |
| 15 | **skill** | core | Skill discovery + bundled definitions |
| 16 | **memory** | core, common | Persistent memory store + ranking + AGENTS.md parsing |
| 17 | **plugin** | core, config, mcp, process, skill | Hooks + WASM + skill↔mcp bridge |
| 18 | **tools** | core, config, fs, process, sandbox, mcp | Layer 2 aggregator; 40+ built-in tools |
| 19 | **swarm** | core | Multi-agent infrastructure: message bus, roster, task list, retry, backends |
| 20 | **session** | core, memory | Session + context compaction |
| 21 | **engine** | core, api, session, tools, plugin | Raw query loop (extracted from agent) |
| 22 | **agent** | common, core, config, engine, memory, session, tools, api, mcp, plugin, swarm, skill | Orchestrator + coordinator + proactive |
| 23 | **tui** | common, core, config, agent, memory | Terminal UI; receives tool state via `core::Event` |
| 24 | **cli** (bin) | All crates | Thin entry point (interactive) |
| 25 | **daemon** (lib+bin) | common, core | Headless composition root — hosts server-side protocols for web/app/desktop |

### 5.3 Dependency Direction Principles

```
Rule 1: Upper layer -> lower layer. Reverse dependencies are prohibited.

Rule 2: Layer 2 is sub-layered into aggregators and leaves.
  - Aggregators (tools, plugin) may depend on leaf services in the same layer.
  - Leaf services (fs, process, mcp, acp, api, sandbox, ide, job, skill,
    memory, swarm, telemetry) must NOT depend on each other.
  - Example: tools -> sandbox (OK); fs -> process (NOT OK).

Rule 3: core decouples via traits (Tool trait defined in core, implemented in tools).

Rule 4: telemetry is a sidecar; it does not participate in the main dependency chain.

Rule 5: cli/daemon only do assembly; they contain no business logic.

Rule 6: Layer 3 internal control flow goes via core::Event only.
  - agent/session/tui/remote/engine do not make direct method calls that trigger
    work in another Layer 3 crate.
  - Exception 1: remote and agent may WRAP engine (engine does not call back up).
  - Exception 2: agent and tui may READ session state (Conversation, costs) as a
    data consumer; read-only access is not considered control flow.
```

---

## 6. Detailed Crate Designs

### 6.1 `crates/common/` -- Shared Foundation

**Responsibility**: A pure utility layer with zero business logic; the lowest-level dependency for all crates

**Directory Structure**

```
src/
├── lib.rs                // re-exports utils + constants
├── constants.rs          // shared constants
└── utils/                // utility functions (no business semantics)
    ├── mod.rs
    ├── id.rs             // ULID generation
    ├── path.rs           // cross-platform path normalization
    ├── text.rs           // Unicode width, ANSI strip, Bidi handling
    ├── debug.rs          // debug categories, tracing init
    ├── argument_substitution.rs  // CLI argument variable expansion
    ├── binary_check.rs   // binary file detection
    └── ca_certs.rs       // CA certificate loading
```

**Core Types**

Note: `Error` and `Result<T>` live in `crates/core`, not common. Common is a pure utility layer with no error types.

```rust
// utils/text.rs
pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(strip_ansi(s).as_str())
}

pub fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    // Truncate by display width, handling CJK characters
    let mut width = 0;
    let mut result = String::new();
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max_width {
            break;
        }
        width += w;
        result.push(ch);
    }
    result
}

// path.rs
use std::path::{Path, PathBuf};

pub fn normalize(path: &Path) -> PathBuf {
    // Unify forward slashes, resolve ~, remove redundant ..
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .expect("failed to resolve home directory")
        .home_dir()
        .to_path_buf()
}
```

**Per-Crate Error Type Examples**

```rust
// crates/api/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: status={status}, message={message}")]
    Api { status: u16, message: String },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Core(#[from] crab_core::Error),
}

// crates/mcp/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP error: code={code}, message={message}")]
    Mcp { code: i32, message: String },

    #[error("transport error: {0}")]
    Transport(String),

    #[error(transparent)]
    Core(#[from] crab_core::Error),
}

// crates/tools/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool {name}: {message}")]
    Execution { name: String, message: String },

    #[error(transparent)]
    Core(#[from] crab_core::Error),
}

// crates/auth/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("auth error: {message}")]
    Auth { message: String },

    #[error(transparent)]
    Core(#[from] crab_core::Error),
}
```

> Each crate defines its own `Error` + `type Result<T>`, with `#[from] crab_core::Error` enabling upward conversion.

**External Dependencies**: `unicode-width`, `strip-ansi-escapes`, `ulid`, `dunce`, `directories`

---

### 6.2 `crates/core/` -- Domain Model

**Responsibility**: Pure data structures + trait definitions with no I/O operations. Defines "what it is", not "how to do it".

**Directory Structure**

```
src/
├── lib.rs
├── message.rs        // Message, Role, ContentBlock, ToolUse, ToolResult
├── conversation.rs   // Conversation, Turn, context window abstraction
├── tool.rs           // trait Tool + ToolOutput + name constants (BASH/READ/WRITE/EDIT/GLOB/GREP)
├── model.rs          // ModelId, TokenUsage, CostTracker
├── hook.rs           // HookTrigger, HookAction enums
├── permission/       // PermissionMode, PermissionPolicy (12 files, see §4.1)
├── event.rs          // Event enum + SessionEvent + EventSink trait + EventStream
├── query.rs          // QuerySource enum
├── ide.rs            // IDE ambient context types
├── error.rs          // Error enum (thiserror)
└── result.rs         // Result<T> type alias
```

**Core Type Definitions**

```rust
// message.rs -- Message model (corresponds to CC src/types/message.ts)
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    Image {
        source: ImageSource,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String, // "base64"
    pub media_type: String,  // "image/png"
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }
}
```

```rust
// tool.rs -- Tool trait (corresponds to CC src/Tool.ts)
// Returns Pin<Box<dyn Future>> instead of native async fn because dyn Trait requires object safety
// (Arc<dyn Tool> requires the trait to be object-safe; RPITIT's impl Future does not satisfy this)
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::permission::PermissionMode;
use crab_core::Result;

/// Tool source classification -- determines the column in the permission matrix
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// Built-in tools (Bash/Read/Write/Edit/Glob/Grep etc.)
    BuiltIn,
    /// Tools provided by external MCP servers (untrusted source, Default/TrustProject require Prompt)
    McpExternal { server_name: String },
    /// Created by sub-Agent (AgentTool, TrustProject auto-approves)
    AgentSpawn,
}

pub trait Tool: Send + Sync {
    /// Unique tool identifier
    fn name(&self) -> &str;

    /// Tool description (used in system prompt)
    fn description(&self) -> &str;

    /// JSON Schema describing input parameters
    fn input_schema(&self) -> Value;

    /// Execute the tool and return the result
    /// Long-running tools should check for cancellation via ctx.cancellation_token
    fn execute(&self, input: Value, ctx: &ToolContext) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>>;

    /// Tool source (defaults to BuiltIn) -- affects the permission checking matrix
    fn source(&self) -> ToolSource {
        ToolSource::BuiltIn
    }

    /// Whether user confirmation is required (defaults to false)
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// Whether the tool is read-only (read-only tools can skip confirmation)
    fn is_read_only(&self) -> bool {
        false
    }

    /// Whether the tool can safely run concurrently given specific input.
    /// Defaults to `is_read_only()`. Override for input-dependent parallelism
    /// (e.g., Write to different files may be concurrent-safe).
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        self.is_read_only()
    }
}

// --- Tool implementation example ---
// impl Tool for BashTool {
//     fn name(&self) -> &str { "bash" }
//     fn description(&self) -> &str { "Execute a shell command" }
//     fn input_schema(&self) -> Value { /* ... */ }
//     fn execute(&self, input: Value, ctx: &ToolContext) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
//         Box::pin(async move {
//             let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
//             let output = crab_process::run(/* ... */).await?;
//             Ok(ToolOutput::success(output.stdout))
//         })
//     }
// }

/// Tool execution context
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub permission_mode: PermissionMode,
    pub session_id: String,
    /// Cancellation token -- long-running tools (e.g., Bash) should check periodically and exit early
    pub cancellation_token: CancellationToken,
    /// Permission policy (from merged configuration)
    pub permission_policy: crate::permission::PermissionPolicy,
}

/// Tool output content block -- supports text, image, and structured JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputContent {
    Text { text: String },
    Image { media_type: String, data: String },
    Json { value: Value },
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: Vec<ToolOutputContent>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolOutputContent::Text { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolOutputContent::Text { text: text.into() }],
            is_error: true,
        }
    }

    pub fn text(&self) -> String {
        self.content.iter()
            .filter_map(|c| match c { ToolOutputContent::Text { text } => Some(text.as_str()), _ => None })
            .collect::<Vec<_>>().join("")
    }
}
```

```rust
// model.rs -- Model and token tracking
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelId(pub String);

impl ModelId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Default)]
pub struct CostTracker {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cost_usd: f64,
}

impl CostTracker {
    pub fn record(&mut self, usage: &TokenUsage, cost: f64) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_cache_read_tokens += usage.cache_read_tokens;
        self.total_cache_creation_tokens += usage.cache_creation_tokens;
        self.total_cost_usd += cost;
    }
}
```

```rust
// event.rs -- Domain events (inter-crate decoupled communication)
use crate::model::TokenUsage;
use crate::tool::ToolOutput;
use serde_json::Value;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Event {
    // --- Message lifecycle ---
    TurnStart { turn_index: usize },
    MessageStart { id: String },
    ContentDelta { index: usize, delta: String },
    ThinkingDelta { index: usize, delta: String },
    ContentBlockStop { index: usize },
    MessageEnd { usage: TokenUsage },

    // --- Tool execution ---
    ToolUseStart { id: String, name: String, input: Value },
    ToolUseInput { id: String, input: Value },
    ToolOutputDelta { id: String, delta: String },
    ToolProgress { id: String, progress: crate::tool::ToolProgress },
    ToolResult { id: String, output: ToolOutput },

    // --- Permission interaction ---
    PermissionRequest { tool_name: String, input_summary: String, request_id: String },
    PermissionResponse { request_id: String, allowed: bool },

    // --- Context compaction ---
    CompactStart { strategy: String, before_tokens: u64 },
    CompactEnd { after_tokens: u64, removed_messages: usize },

    // --- Token warnings ---
    TokenWarning { usage_pct: f32, used: u64, limit: u64 },
    ContextUpgraded { from: String, to: String, old_window: u64, new_window: u64 },

    // --- Memory ---
    MemoryLoaded { count: usize },
    MemorySaved { filename: String },

    // --- Session history ---
    SessionSaved { session_id: String },
    SessionResumed { session_id: String, message_count: usize },

    // --- Sub-agent workers ---
    AgentWorkerStarted { worker_id: String, task_prompt: String },
    AgentWorkerCompleted { worker_id: String, result: Option<String>, success: bool, usage: TokenUsage },

    // --- Errors ---
    Error { message: String },
}
```

```rust
// permission.rs -- Permission model
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// All non-read-only tools require user confirmation.
    Default,
    /// Auto-approve file edits within the project; other mutations still prompt.
    AcceptEdits,
    /// Trust in-project file operations; out-of-project and dangerous still prompt.
    TrustProject,
    /// Auto-approve everything without prompting the user.
    DontAsk,
    /// Auto-approve everything (except `denied_tools`). Use with caution.
    Dangerously,
    /// Planning-only mode: the agent may read but not mutate.
    Plan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub allowed_tools: Vec<String>,
    /// denied_tools supports glob pattern matching (e.g., "mcp__*", "bash"),
    /// uses the globset crate for matching, supporting * / ? / [abc] syntax
    pub denied_tools: Vec<String>,
}
```

**External Dependencies**: `serde`, `serde_json`, `tokio-util` (sync), `futures` (note: `std::pin::Pin` / `std::future::Future` are from std, no extra dependencies)

**Feature Flags**: None (pure type definitions)

---

### 6.3 `crates/config/` -- Configuration System

**Responsibility**: Read/write and merge multi-layered configuration (corresponds to CC `src/services/remoteManagedSettings/` + `src/context/` config sections)

**Directory Structure**

```
src/
├── lib.rs
├── config.rs             // Config struct + Default
├── loader.rs             // resolve() pipeline (multi-source merge)
├── merge.rs              // toml::Value merging
├── runtime.rs            // env + CLI flag -> Value
├── validation.rs         // jsonschema thin wrapper
├── writer.rs             // toml_edit write-back
├── gitignore.rs          // config.local.toml auto-ignore
├── hooks.rs              // Hook config schema parser
├── migration.rs          // Schema versioned migration
├── permissions.rs        // StoredPermissionRule + PermissionRuleSet (disk persistence)
└── plugin_loader.rs      // Plugin config.json loader
```

**Configuration Layers (multi-source merge, low priority -> high priority)**

The `Config` struct covers: `api_provider`, `api_base_url`, `api_key_helper`, `model`, `small_model`, `permission_mode`, `system_prompt`, `mcp_servers`, `hooks`, `theme`, and more. Secrets do **not** live on `Config` — the resolved API key flows through an independent chain in `crab-auth` (see 6.4). The configuration sources (`ConfigLayer` enum) are merged at the `toml::Value` layer by `loader::resolve()` (defaults -> plugin -> user -> project -> local -> --config -> env -> CLI flags), with higher-priority sources overriding lower-priority ones.

**External Dependencies**: `crab-common`, `crab-core`, `serde`, `serde_json`, `toml`, `toml_edit`, `jsonschema`, `directories`

**Feature Flags**: None

---

### 6.4 `crates/auth/` -- Authentication

**Responsibility**: Unified management of all authentication methods (corresponds to CC `src/services/oauth/` + authentication-related code)

**Directory Structure**

```
src/
├── lib.rs
├── error.rs              // AuthError
├── oauth.rs              // OAuth2 PKCE flow + tokens.json store
├── keychain.rs           // System Keychain (macOS/Windows/Linux)
├── resolver.rs           // resolve_auth_key: env -> apiKeyHelper -> keychain -> tokens.json
├── bedrock_auth.rs       // AWS SigV4 signing (feature = "bedrock")
├── vertex_auth.rs        // GCP Vertex AI authentication
├── aws_iam.rs            // AWS IAM Roles + IRSA (pod-level)
├── gcp_identity.rs       // GCP Workload Identity Federation
└── credential_chain.rs   // Credential chain wrapper (delegates to resolver)
```

**Core Interface**

```rust
// lib.rs -- Unified authentication interface
pub enum AuthMethod {
    ApiKey(String),
    OAuth(OAuthToken),
    Bedrock(BedrockCredentials),
}

/// Authentication provider trait
/// Returns Pin<Box<dyn Future>> instead of native async fn because dyn Trait requires object safety
/// (Box<dyn AuthProvider> requires the trait to be object-safe; RPITIT's impl Future does not satisfy this)
/// Implementations use tokio::sync::RwLock internally to protect the token cache;
/// get_auth() takes a read lock on the hot path, refresh() takes a write lock to refresh
pub trait AuthProvider: Send + Sync {
    /// Get the currently valid authentication info (read lock, typically <1us)
    fn get_auth(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<AuthMethod>> + Send + '_>>;
    /// Refresh authentication (e.g., OAuth token expired) -- may trigger network requests
    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>>;
}

// resolver.rs -- secrets do not live on Config; the chain is orthogonal to file-layer config.
pub fn resolve_auth_key(cfg: &crab_config::Config) -> Option<String> {
    // 1. ANTHROPIC_AUTH_TOKEN env
    // 2. provider-specific API-key env (ANTHROPIC_API_KEY / OPENAI_API_KEY / DEEPSEEK_API_KEY)
    // 3. apiKeyHelper script (path declared in config; stdout consumed as the key)
    // 4. system keychain (crab-auth::keychain)
    // 5. ~/.crab/auth/tokens.json (OAuth access token)
    // ...
}

// keychain.rs -- Uses the auth crate's local AuthError, not crab_core::Error
// (the common layer does not include Auth variants; Auth errors are defined in crates/auth/src/error.rs)
use crate::error::AuthError;

pub fn get(service: &str, key: &str) -> Result<String, AuthError> {
    let entry = keyring::Entry::new(service, key)
        .map_err(|e| AuthError::Auth { message: format!("keychain init failed: {e}") })?;
    entry.get_password().map_err(|e| AuthError::Auth {
        message: format!("keychain read failed: {e}"),
    })
}

pub fn set(service: &str, key: &str, value: &str) -> Result<(), AuthError> {
    let entry = keyring::Entry::new(service, key)
        .map_err(|e| AuthError::Auth { message: format!("keychain init failed: {e}") })?;
    entry.set_password(value).map_err(|e| AuthError::Auth {
        message: format!("keychain write failed: {e}"),
    })
}
```

**External Dependencies**: `crab-common`, `crab-core`, `crab-config`, `keyring`, `oauth2`, `reqwest`

**Feature Flags**

```toml
[features]
default = []
bedrock = []
vertex  = []
```

---

### 6.5 `crates/api/` -- LLM API Client

**Responsibility**: Encapsulate all LLM API communication with two independent clients implementing the two major API standards (corresponds to CC `src/services/api/`)

**Core Design**: No unified trait abstraction is used -- the Anthropic Messages API and OpenAI Chat Completions API
differ too much (message format, streaming event granularity, tool call protocol). Forcing unification would create a
"lowest common denominator" trap, losing provider-specific capabilities (Anthropic's prompt cache / extended thinking,
OpenAI's logprobs / structured output).

Uses **two fully independent clients + enum dispatch**:
- `anthropic/` -- Complete Anthropic Messages API client with its own types, SSE parsing, authentication
- `openai/` -- Complete OpenAI Chat Completions client, covering all compatible endpoints (Ollama/DeepSeek/vLLM/Gemini etc.)
- `LlmBackend` enum -- Determined at compile time, zero dynamic dispatch, exhaustive match ensures nothing is missed

The agent/session layer interacts through the `LlmBackend` enum. The internal unified `MessageRequest` / `StreamEvent`
are Crab Code's own data model, not an API abstraction. Each client independently handles format conversion internally.

**Directory Structure**

```
src/
├── lib.rs                // LlmBackend enum + create_backend()
├── types.rs              // Internal unified request/response/event types (Crab Code's own format)
├── anthropic/            // Fully independent Anthropic Messages API client
│   ├── mod.rs
│   ├── client.rs         // HTTP + SSE + retry
│   ├── types.rs          // Anthropic API native request/response types
│   ├── convert.rs        // Anthropic types <-> internal types
│   └── files.rs          // Anthropic Files API
├── openai/               // Fully independent OpenAI Chat Completions client
│   ├── mod.rs
│   ├── client.rs         // HTTP + SSE + retry
│   ├── types.rs          // OpenAI API native request/response types
│   └── convert.rs        // OpenAI types <-> internal types
├── bedrock.rs            // AWS Bedrock adapter (feature = "bedrock", wraps anthropic client)
├── vertex.rs             // Google Vertex adapter (feature = "vertex", wraps anthropic client)
├── rate_limit.rs         // Shared rate limiting, exponential backoff
├── cache.rs              // Prompt cache management (Anthropic path only)
├── error.rs
├── streaming.rs          // Streaming tool call parsing (partial tool argument streaming)
├── fallback.rs           // Multi-model fallback chain (primary fails -> backup model)
├── capabilities.rs       // Model capability negotiation and discovery
├── context_optimizer.rs  // Context window optimization + smart truncation strategy
├── retry_strategy.rs     // Enhanced retry strategy (backoff + jitter)
├── error_classifier.rs   // Error classification (retryable/non-retryable/rate-limited)
├── token_estimation.rs   // Approximate token count estimation
├── ttft_tracker.rs       // Time-to-first-token latency tracking
├── fast_mode.rs          // Fast mode switching
└── usage_tracker.rs      // Usage aggregation (per-session/model)
```

**Core Interface**

```rust
// types.rs -- Crab Code internal unified types (not an API abstraction, but its own data model)
use crab_core::message::Message;
use crab_core::model::{ModelId, TokenUsage};

/// Internal message request -- each client converts it to its own API format internally
#[derive(Debug, Clone)]
pub struct MessageRequest<'a> {
    pub model: ModelId,
    pub messages: std::borrow::Cow<'a, [Message]>,
    pub system: Option<String>,
    pub max_tokens: u32,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
}

/// Internal unified stream event -- each client maps its own SSE format to this enum
#[derive(Debug, Clone)]
pub enum StreamEvent {
    MessageStart { id: String },
    ContentBlockStart { index: usize, content_type: String },
    ContentDelta { index: usize, delta: String },
    ContentBlockStop { index: usize },
    MessageDelta { usage: TokenUsage },
    MessageStop,
    Error { message: String },
}
```

```rust
// lib.rs -- Enum dispatch (no dyn trait, determined at compile time, zero dynamic dispatch overhead)
use futures::stream::{self, Stream, StreamExt};
use either::Either;

/// LLM backend enum -- provider count is limited (2 standards + 2 cloud variants), enum is sufficient
/// Third-party provider extension would go through a WASM plugin system (not built yet)
pub enum LlmBackend {
    Anthropic(anthropic::AnthropicClient),
    OpenAi(openai::OpenAiClient),
    // Bedrock and Vertex are essentially different entry points for the Anthropic API, wrapping AnthropicClient
    #[cfg(feature = "bedrock")]
    Bedrock(anthropic::AnthropicClient),  // Different auth + base_url
    #[cfg(feature = "vertex")]
    Vertex(anthropic::AnthropicClient),   // Different auth + base_url
}

impl LlmBackend {
    /// Stream a message
    pub fn stream_message<'a>(
        &'a self,
        req: types::MessageRequest<'a>,
    ) -> impl Stream<Item = crab_core::Result<types::StreamEvent>> + Send + 'a {
        match self {
            Self::Anthropic(c) => Either::Left(c.stream(req)),
            Self::OpenAi(c) => Either::Right(c.stream(req)),
            // Bedrock/Vertex use the Anthropic path
        }
    }

    /// Non-streaming send (used for lightweight tasks like compaction)
    pub async fn send_message(
        &self,
        req: types::MessageRequest<'_>,
    ) -> crab_core::Result<(crab_core::message::Message, crab_core::model::TokenUsage)> {
        match self {
            Self::Anthropic(c) => c.send(req).await,
            Self::OpenAi(c) => c.send(req).await,
        }
    }

    /// Provider name
    pub fn name(&self) -> &str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::OpenAi(_) => "openai",
        }
    }
}

/// Construct backend from configuration
pub fn create_backend(settings: &crab_config::Config) -> LlmBackend {
    match settings.api_provider.as_deref() {
        Some("openai" | "deepseek") => {
            let base_url = settings.api_base_url.as_deref()
                .unwrap_or("https://api.openai.com/v1");
            let api_key = crab_auth::resolve_auth_key(settings);
            LlmBackend::OpenAi(openai::OpenAiClient::new(base_url, api_key))
        }
        _ => {
            let base_url = settings.api_base_url.as_deref()
                .unwrap_or("https://api.anthropic.com");
            let auth = crab_auth::create_auth_provider(settings);
            LlmBackend::Anthropic(anthropic::AnthropicClient::new(base_url, auth))
        }
    }
}
```

```rust
// anthropic/client.rs -- Anthropic Messages API (fully independent implementation)
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    auth: Box<dyn crab_auth::AuthProvider>,
}

impl AnthropicClient {
    pub fn new(base_url: &str, auth: Box<dyn crab_auth::AuthProvider>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self { http, base_url: base_url.to_string(), auth }
    }

    /// Streaming call -- POST /v1/messages, stream: true
    pub fn stream<'a>(
        &'a self,
        req: crate::types::MessageRequest<'a>,
    ) -> impl Stream<Item = crab_core::Result<crate::types::StreamEvent>> + Send + 'a {
        // 1. MessageRequest -> Anthropic native request (self::types::AnthropicRequest)
        // 2. POST /v1/messages, set stream: true
        // 3. Parse Anthropic SSE: message_start / content_block_delta / message_stop
        // 4. self::convert::to_stream_event() maps to internal StreamEvent
        // ...
    }

    /// Non-streaming call
    pub async fn send(
        &self,
        req: crate::types::MessageRequest<'_>,
    ) -> crab_core::Result<(crab_core::message::Message, crab_core::model::TokenUsage)> {
        // ...
    }
}
```

```rust
// openai/client.rs -- OpenAI Chat Completions API (fully independent implementation)
//
// Covers all backends compatible with /v1/chat/completions:
// OpenAI, Ollama, DeepSeek, vLLM, TGI, LiteLLM, Azure OpenAI, Google Gemini (OpenAI-compatible endpoint)
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self { http, base_url: base_url.to_string(), api_key }
    }

    /// Streaming call -- POST /v1/chat/completions, stream: true
    pub fn stream<'a>(
        &'a self,
        req: crate::types::MessageRequest<'a>,
    ) -> impl Stream<Item = crab_core::Result<crate::types::StreamEvent>> + Send + 'a {
        // 1. MessageRequest -> OpenAI native request (self::types::ChatCompletionRequest)
        //    - system prompt -> messages[0].role="system"
        //    - ContentBlock::ToolUse -> tool_calls array
        //    - ContentBlock::ToolResult -> role="tool" message
        // 2. POST /v1/chat/completions, stream: true
        // 3. Parse OpenAI SSE: data: {"choices":[{"delta":...}]}
        // 4. self::convert::to_stream_event() maps to internal StreamEvent
        // ...
    }

    /// Non-streaming call
    pub async fn send(
        &self,
        req: crate::types::MessageRequest<'_>,
    ) -> crab_core::Result<(crab_core::message::Message, crab_core::model::TokenUsage)> {
        // ...
    }
}
```

**Key Differences Between the Two API Standards** (handled by each client's `convert.rs` internally, not exposed to upper layers)

| Dimension | Anthropic Messages API | OpenAI Chat Completions API |
|-----------|----------------------|---------------------------|
| system prompt | Separate `system` field | `messages[0].role="system"` |
| Message content | `content: Vec<ContentBlock>` | `content: string` |
| Tool calls | `ContentBlock::ToolUse` | `tool_calls` array |
| Tool results | `ContentBlock::ToolResult` | `role="tool"` message |
| Streaming format | `content_block_delta` events | `choices[].delta` |
| Token stats | `input_tokens` / `output_tokens` | `prompt_tokens` / `completion_tokens` |
| Provider-specific | prompt cache, extended thinking | logprobs, structured output |

```rust
// rate_limit.rs -- Shared rate limiting and backoff
use std::time::Duration;

pub struct RateLimiter {
    pub remaining_requests: u32,
    pub remaining_tokens: u32,
    pub reset_at: std::time::Instant,
}

/// Exponential backoff strategy
pub fn backoff_delay(attempt: u32) -> Duration {
    let base = Duration::from_millis(500);
    let max = Duration::from_secs(30);
    let delay = base * 2u32.pow(attempt.min(6));
    delay.min(max)
}
```

**External Dependencies**: `crab-core`, `crab-config`, `crab-auth`, `reqwest`, `tokio`, `serde`, `eventsource-stream`, `futures`, `either`

**Feature Flags**

```toml
[features]
default = []
bedrock = ["crab-auth/bedrock"]
vertex  = ["crab-auth/vertex"]
proxy   = ["reqwest/socks"]
```

---

### 6.6 `crates/mcp/` -- MCP Facade

**Responsibility**: Crab's own MCP facade and protocol adaptation layer (corresponds to CC `src/services/mcp/`)

MCP is an open protocol that lets LLMs connect to external tools/resources, based on JSON-RPC 2.0.
`crab-mcp` does not directly expose the underlying SDK to `cli` / `tools` / `session`; instead, it absorbs the official SDK internally and exposes a stable Crab-side interface: `McpClient`, `McpManager`, `ToolRegistryHandler`, `mcp__<server>__<tool>` naming, and config discovery logic all live in this layer.

**Directory Structure**

```
src/
├── lib.rs
├── protocol.rs             // Crab's own MCP facade types
├── client.rs               // MCP client facade (internally may delegate to rmcp)
├── server/                 // MCP server (module directory)
│   ├── mod.rs
│   ├── prompts.rs          // Prompt serving
│   ├── resources.rs        // Resource serving
│   └── tools.rs            // Tool serving
├── manager.rs              // Lifecycle management, multi-server coordination
├── transport/
│   ├── mod.rs              // Compatible Transport trait / local transport abstraction
│   ├── stdio.rs            // stdin/stdout transport
│   └── ws.rs               // WebSocket transport (feature = "ws")
├── auth/                   // MCP OAuth2 / API key authentication (12 files)
│   ├── mod.rs
│   ├── api_key.rs          // API key auth
│   ├── callback.rs         // OAuth callback server
│   ├── discovery.rs        // Auth endpoint discovery
│   ├── exchange.rs         // Token exchange
│   ├── flow.rs             // OAuth flow orchestration
│   ├── manager.rs          // Auth lifecycle manager
│   ├── pkce.rs             // PKCE challenge/verifier
│   ├── quirks.rs           // Provider-specific quirks
│   ├── refresh.rs          // Token refresh
│   ├── store.rs            // Token persistence
│   └── types.rs            // Auth types
├── resource.rs             // Resource caching, templates
├── discovery.rs            // Server auto-discovery
├── sse_server.rs           // SSE server transport (crab as MCP server)
├── sampling.rs             // MCP sampling (server requests LLM inference)
├── roots.rs                // MCP roots (workspace root directory declaration)
├── logging.rs              // MCP logging protocol (structured log messages)
├── handshake.rs            // Initialization handshake flow (initialize/initialized)
├── negotiation.rs          // Capability negotiation (client/server capability sets)
├── capability.rs           // Capability declaration types (resources/tools/prompts/sampling)
├── notification.rs         // Server notification push (tool changes/resource updates)
├── progress.rs             // Progress reporting (long-running tool execution)
├── cancellation.rs         // Request cancellation mechanism ($/cancelRequest)
├── health.rs               // Health check + heartbeat (auto-reconnect)
├── server_acl.rs           // Server access control list
├── elicitation.rs          // User input request handling
├── env_expansion.rs        // ${VAR} environment variable expansion in config
├── official_registry.rs    // Official MCP server registry
└── normalization.rs        // Tool/resource name normalization
```

**Boundary Principles**

- `crab-mcp` exposes Crab's own facade types, not raw `rmcp` types
- Client-side stdio / HTTP connections preferably reuse the official `rmcp`
- The config layer only retains `stdio` / `http` / `ws` as transport options
- Upper-layer crates only depend on `crab_mcp::*`; they never directly depend on the underlying MCP SDK
- `protocol.rs` continues to carry Crab-side stable data structures, preventing SDK type leakage

**Core Types**

```rust
// protocol.rs -- JSON-RPC 2.0 messages
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String, // "2.0"
    pub id: u64,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

/// MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// MCP resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}
```

```rust
// transport/mod.rs -- Transport abstraction
// Returns Pin<Box<dyn Future>> instead of native async fn because dyn Trait requires object safety
// (Box<dyn Transport> requires the trait to be object-safe; RPITIT's impl Future does not satisfy this)
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use std::future::Future;
use std::pin::Pin;

pub trait Transport: Send + Sync {
    /// Send a request and wait for a response
    fn send(&self, req: JsonRpcRequest) -> Pin<Box<dyn Future<Output = crab_core::Result<JsonRpcResponse>> + Send + '_>>;
    /// Send a notification (no response expected)
    fn notify(&self, method: &str, params: serde_json::Value) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>>;
    /// Close the transport
    fn close(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>>;
}

// --- Transport implementation example ---
// impl Transport for StdioTransport {
//     fn send(&self, req: JsonRpcRequest) -> Pin<Box<dyn Future<Output = crab_core::Result<JsonRpcResponse>> + Send + '_>> {
//         Box::pin(async move {
//             self.write_message(&req).await?;
//             self.read_response().await
//         })
//     }
//     // ... notify, close similarly
// }
```

```rust
// client.rs -- MCP client facade
use crate::protocol::{McpToolDef, ServerCapabilities, ServerInfo};

pub struct McpClient {
    server_name: String,
    server_info: ServerInfo,
    capabilities: ServerCapabilities,
    tools: Vec<McpToolDef>,
}

impl McpClient {
    /// Connect to a stdio MCP server via the official SDK
    pub async fn connect_stdio(...) -> crab_core::Result<Self> { /* ... */ }

    /// Connect to an HTTP MCP endpoint via the official SDK
    pub async fn connect_streamable_http(...) -> crab_core::Result<Self> { /* ... */ }

    /// Call an MCP tool
    pub async fn call_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> crab_core::Result<serde_json::Value> {
        // ...
    }

    /// Read an MCP resource
    pub async fn read_resource(&self, uri: &str) -> crab_core::Result<String> {
        // ...
    }

    pub fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }
}
```

**External Dependencies**: `crab-core`, `tokio`, `serde`, `serde_json`, `rmcp`

**Feature Flags**

```toml
[features]
default = []
ws = ["tokio-tungstenite"]
```

---

### 6.7 `crates/fs/` -- File System Operations

**Responsibility**: Encapsulate all file system related operations (corresponds to the underlying logic of GlobTool/GrepTool/FileReadTool in CC)

**Directory Structure**

```
src/
├── lib.rs
├── glob.rs               // globset wrapper
├── grep.rs               // ripgrep core integration
├── gitignore.rs          // .gitignore rule parsing and filtering
├── watch.rs              // notify file watching (with debouncing + batch aggregation)
├── lock.rs               // File locking (fd-lock)
├── diff.rs               // similar wrapper, edit/patch generation
├── symlink.rs            // Symbolic link handling + secure path resolution (escape prevention)
└── file_cache.rs         // File content cache
```

**Core Interface**

```rust
// glob.rs -- File pattern matching
use std::path::{Path, PathBuf};

pub struct GlobResult {
    pub matches: Vec<PathBuf>,
    pub truncated: bool,
}

/// Search files in a directory by glob pattern
pub fn find_files(
    root: &Path,
    pattern: &str,
    limit: usize,
) -> crab_core::Result<GlobResult> {
    // Uses ignore crate (automatically respects .gitignore)
    // Sorted by modification time
    // ...
}

// grep.rs -- Content search
pub struct GrepMatch {
    pub path: PathBuf,
    pub line_number: usize,
    pub line_content: String,
}

pub struct GrepOptions {
    pub pattern: String,
    pub path: PathBuf,
    pub case_insensitive: bool,
    pub file_glob: Option<String>,
    pub max_results: usize,
    pub context_lines: usize,
}

/// Search content in a directory by regex
pub fn search(opts: &GrepOptions) -> crab_core::Result<Vec<GrepMatch>> {
    // Uses grep-regex + grep-searcher
    // Automatically respects .gitignore
    // ...
}

// diff.rs -- Diff generation
pub struct EditResult {
    pub old_content: String,
    pub new_content: String,
    pub unified_diff: String,
}

/// Exact replacement based on old_string -> new_string
pub fn apply_edit(
    file_content: &str,
    old_string: &str,
    new_string: &str,
) -> crab_core::Result<EditResult> {
    // Uses similar to generate unified diff
    // ...
}
```

**External Dependencies**: `crab-common`, `crab-core`, `globset`, `grep-matcher`, `grep-regex`, `grep-searcher`, `ignore`, `notify`, `similar`, `fd-lock`

**Feature Flags**: None

---

### 6.8 `crates/process/` -- Subprocess Management

**Responsibility**: Subprocess lifecycle management (corresponds to the underlying execution logic of CC's BashTool)

**Directory Structure**

```
src/
├── lib.rs
├── spawn.rs              // Subprocess launching, environment inheritance
├── pty.rs                // Pseudo-terminal allocation (feature = "pty")
├── tree.rs               // Process tree kill (sysinfo)
└── signal.rs             // Signal handling, graceful shutdown
```

**Core Interface**

```rust
// spawn.rs -- Subprocess execution
use std::path::Path;
use std::time::Duration;

pub struct SpawnOptions {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<std::path::PathBuf>,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    pub stdin_data: Option<String>,
}

pub struct SpawnOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

/// Execute a command and wait for the result
pub async fn run(opts: SpawnOptions) -> crab_core::Result<SpawnOutput> {
    use tokio::process::Command;
    // 1. Build Command
    // 2. Set working_dir, env
    // 3. Wrap with tokio::time::timeout if timeout is set
    // 4. Collect stdout/stderr
    // ...
}

/// Execute a command and stream output
pub async fn run_streaming(
    opts: SpawnOptions,
    on_stdout: impl Fn(&str) + Send,
    on_stderr: impl Fn(&str) + Send,
) -> crab_core::Result<i32> {
    // ...
}

// tree.rs -- Process tree management
/// Kill a process and all its child processes
pub fn kill_tree(pid: u32) -> crab_core::Result<()> {
    use sysinfo::{Pid, System};
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    // Recursively find child processes and kill
    // ...
}

// signal.rs -- Signal handling
/// Register Ctrl+C / SIGTERM handler
pub fn register_shutdown_handler(
    on_shutdown: impl Fn() + Send + 'static,
) {
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        on_shutdown();
    });
}
```

**External Dependencies**: `crab-common`, `crab-core`, `tokio` (process, signal), `sysinfo`

**Feature Flags**

```toml
[features]
default = []
pty = ["portable-pty"]
```

---

### 6.9 `crates/tools/` -- Tool System

**Responsibility**: Tool registration, lookup, execution, including all built-in tools (corresponds to CC `src/tools/`)

**Directory Structure**

```
src/
├── lib.rs
├── registry.rs       // ToolRegistry: registration, lookup, schema generation
├── executor.rs       // Unified executor with permission checking
├── permission.rs     // Tool permission checking logic
│
├── builtin/              // Built-in tools (~45 files + computer_use/ subdir)
│   ├── mod.rs            // register_all_builtins()
│   ├── registry.rs       // Builtin tool registry helpers
│   │
│   │  // ── Core file tools ──
│   ├── bash.rs           // BashTool -- shell command execution
│   ├── bash_classifier.rs // Bash command safety classification
│   ├── bash_security.rs  // Bash security checks
│   ├── powershell.rs     // PowerShellTool -- Windows PowerShell execution
│   ├── read.rs           // ReadTool -- file reading
│   ├── read_enhanced.rs  // Enhanced file reading (PDF, images, notebooks)
│   ├── edit.rs           // EditTool -- diff-based file editing
│   ├── write.rs          // WriteTool -- file creation/overwrite
│   ├── glob.rs           // GlobTool -- file pattern matching
│   ├── grep.rs           // GrepTool -- content search
│   ├── image_read.rs     // Image reading tool
│   ├── notebook.rs       // NotebookTool -- Jupyter support
│   ├── snip.rs           // Snip tool -- content truncation
│   │
│   │  // ── Web tools ──
│   ├── web_search.rs     // WebSearchTool -- web search
│   ├── web_fetch.rs      // WebFetchTool -- web page fetching
│   ├── web_cache.rs      // Web page cache
│   ├── web_formatter.rs  // Web content formatter
│   ├── web_browser.rs    // Browser automation tool
│   │
│   │  // ── Agent / task tools ──
│   ├── agent.rs          // AgentTool -- sub-Agent launching
│   ├── task.rs           // TaskCreate/Get/List/Update/Stop/Output
│   ├── todo_write.rs     // TodoWrite tool
│   ├── team.rs           // TeamCreate/Delete tool
│   ├── send_message.rs   // SendMessage tool (inter-agent messaging)
│   ├── send_user_file.rs // Send file to user
│   ├── monitor.rs        // Monitor tool (background process watching)
│   ├── sleep.rs          // Sleep tool
│   ├── cron.rs           // CronCreate/Delete/List tools
│   ├── workflow.rs       // Workflow execution tool
│   │
│   │  // ── Plan mode tools ──
│   ├── plan_mode.rs      // EnterPlanMode tool
│   ├── plan_file.rs      // Plan file operations
│   ├── plan_approval.rs  // ExitPlanMode / plan approval tool
│   ├── verify_plan.rs    // Plan verification tool
│   │
│   │  // ── Integration tools ──
│   ├── mcp_tool.rs       // MCP tool Tool trait adapter
│   ├── mcp_auth.rs       // MCP authentication tool
│   ├── mcp_resource.rs   // MCP resource access tool
│   ├── lsp.rs            // LSP integration tool
│   ├── worktree.rs       // Git Worktree tool
│   ├── ask_user.rs       // User interaction / question tool
│   ├── skill.rs          // Skill invocation tool
│   ├── config_tool.rs    // Configuration tool
│   ├── remote_trigger.rs // Remote trigger tool
│   ├── brief.rs          // Brief / notification tool
│   ├── structured_output.rs // Structured output tool
│   ├── tool_search.rs    // Tool search / discovery
│   │
│   │  // ── Computer use ──
│   └── computer_use/     // Computer use tools (9 files)
│       ├── mod.rs
│       ├── tool.rs       // ComputerUseTool
│       ├── input.rs      // Input simulation
│       ├── screenshot.rs // Screenshot capture
│       ├── window.rs     // Window management
│       └── platform/     // Platform-specific backends
│           ├── mod.rs
│           ├── linux.rs
│           ├── macos.rs
│           └── windows.rs
│
├── sandbox.rs        // Tool sandbox policy
├── schema.rs         // Tool schema -> API tools parameter conversion
├── str_utils.rs      // String utility helpers
└── tool_use_summary.rs // Tool result summary generation
```

**Core Types**

```rust
// registry.rs -- Tool registry
use crab_core::tool::Tool;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Find by name
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Get JSON Schema for all tools (for API requests)
    pub fn tool_schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }

    /// List all tool names
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}
```

```rust
// executor.rs -- Unified executor
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use crate::registry::ToolRegistry;
use std::sync::Arc;

pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
}

impl ToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a tool (with permission checking)
    ///
    /// **Permission decision matrix** (mode x tool_type x path_scope):
    ///
    /// | PermissionMode | read_only | write(in project) | write(outside project) | dangerous | mcp_external | agent_spawn | denied_list |
    /// |----------------|-----------|-------------------|----------------------|-----------|-------------|-------------|-------------|
    /// | Default        | Allow     | **Prompt**        | **Prompt**           | **Prompt**| **Prompt**  | **Prompt**  | **Deny**    |
    /// | TrustProject   | Allow     | Allow             | **Prompt**           | **Prompt**| **Prompt**  | Allow       | **Deny**    |
    /// | Dangerously    | Allow     | Allow             | Allow                | Allow     | Allow       | Allow       | **Deny**    |
    ///
    /// - denied_list is denied in all modes (from settings.json `deniedTools`)
    /// - allowed_list match skips normal Prompt (but does not exempt dangerous detection)
    /// - dangerous = BashTool contains `rm -rf`/`sudo`/`curl|sh`/`chmod`/`eval` and other high-risk patterns
    /// - mcp_external: tools provided by external MCP servers; Default/TrustProject both require Prompt (untrusted source)
    /// - agent_spawn: sub-Agent creation; TrustProject auto-approves; sub-Agents inherit parent Agent's permission_mode
    pub async fn execute(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> crab_core::Result<ToolOutput> {
        let tool = self
            .registry
            .get(tool_name)
            .ok_or_else(|| crab_core::Error::Other(
                format!("tool not found: {tool_name}"),
            ))?;

        // 1. Check denied list -- denied in all modes
        //    denied_tools supports glob matching (e.g., "mcp__*", "bash")
        //    Uses globset for pattern matching, supporting * / ? / [abc] glob syntax
        if ctx.permission_policy.denied_tools.iter().any(|pattern| {
            globset::Glob::new(pattern)
                .ok()
                .and_then(|g| g.compile_matcher().is_match(tool_name).then_some(()))
                .is_some()
        }) {
            return Ok(ToolOutput::error(format!("tool '{tool_name}' is denied by policy")));
        }

        // 2. Dangerously mode short-circuit -- skip all permission checks (including allowed_tools and dangerous detection)
        //    Placed after denied_tools: even in Dangerously mode, denied_tools still applies
        if ctx.permission_mode == PermissionMode::Dangerously {
            return tool.execute(input, ctx).await;
        }

        // 3. Check allowed list -- explicitly allowed skips prompt
        let explicitly_allowed = ctx.permission_policy.allowed_tools.contains(&tool_name.to_string());

        // 4. Decide by matrix (combining tool.source() + mode + path_scope)
        // allowed_tools only exempts normal Prompt, not dangerous detection
        let needs_prompt = if explicitly_allowed {
            self.is_dangerous_command(&input) // allowed_tools only exempts normal Prompt, not dangerous detection
        } else {
            match tool.source() {
                // MCP external tools: Default/TrustProject both require Prompt (untrusted source)
                ToolSource::McpExternal { .. } => true,
                // Sub-Agent creation: TrustProject auto-approves, Default requires Prompt
                ToolSource::AgentSpawn => {
                    ctx.permission_mode == PermissionMode::Default
                }
                // Built-in tools: follow the original matrix
                ToolSource::BuiltIn => {
                    match ctx.permission_mode {
                        PermissionMode::Dangerously => unreachable!(), // Already short-circuited above
                        PermissionMode::TrustProject => {
                            if tool.is_read_only() {
                                false
                            } else {
                                let in_project = self.is_path_in_project(tool_name, &input, &ctx.working_dir);
                                !in_project || self.is_dangerous_command(&input)
                            }
                        }
                        PermissionMode::Default => {
                            !tool.is_read_only()
                        }
                    }
                }
            }
        };

        if needs_prompt {
            // Request user confirmation via event channel
            let approved = self.request_permission(tool_name, &input, ctx).await?;
            if !approved {
                return Ok(ToolOutput::error("user denied permission"));
            }
        }

        tool.execute(input, ctx).await
    }

    /// Check whether the tool operation path is within the project directory
    ///
    /// **TOCTOU + symlink protection**:
    /// - Uses `std::fs::canonicalize()` to resolve symbolic links before comparison, preventing symlink bypass
    /// - File operations should use `O_NOFOLLOW` (or Rust equivalent) to prevent TOCTOU race conditions
    /// - Note: canonicalize only works on existing paths; non-existent paths need parent directory canonicalization
    fn is_path_in_project(&self, tool_name: &str, input: &serde_json::Value, project_dir: &std::path::Path) -> bool {
        // BashTool special handling: input contains "command" not "file_path"
        // Need to parse possible path references from the command string
        if tool_name == "bash" {
            return self.bash_paths_in_project(input, project_dir);
        }

        // Other tools: extract file_path/path field from input
        input.get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .map(|p| {
                let raw = std::path::Path::new(p);
                // Canonicalize first to resolve symlinks, preventing symlink bypass of project boundary
                // Fallback for non-existent paths: canonicalize nearest existing ancestor directory + remaining relative segments
                let resolved = std::fs::canonicalize(raw).unwrap_or_else(|_| {
                    // Path does not exist yet (e.g., a new file about to be created), walk up to find existing ancestor
                    let mut ancestor = raw.to_path_buf();
                    let mut suffix = std::path::PathBuf::new();
                    while !ancestor.exists() {
                        if let Some(file_name) = ancestor.file_name() {
                            suffix = std::path::Path::new(file_name).join(&suffix);
                        }
                        if !ancestor.pop() { break; }
                    }
                    std::fs::canonicalize(&ancestor)
                        .unwrap_or(ancestor)
                        .join(suffix)
                });
                resolved.starts_with(project_dir)
            })
            .unwrap_or(true) // Tools without path parameters default to being considered in-project
    }

    /// BashTool path detection: extract absolute paths from the command string and check them
    /// In TrustProject mode, commands referencing absolute paths outside the project require Prompt
    ///
    /// **Important: This is best-effort heuristic detection**
    /// Path extraction from shell commands cannot be 100% accurate (variable expansion, subshells, nested quotes, etc.).
    /// Conservative strategy: when path analysis is uncertain, return Uncertain -> maps to Prompt.
    /// Specific scenarios:
    /// - Cannot extract any path tokens -> Uncertain (variables/subshells may reference paths)
    /// - Contains shell metacharacters ($, `, $(...)) -> Uncertain (paths may be dynamically constructed)
    ///
    /// **Core principle: when reliable parsing is impossible, default to requiring Prompt -- better to ask too much than miss.**
    fn bash_paths_in_project(&self, input: &serde_json::Value, project_dir: &std::path::Path) -> bool {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // Conservative strategy: cannot reliably extract paths with shell metacharacters, return false (require Prompt)
        let shell_metacharacters = ['$', '`'];
        if cmd.chars().any(|c| shell_metacharacters.contains(&c)) || cmd.contains("$(") {
            return false; // Uncertain -> maps to Prompt
        }

        // cd to absolute path changes the working directory for subsequent commands, treated as path reference
        // e.g., `cd /etc && cat passwd` actually operates on files outside the project
        if cmd.starts_with("cd ") || cmd.contains("&& cd ") || cmd.contains("; cd ") || cmd.contains("|| cd ") {
            // Extract cd target path and check if it's within the project
            for segment in cmd.split("&&").chain(cmd.split(";")).chain(cmd.split("||")) {
                let trimmed = segment.trim();
                if trimmed.starts_with("cd ") {
                    let target = trimmed.strip_prefix("cd ").unwrap().trim();
                    if target.starts_with('/') || target.starts_with("~/") {
                        let expanded = if target.starts_with("~/") {
                            crab_common::path::home_dir().join(&target[2..])
                        } else {
                            std::path::PathBuf::from(target)
                        };
                        if !expanded.starts_with(project_dir) {
                            return false; // cd to outside project -> Prompt
                        }
                    }
                }
            }
        }

        // Extract all absolute path tokens from the command
        let abs_paths: Vec<&str> = cmd.split_whitespace()
            .filter(|token| token.starts_with('/') || token.starts_with("~/"))
            .collect();

        // Conservative strategy: return false when no paths can be extracted (Uncertain -> Prompt)
        // Note: pure relative path commands (e.g., `cargo build`) don't have / prefix, will reach here
        // But these commands are usually safe in-project operations, so still return true
        if abs_paths.is_empty() {
            return true;
        }

        // Any absolute path outside the project -> return false (require Prompt)
        abs_paths.iter().all(|p| {
            let expanded = if p.starts_with("~/") {
                crab_common::path::home_dir().join(&p[2..])
            } else {
                std::path::PathBuf::from(p)
            };
            expanded.starts_with(project_dir)
        })
    }

    /// Detect dangerous command patterns
    /// Covers: destructive operations, privilege escalation, remote code execution, file overwrite, chained dangerous commands
    ///
    /// **Important: all pattern matching must use a shell tokenizer to exclude quoted content**
    /// All `cmd.contains(pattern)` below should be replaced with tokenize-then-match in actual implementation:
    /// 1. Use `shell-words` crate (or equivalent tokenizer) to split cmd into tokens
    /// 2. Only match dangerous patterns in non-quoted tokens
    /// 3. Example: `echo "rm -rf /" > log.txt` should NOT trigger `rm -rf` detection (inside quotes)
    ///    but `> log.txt` redirect is outside quotes and should be detected normally
    /// 4. When tokenizer fails (e.g., unclosed quotes), handle conservatively -> treat as dangerous
    fn is_dangerous_command(&self, input: &serde_json::Value) -> bool {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // 1. Direct dangerous patterns
        // Two-tier strategy: Level 1 exact match (list below) + Level 2 heuristic detection (interpreter + -c/-e combos)
        let dangerous_patterns = [
            // -- Destructive file operations --
            "rm -rf", "rm -fr",
            // -- Privilege escalation --
            "sudo ",
            // -- Disk/device operations --
            "mkfs", "dd if=", "> /dev/",
            // -- Remote code execution (pipe to shell) --
            "curl|sh", "curl|bash", "wget|sh", "wget|bash",
            "curl | sh", "curl | bash", "wget | sh", "wget | bash",
            // -- Permission modification --
            "chmod ", "chown ",
            // -- Dynamic execution (can bypass static detection) --
            "eval ", "exec ", "source ",
            // -- Interpreter inline execution (Level 1: exact match interpreter + -c/-e) --
            "python -c", "python3 -c", "perl -e", "node -e", "ruby -e",
            // -- Dangerous batch operations --
            "xargs ",      // xargs + dangerous target (e.g., xargs rm)
            "crontab",     // Cron job modification
            "nohup ",      // Background persistent execution
            // -- File overwrite redirect --
            // (Quote exclusion logic handled by function-level tokenizer, see function header comment)
            "> ",   // Overwrite redirect
            ">> ",  // Append redirect (writing to sensitive files like .bashrc)
        ];

        // Level 2 heuristic: `find` + `-exec` combo detection
        if cmd.contains("find ") && (cmd.contains("-exec") || cmd.contains("-execdir")) {
            return true;
        }

        // 2. Check direct patterns
        if dangerous_patterns.iter().any(|p| cmd.contains(p)) {
            return true;
        }

        // 3. Check pipe to dangerous commands (e.g., `cat file | sudo tee`, `echo x | sh`)
        let pipe_dangerous_targets = ["sh", "bash", "sudo", "tee", "eval", "exec"];
        if cmd.contains('|') {
            let segments: Vec<&str> = cmd.split('|').collect();
            for seg in &segments[1..] {
                let target = seg.trim().split_whitespace().next().unwrap_or("");
                if pipe_dangerous_targets.contains(&target) {
                    return true;
                }
            }
        }

        // 4. Check for dangerous commands in && / || chains
        let chain_ops = ["&&", "||", ";"];
        for op in &chain_ops {
            if cmd.contains(op) {
                for sub_cmd in cmd.split(op) {
                    let first_word = sub_cmd.trim().split_whitespace().next().unwrap_or("");
                    if ["rm", "sudo", "mkfs", "dd", "chmod", "chown", "eval", "exec"].contains(&first_word) {
                        return true;
                    }
                }
            }
        }

        false
    }
}
```

**CC Tool Mapping Table (CC has 52 tools; below are the core mappings)**

| CC Tool | Crab Tool | File |
|---------|----------|------|
| BashTool | BashTool | `bash.rs` |
| FileReadTool | ReadTool | `read.rs` |
| FileEditTool | EditTool | `edit.rs` |
| FileWriteTool | WriteTool | `write.rs` |
| GlobTool | GlobTool | `glob.rs` |
| GrepTool | GrepTool | `grep.rs` |
| WebSearchTool | WebSearchTool | `web_search.rs` |
| WebFetchTool | WebFetchTool | `web_fetch.rs` |
| AgentTool | AgentTool | `agent.rs` |
| NotebookEditTool | NotebookTool | `notebook.rs` |
| TaskCreateTool | TaskCreateTool | `task.rs` |
| MCPTool | McpToolAdapter | `mcp_tool.rs` |

**External Dependencies**: `crab-core`, `crab-config`, `crab-fs`, `crab-process`, `crab-sandbox`, `crab-mcp`

**Feature Flags**

```toml
[features]
default = ["pdf"]
pdf = ["pdf-extract"]
pty = ["portable-pty", "strip-ansi-escapes"]
```

---

### 6.10 `crates/session/` -- Session Management

**Responsibility**: State management for multi-turn conversations (corresponds to CC `src/services/compact/` + `src/services/SessionMemory/` + `src/services/sessionTranscript/`). Memory system extracted to `crates/memory/`; session re-exports core memory types.

**Directory Structure**

```
src/
├── lib.rs
├── conversation.rs    // Conversation state machine, multi-turn management
├── context.rs         // Context window management
├── compaction.rs      // Message compaction strategies
├── micro_compact.rs   // Micro-compaction: per-message replacement of large tool results
├── auto_compact.rs    // Auto-compaction trigger + cleanup
├── snip_compact.rs    // Snip compaction: "[snipped]" marker
├── input_expand.rs    // Input expansion (variable interpolation)
├── history.rs         // Session persistence, recovery, search, export
├── memory.rs          // Re-exports from crab-memory (MemoryStore, MemoryFile, etc.)
├── memory_extract.rs  // Conversation → memory extraction
├── cost.rs            // Token counting, cost tracking
├── template.rs        // Session template + quick recovery
└── migration.rs       // Data migration system
```

**Core Types**

```rust
// conversation.rs -- Conversation state machine
use crab_core::message::Message;
use crab_core::model::TokenUsage;

pub struct Conversation {
    /// Session ID
    pub id: String,
    /// System prompt
    pub system_prompt: String,
    /// Message history
    pub messages: Vec<Message>,
    /// Cumulative token usage
    pub total_usage: TokenUsage,
    /// Context window limit
    pub context_window: u64,
}

impl Conversation {
    pub fn new(id: String, system_prompt: String, context_window: u64) -> Self {
        Self {
            id,
            system_prompt,
            messages: Vec::new(),
            total_usage: TokenUsage::default(),
            context_window,
        }
    }

    /// Append a message
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Estimate current token count
    ///
    /// **Current**: Rough estimate of text_len/4 (error margin +/-30%), suitable for MVP phase
    /// **Future**: Integrate tiktoken-rs for precise counting (Claude tokenizer is compatible with cl100k_base)
    ///
    /// ```rust
    /// // TODO(M2+): Replace with precise counting
    /// // use tiktoken_rs::cl100k_base;
    /// // let bpe = cl100k_base().unwrap();
    /// // bpe.encode_with_special_tokens(text).len() as u64
    /// ```
    pub fn estimated_tokens(&self) -> u64 {
        let text_len: usize = self.messages.iter().map(|m| {
            m.content.iter().map(|c| match c {
                crab_core::message::ContentBlock::Text { text } => text.len(),
                _ => 100, // Fixed estimate for tool calls
            }).sum::<usize>()
        }).sum();
        (text_len / 4) as u64 // Temporary: +/-30% error margin
    }

    /// Whether compaction is needed
    pub fn needs_compaction(&self) -> bool {
        self.estimated_tokens() > self.context_window * 80 / 100
    }
}

// compaction.rs -- 5-level compaction strategy (progressively triggered by token usage rate)
pub enum CompactionStrategy {
    /// Level 1 (70-80%): Trim full output of old tool calls, keeping only summary lines
    Snip,
    /// Level 2 (80-85%): Replace large results (>500 tokens) with AI-generated single-line summary
    Microcompact,
    /// Level 3 (85-90%): Summarize old messages using a small model
    Summarize,
    /// Level 4 (90-95%): Keep recent N turns + summarize the rest
    Hybrid { keep_recent: usize },
    /// Level 5 (>95%): Emergency truncation, discard oldest messages
    Truncate,
}

use std::future::Future;
use std::pin::Pin;

/// Compaction client abstraction -- decouples compaction logic from specific API client
/// Facilitates testing (mock) and swapping different LLM providers
pub trait CompactionClient: Send + Sync {
    /// Send a compaction/summary request, return summary text
    fn summarize(
        &self,
        messages: &[crab_core::message::Message],
        instruction: &str,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<String>> + Send + '_>>;
}

// LlmBackend adapts to CompactionClient via enum dispatch (in crab-api)
// impl CompactionClient for LlmBackend { ... }

pub async fn compact(
    conversation: &mut Conversation,
    strategy: CompactionStrategy,
    client: &impl CompactionClient,
) -> crab_core::Result<()> {
    // Compact messages according to strategy, using client.summarize() to generate summaries
    // ...
}

// memory.rs -- Memory system
pub struct MemoryStore {
    pub path: std::path::PathBuf, // ~/.crab-code/memory/
}

impl MemoryStore {
    /// Save session memory
    pub fn save(&self, session_id: &str, content: &str) -> crab_core::Result<()> {
        // ...
    }

    /// Load session memory
    pub fn load(&self, session_id: &str) -> crab_core::Result<Option<String>> {
        // ...
    }
}
```

**External Dependencies**: `crab-core`, `crab-memory`

**Feature Flags**: None

---

### 6.11 `crates/swarm/` -- Multi-Agent Infrastructure

**Responsibility**: domain-pure building blocks for all multi-agent execution modes. Extracted from `agent/src/teams/` so that swarm primitives have zero engine/api/session coupling. Only depends on `core`.

**Directory Structure**

```
src/
├── lib.rs            // re-exports all public types
├── bus.rs            // MessageBus + AgentMessage / Envelope / event_channel
├── mailbox.rs        // MessageRouter (inter-agent message routing)
├── roster.rs         // Team / TeamMember / Capability / TeamMode
├── task_list.rs      // TaskList / Task / TaskStatus / SharedTaskList
├── task_lock.rs      // fd-lock file-locked claim_task / with_locked
├── retry.rs          // RetryPolicy / RetryTracker / BackoffStrategy
└── backend/          // Spawner backends
    ├── mod.rs        //   SwarmBackend trait + InProcessBackend
    ├── spawner.rs    //   SpawnerBackend trait
    └── teammate.rs   //   Teammate / TeammateConfig / TeammateState
```

`agent/src/teams/mod.rs` does `pub use crab_swarm::*;` to preserve the existing facade for higher layers.

**Feature Flags**: None

---

### 6.12 `crates/agent/` -- Orchestrator & Multi-Agent System

**Responsibility**: wraps the raw query loop (`crates/engine`) and adds session-aware orchestration — system prompt assembly, context injection (git/PR), error recovery, multi-agent coordination, REPL slash commands, file-history snapshots, conversation compaction. Corresponds to CC `QueryEngine.ts` + `coordinator/` + `tasks/` + `services/compact/` + `utils/fileHistory.ts`. **Does not** contain the low-level message loop (that moved to `crates/engine`, see §6.21).

**Directory Structure** 

```
src/
├── lib.rs
├── runtime.rs               // Top-level agent runtime entry
├── teams/                   // Layer 1 orchestration (re-exports crab_swarm::*)
│   ├── mod.rs               //   pub use crab_swarm::*; + local re-exports
│   ├── coordinator.rs       //   TeamCoordinator (Layer 2b glue)
│   ├── worker.rs            //   AgentWorker (sub-agent runner, depends on engine)
│   └── worker_pool.rs       //   WorkerPool (spawn / collect / cancel / retry)
│
├── coordinator/             // Layer 2b Coordinator Mode (gated on CRAB_COORDINATOR_MODE)
│   ├── mod.rs               //   Coordinator struct: apply(ToolRegistry, &mut prompt)
│   ├── gating.rs            //   env + config gate
│   ├── tool_acl.rs          //   COORDINATOR_TOOLS + WORKER_DENIED_TOOLS constants
│   ├── prompt.rs            //   Anti-pattern prompt overlay ("understand before delegating")
│   └── permission_sync.rs   //   Cross-teammate permission sync
│
├── session/                 // Layer 3 session runtime
│   ├── mod.rs
│   ├── runtime.rs           //   AgentSession + CoordinatorContext + compact_conversation
│   └── session_config.rs    //   SessionConfig (flat value struct)
│
├── system_prompt/           // Modular prompt assembly
│   ├── mod.rs
│   ├── builder.rs           //   build_system_prompt_with_memories
│   ├── git_context.rs       //   Git metadata injection
│   ├── pr_context.rs        //   gh PR context
│   └── tips.rs              //   Contextual tips
│
├── file_history/            // CCB fileHistory equivalent
│   ├── mod.rs
│   └── snapshot.rs          //   FileHistory + Snapshot + rewind / LRU(100)
│
├── error_recovery/          // Classification + recovery strategy
│   ├── mod.rs
│   ├── category.rs          //   ErrorCategory + ErrorClassifier
│   └── strategy.rs          //   Retry / AskUser / Abort
│
├── slash_commands/          // 33 built-ins + registry (wired into REPL)
│   ├── mod.rs
│   ├── types.rs             //   Registry + Context + Result + SlashAction
│   └── handlers.rs          //   cmd_* built-in handlers
│
├── summarizer.rs            // Conversation compaction (/compact, auto at 80%)
├── repl_commands.rs         // ReplCommand enum + parser
├── auto_dream.rs            // Background memory consolidation (cargo feature `auto-dream`)
└── proactive/               // CCB feature('PROACTIVE') placeholder (cargo feature `proactive`)
    ├── mod.rs
    ├── mini_agent.rs
    ├── suggestion.rs
    └── cache.rs
```

Cargo features: `auto-dream` (off), `proactive` (off), `mem-ranker` (off, re-exports `crab-memory/mem-ranker`).

The raw message loop, stop hooks, token budget, and effort mapping live in `crates/engine` (§6.21), not here.

**Message Loop (Core)**

```rust
// query_loop.rs -- Core message loop
// Corresponds to CC src/query.ts query() function
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message};
use crab_session::Conversation;
use crab_tools::executor::ToolExecutor;
use crab_api::LlmBackend;
use tokio::sync::mpsc;

/// Message loop: user input -> API -> tool execution -> continue -> until no tool calls
pub async fn query_loop(
    conversation: &mut Conversation,
    api: &LlmBackend,
    tools: &ToolExecutor,
    event_tx: mpsc::Sender<Event>,
) -> crab_core::Result<()> {
    loop {
        // 1. Check if context needs compaction
        if conversation.needs_compaction() {
            // -> See [session#compaction]
            todo!("compact conversation");
        }

        // 2. Build API request (borrow messages to avoid clone)
        let req = crab_api::MessageRequest {
            model: crab_core::model::ModelId("claude-sonnet-4-20250514".into()),
            messages: std::borrow::Cow::Borrowed(&conversation.messages),
            system: Some(conversation.system_prompt.clone()),
            max_tokens: 16384,
            tools: tools.registry().tool_schemas(),
            temperature: None,
        };

        // 3. Stream to API
        let mut stream = api.stream_message(req);

        // 4. Collect assistant response
        let mut assistant_content: Vec<ContentBlock> = Vec::new();
        let mut has_tool_use = false;

        // (streaming processing details omitted, collecting ContentBlocks)
        // ...

        // 5. Add assistant message to conversation
        conversation.push(Message {
            role: crab_core::message::Role::Assistant,
            content: assistant_content.clone(),
        });

        // 6. If no tool calls, loop ends
        if !has_tool_use {
            break;
        }

        // 7. Partition tool calls by read/write and execute concurrently
        //    Read tools (is_read_only=true) use FuturesUnordered concurrently (max 10)
        //    Write tools execute serially to ensure ordering consistency
        let tool_calls: Vec<_> = assistant_content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
                _ => None,
            })
            .collect();

        let (read_tools, write_tools) = partition_tools(&tool_calls, &tools);

        let cancel = tokio_util::sync::CancellationToken::new();
        let ctx = crab_core::tool::ToolContext {
            working_dir: std::env::current_dir()?,
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_id: conversation.id.clone(),
            cancellation_token: cancel.clone(),
            permission_policy: crab_core::permission::PermissionPolicy {
                mode: crab_core::permission::PermissionMode::Default,
                allowed_tools: Vec::new(),
                denied_tools: Vec::new(),
            },
        };

        // 7a. Execute read tools concurrently (max 10 concurrent)
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        {
            use futures::stream::{FuturesUnordered, StreamExt};
            let mut futures = FuturesUnordered::new();
            let semaphore = Arc::new(tokio::sync::Semaphore::new(10));

            for (id, name, input) in &read_tools {
                let permit = semaphore.clone().acquire_owned().await?;
                let id = (*id).clone();
                let name = (*name).clone();
                let input = (*input).clone();
                let tools = tools.clone();
                let ctx = ctx.clone();
                let event_tx = event_tx.clone();
                futures.push(tokio::spawn(async move {
                    event_tx.send(Event::ToolUseStart {
                        id: id.clone(), name: name.clone(),
                    }).await.ok();
                    let output = tools.execute(&name, input, &ctx).await;
                    drop(permit);
                    (id, output)
                }));
            }
            while let Some(result) = futures.next().await {
                let (id, output) = result?;
                let output = output?;
                event_tx.send(Event::ToolResult {
                    id: id.clone(), content: output.text(), is_error: output.is_error,
                }).await.ok();
                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id, content: output.text(), is_error: output.is_error,
                });
            }
        }

        // 7b. Execute write tools serially
        for (id, name, input) in &write_tools {
            event_tx.send(Event::ToolUseStart {
                id: (*id).clone(), name: (*name).clone(),
            }).await.ok();
            let output = tools.execute(name, (*input).clone(), &ctx).await?;
            event_tx.send(Event::ToolResult {
                id: (*id).clone(), content: output.text(), is_error: output.is_error,
            }).await.ok();
            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: (*id).clone(), content: output.text(), is_error: output.is_error,
            });
        }

        // 8. Add tool results as a user message to the conversation
        conversation.push(Message {
            role: crab_core::message::Role::User,
            content: tool_results,
        });

        // 9. Return to step 1, continue loop
    }

    Ok(())
}
```

```rust
/// Partition tool calls into read/write groups by is_read_only()
fn partition_tools<'a>(
    calls: &[(&'a String, &'a String, &'a serde_json::Value)],
    executor: &ToolExecutor,
) -> (
    Vec<(&'a String, &'a String, &'a serde_json::Value)>,
    Vec<(&'a String, &'a String, &'a serde_json::Value)>,
) {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for &(id, name, input) in calls {
        if executor.registry().get(name).map_or(false, |t| t.is_read_only()) {
            reads.push((id, name, input));
        } else {
            writes.push((id, name, input));
        }
    }
    (reads, writes)
}
```

```rust
// coordinator.rs -- Multi-Agent orchestration
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Running worker handle
pub struct RunningWorker {
    pub worker_id: String,
    pub cancel: CancellationToken,
    pub handle: tokio::task::JoinHandle<WorkerResult>,
}

/// Multi sub-agent orchestrator
pub struct AgentCoordinator {
    backend: Arc<LlmBackend>,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryLoopConfig,
    event_tx: mpsc::Sender<Event>,
    running: HashMap<String, RunningWorker>,
    completed: Vec<WorkerResult>,           // Summary (without conversation history)
    cancel: CancellationToken,
}

impl AgentCoordinator {
    /// Spawn a new sub-agent worker
    pub async fn spawn_worker(
        &mut self,
        config: WorkerConfig,
        task_prompt: String,
    ) -> crab_core::Result<String>;

    /// Wait for a specific worker to complete
    pub async fn wait_for(&mut self, worker_id: &str) -> Option<WorkerResult>;

    /// Wait for all workers to complete
    pub async fn wait_all(&mut self) -> Vec<WorkerResult>;

    /// Cancel a specific worker
    pub fn cancel_worker(&mut self, worker_id: &str) -> bool;

    /// Cancel all workers
    pub fn cancel_all(&mut self);
}

// worker.rs -- Sub-agent worker lifecycle
pub struct WorkerConfig {
    pub worker_id: String,
    pub system_prompt: String,
    pub max_turns: Option<usize>,
    pub max_duration: Option<std::time::Duration>,
    pub context_window: u64,
}

pub struct WorkerResult {
    pub worker_id: String,
    pub output: Option<String>,             // Last assistant text message
    pub success: bool,
    pub usage: TokenUsage,
    pub conversation: Conversation,         // Full conversation history
}

pub struct AgentWorker {
    config: WorkerConfig,
    backend: Arc<LlmBackend>,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryLoopConfig,
    event_tx: mpsc::Sender<Event>,
    cancel: CancellationToken,
}

impl AgentWorker {
    /// Run an independent query loop in a tokio task
    pub fn spawn(self, task_prompt: String) -> tokio::task::JoinHandle<WorkerResult>;
}
```

**Task System (Implemented)**

```rust
// task.rs -- TaskStore + TaskItem + dependency graph
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskItem {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub metadata: serde_json::Value,
    pub blocks: Vec<String>,         // Tasks blocked by this task
    pub blocked_by: Vec<String>,     // Tasks that block this task
}

/// Thread-safe task store with dependency graph support
pub struct TaskStore {
    items: HashMap<String, TaskItem>,
    next_id: usize,
}

impl TaskStore {
    pub fn create(&mut self, subject: String, description: String, ...) -> TaskItem;
    pub fn get(&self, id: &str) -> Option<&TaskItem>;
    pub fn list(&self) -> Vec<&TaskItem>;
    pub fn update(&mut self, id: &str, ...) -> Option<String>;
    pub fn add_dependency(&mut self, task_id: &str, blocked_by_id: &str);
}
```

**Streaming Tool Execution (StreamingToolExecutor)**

CC starts tool execution immediately once the `tool_use` JSON is fully parsed during API streaming,
without waiting for the `message_stop` event. Crab Code should implement the same optimization:

```
API SSE stream:  [content_block_start: tool_use] -> [input_json_delta...] -> [content_block_stop]
                                                                                  |
                                                JSON complete -> spawn immediately -+
                                                               |
Subsequent blocks continue streaming <---- parallel with tool execution ------->| tool result ready
```

```rust
/// Streaming tool executor -- starts tools early during API streaming
pub struct StreamingToolExecutor {
    pending: Vec<tokio::task::JoinHandle<(String, crab_core::Result<ToolOutput>)>>,
}

impl StreamingToolExecutor {
    /// Called immediately when a tool_use block's JSON is fully parsed
    pub fn spawn_early(&mut self, id: String, name: String, input: Value, ctx: ToolContext, executor: Arc<ToolExecutor>) {
        let handle = tokio::spawn(async move {
            let result = executor.execute(&name, input, &ctx).await;
            (id, result)
        });
        self.pending.push(handle);
    }

    /// After message_stop, collect all completed/in-progress tool results
    pub async fn collect_all(&mut self) -> Vec<(String, crab_core::Result<ToolOutput>)> {
        let mut results = Vec::new();
        for handle in self.pending.drain(..) {
            results.push(handle.await.expect("tool task panicked"));
        }
        results
    }
}
```

**External Dependencies**: `crab-common`, `crab-core`, `crab-config`, `crab-engine`, `crab-memory`, `crab-session`, `crab-tools`, `crab-api`, `crab-mcp`, `crab-plugin`, `crab-swarm`, `crab-skill`, `tokio`, `tokio-util`, `futures`

**Feature Flags**

```toml
[features]
default    = []
mem-ranker = ["crab-memory/mem-ranker"]
auto-dream = []
proactive  = []
```

---

### 6.13 `crates/tui/` -- Terminal UI

**Layer**: Layer 3 Engine.

**Responsibility**: All terminal interface rendering (corresponds to CC `src/components/` + `src/screens/` + `src/ink/` + `src/vim/` + `src/bridge/bridgeUI.ts`).

CC uses React/Ink to render the terminal UI; Crab uses ratatui + crossterm to achieve equivalent experience. Control flow between tui and other Layer 3 crates (agent / session / remote / engine) follows Rule 6 (§5.3): state is consumed via `core::Event` broadcasts. Read-only access to `session::Conversation` and cost accumulators is allowed.

**Architecture overview**:

The TUI is organized into 9 top-level module directories plus core files. Modules are grouped by concern:

- **Core loop**: `app/` (state machine + update + commands, split into `mod.rs` / `state.rs` / `update.rs` / `commands.rs`), `runner/` (terminal init + event loop + slash dispatch, split into `mod.rs` / `init.rs` / `event_loop.rs` / `slash.rs`), `event.rs` / `app_event.rs` / `event_broker.rs` (event pipeline), `layout.rs` (responsive panel allocation), `frame_requester.rs` (redraw coalescing)
- **Action dispatch**: `action.rs` (single `Action` enum with `serde::Serialize` + `schemars::JsonSchema` derives, used by keybinding resolver and potential multi-frontend JSON-RPC)
- **Keybinding system** (`keybindings/`): chord-aware resolver with `KeySequenceParser`, 18 `KeyContext` variants, TOML user overrides at `~/.crab/keybindings.toml`
- **Overlay system** (`overlay/`): `OverlayKind` enum dispatching `handle_key` / `render` / `contexts` / `name`
- **Theme** (`theme/`): ~120 semantic color fields, shimmer derivation, 8-slot agent palette, brand accents, OSC 10/11 background detection, dark/light auto-switching
- **Animation** (`animation/`): `FrameScheduler`, braille/dots/line `Spinner`, `ShimmerState` (per-column color lookup)
- **Markdown** (`markdown/`): LRU cache keyed by (content, theme, width), background `syntect` highlighting thread, GFM table renderer
- **Vim** (`vim/`): 5-file key-handling state machine (mode / motion / operator / handler / mod), supports Normal/Insert/Visual/Command modes with operator-motion composition
- **Components** (`components/`): ~55 higher-level views including input_area, message_list, header, bottom_bar, virtual_list, call_card, permission, autocomplete, command_palette, toast_queue, notification_banner, token_warning, message_pill, sticky_header, update_banner, context_visualization, prompt_chips, message_actions, at_mention, and more

**Directory Structure**

```
src/
├── lib.rs
├── action.rs                  // Action enum (Serialize + JsonSchema)
├── app/                       // App state machine
│   ├── mod.rs                 //   App struct, new(), render, re-exports
│   ├── state.rs               //   AppState, ThinkingState, PromptInputMode, AppAction
│   ├── update.rs              //   apply_action() + event handlers
│   └── commands.rs            //   Slash command handling
├── app_event.rs               // App-level event enum
├── changelog.rs               // Changelog display
├── clipboard.rs               // Clipboard integration
├── command_queue.rs            // Queued command execution
├── event.rs                   // crossterm Event -> AppEvent mapping
├── event_broker.rs            // Internal event bus
├── frame_requester.rs         // Redraw coalescing
├── global_state.rs            // User global preferences (migrated from config)
├── hyperlink.rs               // OSC 8 hyperlink support
├── layout.rs                  // Layout calculation (panel allocation, responsive)
├── runner/                    // TUI runner
│   ├── mod.rs                 //   run() skeleton + ExitInfo + TuiConfig
│   ├── init.rs                //   Terminal setup + App initialization
│   ├── event_loop.rs          //   Main event loop + input polling
│   └── slash.rs               //   Slash command infrastructure
├── terminal_notify.rs         // Desktop notification bridge
├── traits.rs                  // Renderable trait
├── watcher.rs                 // File watcher (config reload)
│
├── keybindings/               // Chord-aware keybinding system
│   ├── mod.rs
│   ├── types.rs               // Action / KeyContext (18) / KeyChord / Sequence
│   ├── parser.rs              // "ctrl+k ctrl+s" text → Sequence
│   ├── resolver.rs            // Feed-per-key resolver with chord + timeout
│   ├── sequence.rs            // KeySequenceParser (multi-key sequences)
│   ├── defaults.rs            // Built-in bindings per context
│   └── config.rs              // ~/.crab/keybindings.toml user overrides
│
├── overlay/                   // Modal overlay system
│   ├── mod.rs                 // OverlayKind enum + re-exports
│   └── kind.rs                // State structs, key handlers, render functions
│
├── theme/                     // Color + brand semantics
│   ├── mod.rs                 // Theme struct + palette switcher
│   ├── current.rs             // Global current theme accessor
│   ├── accents.rs             // Permission/fast-mode/brief-label colors
│   ├── agents.rs              // 8-slot agent accent palette (dark + light)
│   ├── osc.rs                 // OSC 10/11 system-color probe + classifier
│   └── shimmer.rs             // Shimmer lift derivation from any base color
│
├── animation/                 // Frame-scheduled animations
│   ├── mod.rs                 // FrameScheduler + subscribe/ticket
│   ├── shimmer.rs             // ShimmerState + per-column color lookup
│   └── spinner.rs             // Spinner (braille / dots / line / custom)
│
├── markdown/                  // Cached markdown renderer
│   ├── mod.rs                 // CachedMarkdownRenderer façade
│   ├── cache.rs               // LRU keyed by (content, theme, width)
│   ├── highlight.rs           // Background syntect worker + HighlightJob
│   └── table.rs               // GFM table with column-aligned cells
│
├── components/                // Higher-level views (~55 files)
│   ├── mod.rs
│   ├── ansi.rs                // ANSI escape -> ratatui Span conversion
│   ├── approval_queue.rs      // Pending permission queue
│   ├── at_mention.rs          // @ file mention UI
│   ├── autocomplete.rs        // Autocomplete popup
│   ├── bottom_bar.rs          // Bottom status bar
│   ├── call_card.rs           // Foldable tool-call card
│   ├── code_block.rs          // Code block + copy affordance
│   ├── command_palette.rs     // Command palette (fuzzy)
│   ├── config_browser.rs      // Config browser panel
│   ├── context_collapse.rs    // Long-context fold view
│   ├── context_visualization.rs // Compaction stats display
│   ├── diff.rs                // Diff visualization
│   ├── diff_viewer.rs         // Diff viewer panel
│   ├── fuzzy.rs               // Fuzzy match primitive
│   ├── global_search.rs       // Global search dialog
│   ├── header.rs              // Top header bar
│   ├── history_search.rs      // Ctrl+R history search overlay
│   ├── input.rs               // Text input (single/multi-line)
│   ├── input_area.rs          // Input area shell (ghost text, Vim mode)
│   ├── loading.rs             // Loading placeholder
│   ├── markdown.rs            // Base pulldown-cmark → ratatui renderer
│   ├── mcp_browser.rs         // MCP server browser panel
│   ├── memory_browser.rs      // Memory browser panel
│   ├── message_actions.rs     // Per-message action buttons
│   ├── message_list.rs        // Chronological message list
│   ├── message_pill.rs        // "N new messages" / "Jump to bottom" pill
│   ├── model_picker.rs        // Model switcher overlay
│   ├── notification.rs        // Toast notification system
│   ├── notification_banner.rs // Persistent sticky banners
│   ├── output_styles.rs       // Shared styling helpers
│   ├── permission.rs          // Permission dialog
│   ├── permissions_browser.rs // Permission rules browser
│   ├── plan_card.rs           // Plan display card
│   ├── progress_indicator.rs  // Progress bar
│   ├── prompt_chips.rs        // Mode / context chips on prompt line
│   ├── resume_browser.rs      // Resume session browser
│   ├── search.rs              // In-conversation search
│   ├── select.rs              // Selection list
│   ├── session_sidebar.rs     // Session sidebar
│   ├── shortcut_hint.rs       // Key hint strip
│   ├── spinner.rs             // Spinner data adapter
│   ├── status_bar.rs          // Status bar
│   ├── status_line.rs         // One-line status slot
│   ├── sticky_header.rs       // Pinned user prompt on scroll-up
│   ├── syntax.rs              // syntect-backed code highlight
│   ├── tab_bar.rs             // Tab strip
│   ├── task_list.rs           // Task panel
│   ├── team_browser.rs        // Team browser panel
│   ├── text_utils.rs          // Text helpers
│   ├── toast_queue.rs         // Timed notification toasts (3 max visible)
│   ├── token_warning.rs       // Context budget alerts (80%/90%)
│   ├── tool_output.rs         // Collapsible tool output
│   ├── transcript_overlay.rs  // Transcript overlay host
│   ├── trust_dialog.rs        // Trust / security dialog
│   ├── update_banner.rs       // Auto-update status display
│   └── virtual_list.rs        // Viewport-sliced, width-keyed LRU list
│
├── history/                   // Session history display
│   ├── mod.rs
│   ├── grouping.rs            // History grouping logic
│   └── cells/                 // Per-message-type cell renderers
│       ├── mod.rs
│       ├── assistant.rs       // Assistant message cell
│       ├── collapsed_read_search.rs // Collapsed read/search cell
│       ├── compact_boundary.rs // Compaction boundary marker
│       ├── plan_step.rs       // Plan step cell
│       ├── system.rs          // System message cell
│       ├── thinking.rs        // Thinking block cell
│       ├── tool_call.rs       // Tool call cell
│       ├── tool_rejected.rs   // Rejected tool cell
│       ├── tool_result.rs     // Tool result cell
│       ├── user.rs            // User message cell
│       └── welcome.rs         // Welcome screen cell
│
└── vim/                       // Vim mode (top-level, sibling of keybindings/theme)
    ├── mod.rs
    ├── handler.rs             // Event handler integration
    ├── mode.rs                // Normal/Insert/Visual/Command
    ├── motion.rs              // hjkl, w/b/e, 0/$, gg/G, f/t
    └── operator.rs            // d/c/y + motion composition
```

**Key design decisions**:

- `OverlayKind` is a flat enum (not trait objects) dispatching `handle_key` / `render` / `contexts` / `name`. Shared handler helpers reduce duplication across variants.
- The `Action` enum derives `schemars::JsonSchema` to support future multi-frontend (CLI / IDE / web) dispatch via JSON-RPC.
- Keybinding config uses TOML at `~/.crab/keybindings.toml` with `Action` variant names that round-trip through serde.

**External Dependencies**: `crab-common`, `crab-core`, `crab-config`, `crab-agent`, `crab-memory`, `ratatui`, `crossterm`, `syntect`, `pulldown-cmark`, `schemars`

> tui does not directly depend on tools; it receives tool execution state via the `crab_core::Event` enum, with crates/cli responsible for assembling agent+tui.

**Feature Flags**: None (tui itself is an optional dependency of cli)

---

### 6.14 `crates/skill/` -- Skill System

**Responsibility**: Skill discovery, loading, registry, and built-in skill definitions (corresponds to CC `src/skills/`)

**Directory Structure**

```
src/
├── lib.rs            // Public API re-exports
├── types.rs          // Skill, SkillTrigger, SkillContext, SkillSource
├── frontmatter.rs    // YAML frontmatter parsing from .md files
├── registry.rs       // SkillRegistry (discover, register, find, match)
├── matcher.rs        // Skill matching logic
├── builder.rs        // SkillBuilder fluent API
└── bundled/
    ├── mod.rs         // bundled_skills() + BUNDLED_SKILL_NAMES
    ├── commit.rs      // /commit
    ├── review_pr.rs   // /review-pr
    ├── debug.rs       // /debug
    ├── loop_skill.rs  // /loop
    ├── remember.rs    // /remember
    ├── schedule.rs    // /schedule
    ├── simplify.rs    // /simplify
    ├── stuck.rs       // /stuck
    ├── verify.rs      // /verify
    └── update_config.rs // /update-config
```

**External Dependencies**: `crab-core`, `serde`, `serde_json`, `regex`, `tracing`

---

### 6.15 `crates/plugin/` -- Plugin System

**Responsibility**: Plugin lifecycle, hooks, WASM sandbox, MCP↔skill bridge (corresponds to CC `src/services/plugins/`)

**Directory Structure**

```
src/
├── lib.rs
├── skill_builder.rs      // MCP → Skill bridge (load_mcp_skills)
├── hook.rs               // Lifecycle hook execution
├── hook_registry.rs      // Hook registry
├── hook_types.rs         // Hook type definitions
├── hook_watchers.rs      // File watcher hooks
├── frontmatter_hooks.rs  // Parse hooks from skill YAML frontmatter
├── manager.rs            // Plugin discovery and lifecycle
├── manifest.rs           // Plugin manifest parsing
└── wasm_runtime.rs       // WASM plugin sandbox (wasmtime, feature = "wasm")
```

**Hook Triggers**: `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop` (fires when the query loop is about to exit — no tool calls in the response)

**Hook Actions**: `Allow` (default), `Deny` (block execution), `Modify` (alter tool input), `Retry` (request the query loop to continue instead of stopping; used by Stop hooks)

**Action priority**: Deny > Retry > Modify > Allow — when multiple hooks return different actions, the highest-priority action wins.

**External Dependencies**: `crab-core`, `crab-config`, `crab-mcp`, `crab-process`, `crab-skill`, `wasmtime` (optional)

**Feature Flags**

```toml
[features]
default = []
wasm = ["wasmtime"]
```

---

### 6.16 `crates/memory/` -- Persistent Memory System

**Responsibility**: File-based cross-session memory storage — user preferences, feedback, project context, external references (corresponds to CC `src/memdir/`)

**Directory Structure**

```
src/
├── lib.rs              // Public API re-exports
├── types.rs            // MemoryType enum, MemoryMetadata, frontmatter parsing
├── store.rs            // MemoryStore — file CRUD + mtime-sorted scan
├── index.rs            // MEMORY.md index read/write + truncation (200 lines / 25KB)
├── relevance.rs        // MemorySelector keyword scoring + MemoryRanker trait
├── age.rs              // Exponential decay scoring (30-day half-life, SystemTime)
├── paths.rs            // Per-project / global / team memory directory resolution
├── security.rs         // Path traversal / symlink / null byte validation
├── prompt.rs           // MemoryPromptBuilder — system prompt injection
├── team.rs             // TeamMemoryStore — shared team memory with slugified filenames
├── agents_md.rs        // AGENTS.md discovery + parsing (migrated from config)
└── ranker.rs           // LlmMemoryRanker — Sonnet sidequery (feature = "mem-ranker")
```

**External Dependencies**: `crab-common`, `crab-core`, `serde`, `serde_json`, `serde_yaml_ng`, `dunce`. Optional: `crab-api`, `tokio` (with `mem-ranker` feature)

**Feature Flags**

```toml
[features]
default = []
mem-ranker = ["dep:crab-api", "dep:tokio"]                   # LLM-driven memory selection
```

**Key Types**: `MemoryType` (User/Feedback/Project/Reference), `MemoryMetadata`, `MemoryFile`, `MemoryStore`, `MemorySelector`, `MemoryRanker` (trait), `LlmMemoryRanker` (impl, feature-gated), `MemoryPromptBuilder`, `TeamMemoryStore`

---

### 6.17 `crates/telemetry/` -- Observability

**Responsibility**: Distributed tracing and metrics collection (corresponds to CC `src/services/analytics/` + `src/services/diagnosticTracking.ts`)

**Directory Structure**

```
src/
├── lib.rs
├── tracer.rs         // OpenTelemetry tracer initialization
├── metrics.rs        // Custom metrics (API latency, tool execution time, etc.)
├── cost.rs           // Cost tracking
├── export.rs         // OTLP export
└── session_recorder.rs // Session recording (local transcript)
```

**Core Interface**

```rust
// tracer.rs
use tracing_subscriber::prelude::*;

/// Initialize the tracing system
pub fn init(service_name: &str, endpoint: Option<&str>) -> crab_core::Result<()> {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .compact();

    let registry = tracing_subscriber::registry().with(fmt_layer);

    #[cfg(feature = "otlp")]
    if let Some(endpoint) = endpoint {
        let _tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(endpoint),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio)?;
        // Add OpenTelemetry layer to registry
    }

    #[cfg(not(feature = "otlp"))]
    let _ = (service_name, endpoint); // Suppress unused warnings

    registry.init();
    Ok(())
}
```

**External Dependencies**: `crab-common`, `crab-core`, `tracing`, `tracing-subscriber`; OTLP-related are optional dependencies

**Feature Flags**

```toml
[features]
default = ["fmt"]
fmt = ["tracing-subscriber/fmt"]                               # Local log formatting (default)
otlp = [                                                       # OpenTelemetry OTLP export
    "opentelemetry",
    "opentelemetry-otlp",
    "opentelemetry_sdk",
    "tracing-opentelemetry",
]
```

> By default, only `fmt` is enabled (local tracing-subscriber), without pulling in the full opentelemetry stack.
> Production deployments needing OTLP export can enable it with `cargo build -F otlp`.

---

### 6.18 `crates/cli/` -- Terminal Entry Point

**Responsibility**: An extremely thin binary entry point that only does assembly with no business logic (corresponds to CC `src/entrypoints/cli.tsx`)

**Directory Structure**

```
src/
├── main.rs           // #[tokio::main] thin entry point
├── args.rs           // Cli struct + clap definitions + OutputFormat
├── agent.rs          // run() + run_single_shot() + run_line_repl() + model/tool resolution
├── output.rs         // event_to_json() + Spinner + print_banner()
├── acp_mode.rs       // ACP server mode entry
├── completions.rs    // Shell completion generation
├── deep_link.rs      // Deep link protocol handler
├── installer.rs      // System installer
└── commands/         // clap subcommand definitions
    ├── mod.rs
    ├── agents.rs     // AGENTS.md management
    ├── auth.rs       // Authentication management
    ├── config.rs     // Configuration management (crab config set/get)
    ├── doctor.rs     // Diagnostic checks
    ├── permissions.rs // Permission rule management
    ├── plugin.rs     // Plugin management
    ├── serve.rs      // Serve mode
    ├── session.rs    // ps, logs, attach, kill
    └── update.rs     // Self-update
```

**Panic Hook Design**

```rust
// main.rs -- Terminal state recovery panic hook
// Must be registered after terminal.init() and before entering the main loop
pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // 1. Restore terminal state (most important -- otherwise terminal becomes unusable)
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        // 2. Call original hook (print panic info)
        original_hook(panic_info);
        // Recommended alternative: use color-eyre::install() for automatic handling,
        // providing beautified panic reports + backtrace
    }));
}
```

**Entry Point Code**

```rust
// main.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "crab", version, about = "AI coding assistant")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Pass prompt directly (equivalent to crab run -p)
    #[arg(short, long)]
    prompt: Option<String>,

    /// Permission mode
    #[arg(long, default_value = "default")]
    permission_mode: String,

    /// Specify model
    #[arg(long)]
    model: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive mode (default)
    Chat,
    /// Single execution
    Run {
        #[arg(short, long)]
        prompt: String,
    },
    /// Session management
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// MCP mode
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 1. Initialize telemetry
    crab_telemetry::init("crab-code", None)?;

    // 2. Load configuration
    let ctx = crab_config::ResolveContext::new().with_process_env();
    let config = crab_config::resolve(&ctx)?;

    // 3. Initialize authentication (out-of-chain; never reads Config secrets)
    let auth = crab_auth::resolve_auth_key(&config)
        .ok_or_else(|| anyhow::anyhow!("no API key found"))?;

    // 4. Dispatch commands
    match cli.command.unwrap_or(Commands::Chat) {
        Commands::Chat => {
            // Start interactive mode
            // ...
        }
        Commands::Run { prompt } => {
            // Single execution
            // ...
        }
        _ => { /* ... */ }
    }

    Ok(())
}
```

**External Dependencies**: All crates, `clap`, `tokio`, `anyhow`

**Feature Flags**

```toml
[features]
default = ["tui"]
tui = ["crab-tui"]
full = ["tui", "crab-plugin/wasm", "crab-api/bedrock", "crab-api/vertex", "crab-process/pty", "crab-telemetry/otlp", "crab-agent/mem-ranker"]
```

---

### 6.19 `crates/daemon/` -- Headless Composition Root

**Responsibility**: The headless entry point — opposite of `cli`. Where `cli` is the interactive composition root (brings up `engine + agent + tui + ide-client + ...`), `daemon` is the headless one: it hosts the **server-side** protocols (`remote-server`, `mcp-server`, `acp-server`) and the `job` scheduler, without pulling `tui` or any of its deps (ratatui / crossterm / unicode-width). This is what web / app / desktop clients attach to; it is also the natural target for systemd / Docker deployments.

**Split rationale**: the decision between `daemon` and "`crab daemon` subcommand of cli" came down to deps. A headless server image should not ship ratatui. Keeping `daemon` as a separate binary lets the `cargo install crab-daemon` path produce a small artifact.

**Directory Structure**

```
src/
├── lib.rs
├── main.rs
├── protocol.rs            // IPC message protocol
├── server.rs              // Daemon server
└── session_pool.rs        // Session pool management
```

**IPC Communication Design**

```
CLI <--- Unix socket (Linux/macOS) / Named pipe (Windows) ---> Daemon
         Protocol: length-prefixed frames + JSON messages
         Format: [4 bytes: payload_len_le32][payload_json]
```

**IPC Message Protocol**

```rust
/// CLI -> Daemon request
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonRequest {
    /// Create new session or attach to existing session
    Attach { session_id: Option<String>, working_dir: PathBuf },
    /// Disconnect but keep session running
    Detach { session_id: String },
    /// List active sessions
    ListSessions,
    /// Terminate session
    KillSession { session_id: String },
    /// Send user input
    UserInput { session_id: String, content: String },
    /// Health check
    Ping,
}

/// Daemon -> CLI response/event push
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonResponse {
    /// Attach successful
    Attached { session_id: String },
    /// Session list
    Sessions { list: Vec<SessionInfo> },
    /// Forward agent Event (streaming push)
    Event(crab_core::event::Event),
    /// Error
    Error { message: String },
    /// Pong
    Pong,
}
```

**Session Pool Management**

```rust
pub struct SessionPool {
    /// Active sessions (max N, default 8)
    sessions: HashMap<String, SessionHandle>,
    /// Shared API connection pool (reused across all sessions)
    api_client: Arc<LlmBackend>,
    /// Idle timeout auto-cleanup (default 30 minutes)
    idle_timeout: Duration,
}

pub struct SessionHandle {
    pub id: String,
    pub working_dir: PathBuf,
    pub created_at: Instant,
    pub last_active: Instant,
    /// Whether a CLI is currently connected
    pub attached: bool,
    /// Session control channel
    pub tx: mpsc::Sender<DaemonRequest>,
}
```

**CLI Attach/Detach Flow**

```
1. CLI starts -> connects to daemon socket
2. Sends Attach { session_id: None } -> daemon creates new session
3. Daemon replies Attached { session_id: "xxx" }
4. CLI sends UserInput -> daemon forwards to query_loop
5. Daemon streams Event -> CLI renders
6. CLI exits -> sends Detach -> session continues running in background
7. CLI re-attaches Attach { session_id: "xxx" } -> resumes conversation
```

**Core Logic**

```rust
// main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 0. Log initialization -- use tracing-appender for log rotation
    //    daemon is a long-running process, must have log rotation to prevent disk from filling up
    let log_dir = directories::ProjectDirs::from("", "", "crab-code")
        .expect("failed to resolve project dirs")
        .data_dir()
        .join("logs");
    let file_appender = tracing_appender::rolling::daily(&log_dir, "daemon.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false) // File logs don't need ANSI colors
        .init();

    // 1. PID file + single instance check (fd-lock)
    // 2. Initialize shared API connection pool
    // 3. Create SessionPool
    // 4. Listen on IPC socket
    // 5. Accept loop: spawn independent handler for each CLI connection
    // 6. Periodically clean up idle sessions
    // ...
}
```

**External Dependencies**: `crab-common`, `crab-core`

---

### 6.20 Global State Split: AppConfig / AppRuntime

Global state shared by CLI and Daemon is split into **immutable configuration** and **mutable runtime** halves,
avoiding a single `Arc<RwLock<AppState>>` where read paths get blocked by write locks.

```rust
/// Immutable configuration -- initialized at startup, unchanged during runtime
/// Arc<AppConfig> shared with zero locks, readable by any thread/task
pub struct AppConfig {
    /// Merged settings.json
    pub settings: crab_config::Settings,
    /// AGENTS.md content (global + user + project)
    pub agents_md: Vec<crab_memory::agents_md::AgentsMd>,
    /// Permission policy
    pub permission_policy: crab_core::permission::PermissionPolicy,
    /// Model configuration
    pub model_id: crab_core::model::ModelId,
    /// Project root directory
    pub project_dir: std::path::PathBuf,
}

/// Mutable runtime state -- changes frequently during runtime
/// Arc<RwLock<AppRuntime>> read-heavy/write-light, RwLock read locks are non-exclusive
pub struct AppRuntime {
    /// Cost tracker (written after each API call)
    pub cost_tracker: crab_core::model::CostTracker,
    /// Active session list (multiple in daemon mode)
    pub active_sessions: Vec<String>,
    /// MCP connection pool (dynamic connect/disconnect)
    pub mcp_connections: std::collections::HashMap<String, crab_mcp::McpClient>,
}

// Usage:
// let config = Arc::new(AppConfig { ... });     // Built at startup, read-only afterward
// let runtime = Arc::new(RwLock::new(AppRuntime { ... })); // Read/write at runtime
//
// // Hot path: zero-lock config reads
// let model = &config.model_id;
//
// // Write path: update cost (brief write lock)
// runtime.write().await.cost_tracker.record(&usage, cost);
```

---

### 6.21 `crates/engine/` -- Raw Query Loop

**Responsibility**: the pure "conversation + backend + tool executor → streaming events" loop. Corresponds to CC `src/query.ts` + `src/query/{stopHooks,tokenBudget,transitions,config,deps}.ts`. Contains no session persistence, no REPL state, no swarm, no system-prompt assembly.

**Directory Structure**

```
src/
├── lib.rs
├── loop.rs                  // run_query() core loop
├── streaming.rs             // SSE parsing
├── tool_orchestration.rs    // Tool dispatch (partition + execute)
├── stop_hooks.rs            // StopReason + stop conditions
├── token_budget.rs          // Token budget tracking
└── effort.rs                // Reasoning effort levels
```

**Public API**

```rust
pub struct QueryConfig {
    pub model: ModelId,
    pub max_tokens: u32,
    pub fallback_model: Option<ModelId>,   // streaming fallback on overload
    pub plan_model: Option<ModelId>,       // stronger model for plan mode
    // ...
}

pub async fn run_query(
    conversation: &mut Conversation,
    backend: &LlmBackend,
    executor: &ToolExecutor,
    config: &QueryConfig,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
) -> Result<QueryOutcome, EngineError>;

pub enum StopReason { NoToolCalls, ExplicitStop, MaxTurns(u32), TokenBudgetExceeded, UserCancel, Error(String) }
```

**Recovery paths** (continue sites within the query loop):
- **PTL recovery**: API returns prompt-too-long → drop oldest message group → retry (max 3 attempts, circuit breaker)
- **Max-output-tokens retry**: model output truncated → increase `max_tokens` → retry (max 3 attempts)
- **Streaming fallback**: SSE error with `fallback_model` configured → switch model and retry
- **Stop hook retry**: model produces no tool calls → `HookTrigger::Stop` fires → hook returns `Retry` → loop continues
- **Plan model routing**: when `plan_mode` is active and `plan_model` is set, the stronger model is used automatically

**Tool orchestration**:
- `partition_tool_calls()` splits tools into concurrent-safe and sequential groups using `Tool::is_concurrency_safe(input)` (input-dependent, not just static read/write)
- Concurrent-safe tools run in parallel via `futures::future::join_all()`
- Sequential tools run one-by-one with pre/post hook support
- Bash errors cancel remaining sibling writes in the batch (child `CancellationToken`)
- Cancellation hierarchy: query token → batch child token → individual tool

**Internal dependencies**: `core, api, session, tools, plugin`.

**Consumers**: `daemon` (headless), `agent` (wraps with orchestration), `remote::server` (drives a session's loop from a remote client).

---

### 6.22 `crates/remote/` -- crab-proto: Remote-Control Protocol (server + client)

**Responsibility**: Layer 3 crate that owns the `crab-proto` open protocol and both of its endpoints — a WebSocket **server** that attaches running sessions to remote clients, and an outbound **client** that connects to another crab-proto server. This is the **hinge for every non-CLI entry point**: web UI, mobile app, desktop app all attach via the server side; the client side powers crab-to-crab dispatch (supervisor crab driving worker crab) and bot integrations.

**Why a single crate (not three: proto / client / server)**: the same pattern `crab-mcp` already uses — one crate per protocol, with client + server + wire types grouped. Avoids the "server depends on client" awkwardness (proto types aren't client-owned), and removes the third-crate overhead that would only pay off if a Rust third-party consumer wanted proto-only access (current web/app/desktop clients will generate stubs from JSON Schema instead).

**Direction contrast**: both roles live here, unified by the protocol. `remote::server` is inbound (remote clients drive crab). `remote::client` is outbound (crab connects to another crab-proto endpoint, or any server speaking the same protocol). Contrast with `crates/ide` (outbound MCP client to IDE plugins) and `crates/acp` (inbound ACP server for editors) — those speak different protocols.

**Why not claude.ai**: binding a single vendor's private endpoints contradicts our multi-entry-point goal. The protocol is our own; claude.ai compatibility, if ever needed, would live as an optional adapter under the client side.

**Directory Structure**

```
src/
├── lib.rs
│
├── protocol/                    // wire types, JSON-RPC envelopes
│   ├── mod.rs
│   ├── envelope.rs              // message envelope framing
│   ├── error.rs                 // protocol error types
│   ├── handshake.rs             // connection handshake
│   ├── meta.rs                  // metadata types
│   ├── method.rs                // RPC method definitions
│   └── session.rs               // session protocol messages
│
├── auth/                        // shared auth (JWT)
│   ├── mod.rs
│   └── jwt.rs                   // jsonwebtoken sign/verify
│
├── client/                      // outbound crab-proto client
│   ├── mod.rs                   // RemoteClient::connect(url, auth)
│   ├── config.rs                // endpoint / auth_mode / timeout
│   └── error.rs                 // ClientError
│
└── server/                      // inbound crab-proto server
    ├── mod.rs                   // RemoteServer + SessionHandler trait
    ├── config.rs                // RemoteConfig
    ├── dispatch.rs              // message dispatch
    ├── handler.rs               // request handlers
    └── listener.rs              // WebSocket listener
```

**Internal dependencies**: `core, config, auth`.

---

### 6.23 `crates/sandbox/` -- Process Sandbox

**Responsibility**: Layer 2 leaf service. `Sandbox` trait + platform backends (seatbelt / landlock / windows / noop), consumed by `crates/tools` for Bash/PowerShell execution. Corresponds to CC `src/utils/sandbox/sandbox-adapter.ts` (985 LOC).

**Directory Structure**

```
src/
├── lib.rs
├── traits.rs                // Sandbox trait
├── config.rs                // SandboxConfig: workdir / env / timeout
├── policy.rs                // SandboxPolicy: read/write/exec/net allowlist
├── error.rs                 // SandboxError
├── doctor.rs                // diagnose platform support
├── violation.rs             // violation reporting
│
└── backend/
    ├── mod.rs               // auto-select: seatbelt > landlock > windows > noop
    ├── factory.rs           // backend auto-selection factory
    ├── noop.rs              // dev / fallback: allow-all
    ├── seatbelt.rs          // macOS: generate .sb profile + sandbox-exec
    ├── landlock.rs          // Linux 5.13+: landlock crate (cfg-gated)
    └── windows.rs           // Windows sandbox
```

**Core trait**:

```rust
pub trait Sandbox: Send + Sync {
    fn spawn(&self, cmd: &mut std::process::Command, policy: &SandboxPolicy)
        -> Result<std::process::Child, SandboxError>;
    fn name(&self) -> &'static str;
    fn is_supported() -> bool where Self: Sized;
}
```

**Platform selection**: no feature flags — backends are selected via `cfg(target_os)`. `landlock` dep is `[target.'cfg(target_os = "linux")'.dependencies]` so it only compiles on Linux. On platforms without a native sandbox we fall back to `noop`.

**UI**: sandbox violations surface in TUI via `core::Event` broadcasts, not this crate.

---

### 6.24 `crates/ide/` -- IDE MCP Client

**Responsibility**: Layer 2 leaf service. Client that connects to an IDE plugin's MCP server (hosted by VS Code / JetBrains extensions) and receives ambient context (selection, opened file, `@`-mentions). Publishes `IdeSelection` / `IdeAtMention` / `IdeConnection` to shared state consumed by `tui` (for display) and `agent` (for system-prompt injection).

**Direction contrast**: `ide` is an OUTBOUND client over MCP (crab → IDE MCP server). `remote::server` is an INBOUND server over crab-proto (web / app / desktop → crab). `acp` is an INBOUND server over ACP (editor → crab). Three different inbound/outbound × protocol combinations — all needed, all clean.

**Directory Structure**

```
src/
├── lib.rs
├── client.rs                // IdeClient + connection lifecycle
├── detection.rs             // detect running IDE MCP server
├── lockfile.rs              // IDE lockfile discovery for endpoint
├── notifications.rs         // inbound MCP notification handlers
├── injection.rs             // build system-reminder for agent
├── state.rs                 // Arc<RwLock<...>> shared handles
└── quirks/                  // IDE-specific quirks
    ├── mod.rs
    ├── vscode.rs            // VS Code quirks
    ├── jetbrains.rs         // JetBrains quirks
    └── wsl.rs               // WSL quirks
```

**Shared types** (`core::ide`): `IdeSelection`, `IdeAtMention`, `IdeConnection`. These live in `core` so `tui` can read without depending on `ide`.

**Internal dependencies**: `core, mcp`.

---

### 6.25 `crates/acp/` -- Agent Client Protocol server

**Responsibility**: Layer 2 leaf service that implements the server side of the [Agent Client Protocol](https://agentclientprotocol.com), the open JSON-RPC standard introduced by Zed in 2025 that lets editors drive external AI coding agents the way LSP lets them drive language servers. This crate lets crab **be** such an external agent: users in Zed / Neovim / Helix pick crab from their editor's "external agents" menu, the editor spawns crab as a child process, and messages flow over stdio framed as ACP JSON-RPC.

**Architectural role**:

```
Editor (ACP client)  ◄── ACP over stdio ──►  crab-acp (this crate)
                                                    │
                                                    ▼
                                              AgentHandler trait
                                                    │
                                                    ▼
                                         crab-engine / crab-agent
```

`AgentHandler` is the crate's external boundary — consumers (cli / daemon) plug in a real implementation wired to `crab-engine`. Mirrors how `crab-mcp::McpServer` takes a `ToolHandler` trait without embedding any specific tool backend.

**Symmetry with `crab-mcp`**: both crates are Layer 2 protocol implementations, both follow the "trait-in-this-crate, impl-elsewhere" pattern, both derive `schemars::JsonSchema` on wire types. The split is by **protocol**, not by direction — `crab-mcp` handles client+server of MCP, `crab-acp` handles server of ACP.

**Why not inside `crates/ide/`**: `ide` is crab-as-MCP-client (outbound); `acp` is crab-as-ACP-server (inbound). Opposite directions, different protocol stacks. Keeping them separate matches the "one crate = one protocol" rule.

**Directory Structure**

```
src/
├── lib.rs
└── server.rs                    // AcpServer + AgentHandler trait
```

**Internal dependencies**: `core`.

**External dependencies**: `serde`, `schemars`, `tokio`, `thiserror`, `tracing`.

---

### 6.26 `crates/job/` -- Unified Scheduling

**Responsibility**: Layer 2 crate that replaces the hand-rolled `tokio::time::interval` and `sleep_until` calls scattered across `crab-mcp` (heartbeat), `crab-agent` (proactive timers), `crab-remote` (server-scheduled triggers), and provides user-facing cron jobs. One API, one view — TUI can render "pending jobs", web UI can show a jobs panel, CLI can offer `crab jobs list / cancel`.

**Why needed under the multi-entry-point architecture**: every entry point (cli / ide / web / app / desktop) needs to **observe** scheduled work. Centralising through a shared crate means the scheduler state is queryable from any composition root (daemon for headless hosts, cli for interactive).

**Three job kinds**:

| Kind | Trigger | Persistence | Example |
|---|---|---|---|
| `OneShot` | one time at instant / after delay | in-memory | `ScheduleWakeup` |
| `Interval` | every N seconds from a reference point | in-memory | MCP server heartbeat |
| `Cron` | cron-expression schedule | JSON file under `~/.crab/jobs/` | "every day at 09:00, pull the latest report" |

**Directory Structure**

```
src/
├── lib.rs
├── id.rs                        // JobId + JobKind
├── spec.rs                      // JobSpec enum (one-shot | interval | cron)
├── scheduler.rs                 // JobScheduler
└── handler.rs                   // JobHandler trait
```

**Naming**: singular `job` (not `jobs`) per workspace convention — system-concept crates are singular (`skill`, `session`, `memory`, `engine`); only `tools` is plural because it's a collection of implementations. CLI commands stay plural (`crab jobs list`) per Unix convention; crate name and CLI surface don't need to match.

**Internal dependencies**: `core`.

**External dependencies**: `croner` (cron expression parsing, already in workspace), `tokio` (timers), `serde`, `thiserror`, `tracing`.

---

## 6.X Multi-Agent Three-Layer Architecture

crab models multi-agent collaboration in **three conceptually distinct layers**,
aligned with CCB's design but structured in Rust-idiomatic form.

| Layer | Purpose | CCB equivalent | crab location |
|-------|---------|----------------|---------------|
| **L1 — Teams (infrastructure)** | Mailbox, shared task list with `claimTask()`, spawner backends, worker pool, roster (team/member) | `isAgentSwarmsEnabled()` gate + `TeamCreate/Delete/SendMessage` tools + `teammateMailbox` | `crates/agent/src/teams/` ** |
| **L2a — Swarm (flat topology)** | Peer-to-peer, competitive task claiming; default usage when Teams is on and Coordinator Mode is off | "opened Teams but didn't enable Coordinator Mode" | `TeamMode::PeerToPeer` enum variant — no separate module |
| **L2b — Coordinator Mode (star overlay)** | Coordinator agent stripped of hands-on tools, workers run with allow-list, anti-pattern prompt ("understand before delegating") | `feature('COORDINATOR_MODE') && CLAUDE_CODE_COORDINATOR_MODE=1` | `crates/agent/src/coordinator/` ** |
| **L3 — Session runtime** | `AgentSession` ties conversation + backend + executor + topology choice | — | `crates/agent/src/session/` ** |

### Gating

- **L1 (Teams infrastructure)** is **unconditional** — it's crab's base multi-agent plumbing and ships enabled by default. No env/settings flag.
- **L2a (Swarm)** is the natural usage pattern whenever multiple agents exist and no overlay is active. Not a feature — just a topology choice via `TeamMode::PeerToPeer`.
- **L2b (Coordinator Mode)** is gated on `CRAB_COORDINATOR_MODE=1` env only (no CLI flag — keeps the surface hidden from `--help`). Helper: `crates/cli/src/main.rs::coordinator_mode_enabled()`.

### Divergence from CCB

| CCB choice | crab choice | Reason |
|------------|-------------|--------|
| `feature('COORDINATOR_MODE')` GrowthBook feature flag | `settings.experimental.coordinator_mode_enabled`  + env var | No remote telemetry; no GrowthBook in crab |
| `<task-notification>` XML protocol | Reuse `crab_core::Event` + `serde_json` | Rust serde is idiomatic; XML is a JS/TS artefact |
| Node `proper-lockfile` for `claimTask()` | `fd-lock` crate  | Rust-native, cross-platform |
| 7 concrete `*Task` classes | `trait TaskExecutor` + 4 concrete impls  | Rust trait polymorphism vs JS duck typing |
| CCB gates Agent Teams behind env + CLI flag | L1 Teams ships unconditional; only Coordinator Mode is gated | Teams is base plumbing for crab's multi-agent story — not an experiment to toggle |

### Current state

- `SessionConfig.coordinator_mode: bool` is propagated from env (`CRAB_COORDINATOR_MODE=1`).
- `crates/agent/src/teams/worker_pool.rs::WorkerPool` is the Layer 1 worker pool.
- `crates/agent/src/coordinator/` holds the Layer 2b overlay: `Coordinator::from_flag(true).apply(&mut registry, &mut prompt)` retains the registry to `{Agent, SendMessage, TaskStop}` and appends the anti-pattern prompt overlay.
- `session/runtime.rs::AgentSession::new` invokes the coordinator if `coordinator_mode` is set; otherwise no-op.
- `crates/agent/src/coordinator/tool_acl.rs` hosts the `COORDINATOR_TOOLS` / `WORKER_DENIED_TOOLS` constants; `ToolRegistry::retain_names` / `remove_names` in `crates/tools/src/registry.rs` implement the filter.
- Workers spawned via `Agent` from a Coordinator session now get a fresh registry built by `Coordinator::build_worker_registry` (default registry minus `WORKER_DENIED_TOOLS`) and an overlay-free prompt snapshotted into `CoordinatorContext::worker_base_prompt`. Non-coordinator sessions inherit as before.
- File-locked `TaskList` (`crates/swarm/src/task_lock.rs`) provides `with_locked` and `claim_task` over `fd-lock`, serialising cross-process task claims through an OS exclusive lock on `<path>.lock`. Used when teammates live in separate processes (tmux panes, remote agents); single-process use keeps the existing `Arc<Mutex<TaskList>>`.

---

## 7. Design Principles

| # | Principle | Description | Rationale |
|---|-----------|-------------|-----------|
| 1 | **core has zero I/O** | Pure data structures and traits, no file/network/process operations | Reusable by CLI/GUI/WASM frontends; unit tests need no mocking |
| 2 | **tools as independent crate** | 21+ tools have significant compile cost; keeping them separate means incremental compilation only triggers on changed tools | Changing one tool doesn't recompile everything |
| 3 | **fs and process are separate** | Orthogonal responsibilities: fs handles file content, process handles execution | GlobTool doesn't need sysinfo, BashTool doesn't need globset |
| 4 | **tui is optional** | cli bin uses feature flags to decide whether to compile tui | Future Tauri GUI imports core+session+tools but not tui |
| 5 | **api and session are layered** | api only handles HTTP communication, session manages business state | Replacing an API provider doesn't affect session logic |
| 6 | **Feature flags control optional dependencies** | No Bedrock? Don't compile AWS SDK. No WASM? Don't compile wasmtime. | Reduces compile time and binary size |
| 7 | **workspace.dependencies unifies versions** | All crates share the same version of third-party libraries | Avoids dependency conflicts and duplicate compilation |
| 8 | **Binary crates only do assembly** | cli/daemon only do assembly; all logic lives in library crates | Makes it easy to add new entry points in the future (desktop/wasm/mobile) |
| 9 | **CCB parity audit per crate** | Every existing crate gets a per-crate gap report vs CCB initially; reports live in `docs/superpowers/audits/` | Prevents silent drift from CCB behavior and makes Rust-idiom diverge decisions explicit. See `docs/superpowers/specs/2026-04-17-crate-restructure-design.md` §11 |
| 10 | **CCB references stay in docs, not code** | Audit reports, specs, and architecture docs may cite CCB paths. Code comments, identifier names, and test names must not. | Maintains clean separation between research material and shipping code |

---

## 8. Feature Flag Strategy

### 8.1 Per-Crate Feature Configuration

```toml
# --- crates/api/Cargo.toml ---
[features]
default = []
bedrock = ["crab-auth/bedrock"]                       # AWS Bedrock provider
vertex  = ["crab-auth/vertex"]                        # Google Vertex provider
proxy   = ["reqwest/socks"]                           # SOCKS5 proxy support

# --- crates/auth/Cargo.toml ---
[features]
default = []
bedrock = []                                          # AWS Bedrock credential support
vertex  = []                                          # GCP Vertex credential support

# --- crates/mcp/Cargo.toml ---
# Note: ws transport is NOT feature-gated as of 2026-04 — MCP server and WS
# transport ship default-on per the "no gates on protocol server side" rule.
[features]
default = []

# --- crates/plugin/Cargo.toml ---
[features]
default = []
wasm = ["wasmtime"]                                   # WASM plugin sandbox

# --- crates/process/Cargo.toml ---
[features]
default = []
pty = ["portable-pty"]                                # Pseudo-terminal allocation

# --- crates/sandbox/Cargo.toml (revised 2026-04: feature gates dropped) ---
# Backend selection is cfg(target_os=...) — no per-backend feature flags.
# `landlock` crate dep only compiles on Linux via target-cfg'd entry in
# [target.'cfg(target_os = "linux")'.dependencies].
[features]
default = []

# --- crates/remote/Cargo.toml ---
# Server WS listener + WS client both ship default-on (no feature gates).
[features]
default = []

# --- crates/acp/Cargo.toml ---
[features]
default = []

# --- crates/job/Cargo.toml ---
[features]
default = []

# --- crates/agent/Cargo.toml ---
[features]
default    = []
mem-ranker = ["crab-memory/mem-ranker"]               # LLM-based memory ranking
auto-dream = []                                       # Background memory consolidation
proactive  = []                                       # Proactive suggestions (stub)

# --- crates/tools/Cargo.toml ---
[features]
default = ["pdf"]
pdf     = ["pdf-extract"]                             # PDF reading support
pty     = ["portable-pty", "strip-ansi-escapes"]      # PTY-based bash

# --- crates/telemetry/Cargo.toml ---
[features]
default = ["fmt"]
fmt = ["tracing-subscriber/fmt"]                             # Local logging (default)
otlp = [                                                     # OTLP export
    "opentelemetry", "opentelemetry-otlp",
    "opentelemetry_sdk", "tracing-opentelemetry",
]

# --- crates/cli/Cargo.toml ---
[features]
default = ["tui"]
tui = ["crab-tui"]                                    # Terminal UI (enabled by default)
full = [                                              # Full-feature build
    "tui",
    "crab-plugin/wasm",
    "crab-api/bedrock",
    "crab-api/vertex",
    "crab-process/pty",
    "crab-telemetry/otlp",
    "crab-agent/mem-ranker",
]
```

### 8.2 Build Combinations

| Scenario | Command | What Gets Compiled |
|----------|---------|-------------------|
| Daily development | `cargo build` | cli + tui (default) |
| Minimal build | `cargo build --no-default-features` | cli only, no tui |
| Full feature | `cargo build -F full` | All providers + WASM + PTY |
| Library only | `cargo build -p crab-core` | Single crate compilation |
| WASM target | `cargo build -p crab-core --target wasm32-unknown-unknown` | core layer WASM |

### 8.3 Mapping to CC Feature Flags

CC source code manages about 31 runtime flags through `featureFlags.ts`; Crab Code splits them into:

- **Compile-time features**: Provider selection, WASM plugins, PTY, etc. (Cargo features)
- **Runtime flags**: Managed via `config.toml` settings and environment variables at startup

---

## 9. Workspace Configuration

### 9.1 Root Cargo.toml

```toml
[workspace]
resolver = "2"
members = ["crates/*", "xtask"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.95"
license = "Apache-2.0"
repository = "https://github.com/lingcoder/crab-code"
description = "Rust-native agentic coding CLI — open-source alternative to Claude Code"

[workspace.dependencies]
# See root Cargo.toml for complete dependency list
# Main categories: async runtime (tokio), serialization (serde), CLI (clap), HTTP (reqwest),
# TUI (ratatui), error handling (thiserror/anyhow), file system (globset/ignore), etc.

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = "warn"
pedantic = "warn"
nursery = "warn"

[profile.dev]
opt-level = 0
debug = true

[profile.release]
lto = "thin"
strip = true
codegen-units = 1
opt-level = 3
```

### 9.2 rust-toolchain.toml

```toml
[toolchain]
channel = "1.95"
components = ["rustfmt", "clippy", "rust-analyzer"]
```

### 9.3 rustfmt.toml

```toml
edition = "2024"
max_width = 100
tab_spaces = 4
use_field_init_shorthand = true
reorder_imports = true
```

---

## 10. Data Flow Design

### 10.1 Primary Data Flow: Query Loop

```
User input
  |
  v
┌──────────┐   prompt    ┌──────────┐        ┌──────────┐   HTTP POST   ┌───────────┐
│   cli    │────────────>│  agent   │ wraps  │  engine  │──────────────>│ LLM API   │
│  (tui)   │             │ (orches- │───────>│  run_    │  /v1/messages │ (Anthropic│
│          │             │ trator)  │        │  query() │<──────────────│  /OpenAI) │
└──────────┘             └────┬─────┘        └────┬─────┘   SSE stream  └───────────┘
      ^                       │                   │
      |                       │ system_prompt +   │ parse stream
      |                       │ hook_executor     │
      | core::Event::*        │                   v
      |                       │            ┌──────────┐
      |                       │            │ has tool │── No ─> StopReason → Outcome
      |                       │            │ calls?   │
      |                       │            └────┬─────┘
      |                       │                 │ Yes
      |                       │                 v
      |                       │            ┌──────────┐  delegate   ┌────────────┐
      |                       │            │  tools   │────────────>│ fs / mcp / │
      |                       │            │ executor │             │ proc / sb  │
      |                       │            └────┬─────┘<────────────└────────────┘
      |                       │                 │       ToolOutput
      └───────────────────────┴─────────────────┘
               events fan out via core::Event broadcast channel
```

Notes:
- `engine::run_query` is the pure loop (no session state, no REPL). It emits `Event::ContentDelta`, `Event::ToolResult`, `Event::TurnStart`, etc.
- The loop has built-in recovery: PTL retry (drop oldest messages), max-output-tokens retry (increase limit), streaming fallback (switch model), and stop hook retry (continue on hook request).
- `agent` wraps `engine` and adds: system-prompt assembly, git/PR context, error recovery, retry, proactive suggestions, auto-dream.
- `daemon` calls `engine::run_query` directly, skipping `agent`'s REPL-oriented layer.

### 10.2 MCP Tool Call

```
┌──────────┐  call_tool   ┌──────────┐  Crab facade   ┌──────────────┐
│  tools   │─────────────>│   mcp    │───────────────>│  MCP Server  │
│ executor │              │  client  │               │  (external    │
│          │              │          │               │   process)    │
└──────────┘              └────┬─────┘               └──────┬───────┘
                               │                             │
                               │     rmcp transport/client   │
                          ┌────v─────────────────────────┐   │
                          │  stdio child process / HTTP  │   │
                          │  handshake / tools/list      │   │
                          │  tools/call / resources      │   │
                          └──────────────────────────────┘   │
                                                             │
                               <─────────────────────────────┘
                                     tool / resource result
```

### 10.3 Context Compaction Decision Flow

```
┌──────────────┐
│ query_loop   │
│ start of     │
│ each turn    │
└──────┬───────┘
       │
       v
┌──────────────┐     estimated_tokens()
│ Estimate     │──────────────────────────┐
│ current      │                          │
│ token count  │                          v
└──────────────┘                   ┌──────────────┐
                                   │ > 70% of     │
                                   │ window?      │
                                   └──────┬───────┘
                                          │
                               ┌─── No ───┼─── Yes ──┐
                               │          │           │
                               v          │           v
                          Continue         │    ┌──────────────┐
                          normally         │    │ Select       │
                                          │    │ compaction   │
                                          │    │ strategy     │
                                          │    └──────┬───────┘
                                          │           │
                                          │    ┌──────v───────┐
                                          │    │  Snip        │ <- 70-80%
                                          │    │  Microcompact│ <- 80-85%
                                          │    │  Summarize   │ <- 85-90%
                                          │    │  Hybrid      │ <- 90-95%
                                          │    │  Truncate    │ <- > 95%
                                          │    └──────┬───────┘
                                          │           │
                                          │           v
                                          │    ┌──────────────┐
                                          │    │ Call small   │
                                          │    │ model to     │
                                          │    │ generate     │
                                          │    │ summary      │
                                          │    └──────┬───────┘
                                          │           │
                                          │           v
                                          │    ┌──────────────┐
                                          │    │ Rebuild      │
                                          │    │ message list │
                                          │    │ [summary] +  │
                                          │    │ recent N     │
                                          │    │ turns        │
                                          │    └──────┬───────┘
                                          │           │
                                          └───────────┘
                                                      │
                                                      v
                                                Continue query_loop
```

### 10.4 Remote-Session Attach Flow (crab-proto)

```
┌────────────────┐   WebSocket   ┌───────────────┐   attach     ┌──────────┐
│ Web / App /    │──────────────>│ remote::server│─────────────>│ session  │
│ Desktop / CLI  │<──────────────│   (crab-proto)│<─────────────│ (local)  │
└────────────────┘  inbound msg  └───────┬───────┘  outbound    └────┬─────┘
                                          │  Event relay             │
                                          │                          │
                                          │                    ┌─────▼─────┐
                                          │                    │   agent   │
                                          │                    │ (wrapper) │
                                          │                    └─────┬─────┘
                                          │                          │
                                          │                    ┌─────▼─────┐
                                          └───────────────────>│  engine   │
                                              drives loop      │ run_query │
                                                               └───────────┘
```

Auth: JWT (`remote/auth/jwt.rs`). Wire types derive `schemars::JsonSchema` so TS / Swift / Kotlin clients are stub-generated from the same Rust source.

### 10.5 Crab-to-Crab Trigger Flow (crab-proto, outbound side)

```
┌──────────┐   Tool call      ┌──────────────────┐   WS       ┌────────────────────┐
│   LLM    │─────────────────>│ tools/builtin/   │──────────> │ another crab's     │
│          │  remote_trigger  │ remote_trigger.rs│            │ remote::server     │
└──────────┘                  └────────┬─────────┘            └────────────────────┘
                                        │ uses
                                        v
                               ┌────────────────┐
                               │ remote::client │
                               └────────────────┘
```

A supervisor crab uses `remote::client` to dispatch work to a worker crab's `remote::server`. Target need not be another crab — any server speaking crab-proto works (webhook bot, user-built VPS front-end). No local session is touched on the sender; on the receiver the request lands via the same attach flow as §10.4. Scheduling for recurring triggers is delegated to `crates/job` (cron / interval / one-shot) rather than hand-rolled per-subsystem timers.

---

## 11. Extension System Design

### 11.1 Multi-Model Support Architecture (crab-api)

`crab-api`'s multi-model fallback and error classification layer, stacked on top of the `LlmBackend` enum:

```
User request
    |
    v
┌─────────────────┐
│    fallback.rs   │  -- Multi-model fallback chain (primary -> backup1 -> backup2)
└────────┬────────┘
         │
    ┌────v────────────────┐
    │  retry_strategy.rs  │  -- Enhanced retry (backoff + jitter)
    └────┬────────────────┘
         │
    ┌────v────────────────┐
    │ error_classifier.rs │  -- Error classification (retryable/non-retryable)
    └─────────────────────┘
```

**Module List**:
- `fallback.rs` -- Multi-model fallback chain (auto-switch to backup on primary failure)
- `capabilities.rs` -- Model capability negotiation and discovery
- `context_optimizer.rs` -- Context window optimization + smart truncation
- `streaming.rs` -- Streaming tool call parsing
- `retry_strategy.rs` / `error_classifier.rs` -- Enhanced retry and error classification


### 11.2 MCP Protocol Stack (crab-mcp)

MCP protocol extension modules:

- `crab-mcp` is Crab's MCP facade; it does not directly expose the underlying SDK to upper-layer crates
- Client-side stdio / HTTP connections are handled by the official SDK; Crab handles config discovery, naming, permission integration, and tool bridging
- The config primary path only retains `stdio` / `http` / `ws`
- Server / prompt / resource / tool registry still retain Crab's own abstraction layer

| Module | Function |
|--------|----------|
| `handshake.rs` + `negotiation.rs` | initialize/initialized handshake, capability set negotiation |
| `sampling.rs` | Server requests LLM inference (server -> client sampling) |
| `roots.rs` | Workspace root directory declaration (client tells server accessible paths) |
| `logging.rs` | Structured log message protocol |
| `sse_server.rs` | Crab as MCP server providing SSE transport |
| `capability.rs` | Capability declaration types |
| `notification.rs` | Server notification push |
| `progress.rs` | Progress reporting (long-running tool execution) |
| `cancellation.rs` | Request cancellation (`$/cancelRequest` JSON-RPC notification) |
| `health.rs` | Health check + heartbeat |


### 11.3 Agent Reliability (crab-agent)

**Reliability Subsystem** :
```
error_recovery::category + error_recovery::strategy    -- classify + recommend Retry/AskUser/Abort
teams::retry                                           -- exponential backoff
file_history                                           -- per-session Edit/Write snapshots, /rewind
summarizer + session::runtime::compact_conversation    -- /compact and auto-compact at 80% watermark
microcompact                                           -- truncate stale tool results before full compaction
llm_summarizer                                         -- LLM-driven summary (falls back to heuristic)
```

**Compaction pipeline** (multi-layer, triggered by context window pressure):
1. **Microcompaction** — truncate old Bash/Grep/Glob/Read/Web tool results, mark as `[Old tool result content cleared]`
2. **LLM summarization** — call small model to generate semantic summary preserving decisions and code changes
3. **Heuristic fallback** — pattern-based extraction (Decision/CodeChange/UnresolvedIssue) when LLM is unavailable
4. **Compact boundary** — session marks compaction point; API only sees post-boundary messages

**Engine-level recovery** (in `crates/engine`):
- PTL retry with message group eviction (max 3 attempts)
- Max-output-tokens retry with increasing limits
- Streaming fallback model switching
- Stop hook retry via `HookAction::Retry`

The in-memory `rollback.rs` UndoStack was replaced with the file-backed
`file_history/` module that mirrors CCB's `src/utils/fileHistory.ts`.


### 11.4 TUI Component Library (crab-tui, 21 Components)

**Interactive Components** (user-operated):
- `command_palette` -- Ctrl+P command palette, fuzzy search all commands
- `autocomplete` -- Popup completion suggestions while typing
- `search` -- Global search (filename + content)
- `history_search` -- Ctrl+R history search overlay

**Content Display Components**:
- `code_block` -- Code block + copy button (syntect highlighting)
- `tool_output` -- Collapsible tool output display (expandable/collapsible)

**Status Feedback Components**:
- `notification` -- Toast notification (top popup, 3s auto-dismiss)
- `progress_indicator` -- Percentage progress bar
- `loading` -- Multi-style loading animation (spin/dot/bar)
- `status_bar` -- Enhanced status bar (mode/provider/token count/response latency)
- `shortcut_hint` -- Always-visible shortcut hint bar at bottom


### 11.5 Auth Cloud Platform Authentication (crab-auth)

```
AWS Scenario:
  aws_iam.rs -> Supports IRSA (pod-level IAM roles) + standard IAM credential chain
  credential_chain.rs -> env -> keychain -> file -> IRSA -> instance metadata

GCP Scenario:
  gcp_identity.rs -> Workload Identity Federation
  vertex_auth.rs -> GCP Vertex AI dedicated authentication
```

### 11.6 Sandbox Backend Strategy

`crates/sandbox` provides a trait-only core with platform backends selected via `cfg(target_os)`. At runtime, `create_sandbox(None)` picks the best available backend using this precedence:

```
seatbelt (macOS)  >  landlock (Linux 5.13+)  >  wsl (Windows)  >  noop
```

Flow:

```
┌──────────────────────┐
│ create_sandbox(None) │
└──────────┬───────────┘
           │
           v
┌──────────────────────┐
│ for each backend in  │
│ precedence order:    │
│   if is_supported()  │──── yes ──> return Box<dyn Sandbox>
│     return it        │
└──────────┬───────────┘
           │ no backend supported
           v
┌──────────────────────┐
│ noop + warn!()       │
└──────────────────────┘
```

Doctor (`sandbox::doctor::diagnose()`) reports each backend's support status, used by `/doctor` and the TUI Sandbox settings tab. Consumers (e.g., `tools/builtin/bash.rs`) may override precedence by passing `Some("seatbelt")` etc. to force a specific backend for testing.

Violation events flow upward via `core::Event` broadcasts, consumed by the TUI for display and by the denial tracker (`core/permission/denial_tracker.rs`) for repeat-offense patterns.
