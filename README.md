<div align="center">

<img src="assets/logo-horizontal.svg" width="360" alt="Crab Code" />

**Open-source alternative to Claude Code, built from scratch in Rust.**

[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/lingcoder/crab-code/actions/workflows/ci.yml/badge.svg)](https://github.com/lingcoder/crab-code/actions/workflows/ci.yml)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#contributing)

**English** | [**中文**](README.zh-CN.md)

</div>

---

> **Active Development** — 4600+ tests · 26 crates · ~146k LOC

Crab Code is a Rust-native agentic coding CLI. It aligns with Claude Code's toolset, permission model, and interaction patterns while supporting any LLM provider (Anthropic / OpenAI / DeepSeek / Bedrock / Vertex).

## Quick Start

```bash
git clone https://github.com/lingcoder/crab-code.git && cd crab-code
cargo build --release
export ANTHROPIC_API_KEY=sk-ant-...

./target/release/crab                   # Interactive TUI
./target/release/crab "explain this codebase"   # Single-shot
./target/release/crab -p "fix the bug"  # Non-interactive
```

See `crab --help` for more. Config lives at `~/.crab/config.toml` (snake_case TOML); the full loading & merge spec is in [`docs/config.md`](docs/config.md).

## Configuration

Config sources, low → high priority:

```
defaults  <  plugin  <  user  <  project  <  local  <  --config <file>     (file layer)
                                                            <
                                                       env  <  CLI flag    (runtime layer)
```

- **User**: `~/.crab/config.toml` (or `$CRAB_CONFIG_DIR/config.toml`)
- **Project**: `$PWD/.crab/config.toml` (committed)
- **Local**: `$PWD/.crab/config.local.toml` (gitignored)
- **`--config <path>`**: CLI-injected file
- **`-c key.path=value`**: dotted runtime override (TOML grammar; repeatable)

Example `config.toml`:

```toml
api_provider = "deepseek"
base_url = "https://api.deepseek.com"
model = "deepseek-chat"
api_key = "sk-..."           # optional; env wins if both set

[permissions]
allow = ["Bash(git:*)", "Read", "Edit"]
deny  = ["Bash(rm:*)"]       # deny always wins over allow
```

## Environment Variables

Env (runtime layer) always wins over file. Mutually-exclusive variants apply highest-first.

| Category | Variable | Purpose |
|----------|----------|---------|
| Provider | `CRAB_API_PROVIDER` | Override provider: `anthropic`, `openai`, `deepseek`, `bedrock`, `vertex`, `custom` |
| Provider | `CRAB_API_KEY` | Universal API key (any provider; highest priority) |
| Provider | `CRAB_MODEL` | Override model name |
| Provider | `CRAB_BASE_URL` | Universal base URL override |
| Provider | `CRAB_CONFIG_DIR` | Relocate config root (default `~/.crab/`) |
| Provider | `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` | Anthropic provider only |
| Provider | `ANTHROPIC_BASE_URL` | Anthropic base URL (only when `CRAB_API_PROVIDER=anthropic`) |
| Provider | `OPENAI_API_KEY` | OpenAI provider |
| Provider | `OPENAI_BASE_URL` | OpenAI base URL (only when `CRAB_API_PROVIDER=openai`) |
| Provider | `DEEPSEEK_API_KEY` | DeepSeek provider |
| Provider | `DEEPSEEK_BASE_URL` | DeepSeek base URL (only when `CRAB_API_PROVIDER=deepseek`) |
| Bedrock | `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` | Static credentials |
| Bedrock | `AWS_SESSION_TOKEN` | Optional session token (temporary credentials) |
| Bedrock | `AWS_REGION` / `AWS_DEFAULT_REGION` | AWS region |
| Bedrock | `AWS_ROLE_ARN` | IAM role ARN to assume |
| Bedrock | `AWS_WEB_IDENTITY_TOKEN_FILE` | OIDC token file (web-identity role assumption) |
| Bedrock | `AWS_EXTERNAL_ID` | External ID for cross-account role assumption |
| Bedrock | `AWS_ROLE_SESSION_NAME` | Session name for assumed role |
| Vertex | `GOOGLE_CLOUD_PROJECT` / `GCLOUD_PROJECT` | GCP project ID |
| Vertex | `GOOGLE_CLOUD_REGION` | GCP region (default: `us-central1`) |
| Vertex | `GOOGLE_APPLICATION_CREDENTIALS` | Path to service account key JSON |
| Shell | `CRAB_SHELL` | Path to bash/zsh binary (overrides auto-detection for the Bash tool) |
| Shell | `SHELL` | POSIX fallback when `CRAB_SHELL` is unset |
| Shell | `CRAB_USE_POWERSHELL_TOOL` | Truthy value exposes the `PowerShell` tool on Windows (default off) |
| Agent | `CRAB_COORDINATOR_MODE` | `1` enables Agent Teams coordinator mode |
| Agent | `CRAB_AUTO_DREAM` | `1` enables background memory consolidation between sessions |
| Agent | `CRAB_AUTO_DREAM_MIN_HOURS` | Minimum hours between consolidations (default: 6) |
| Agent | `CRAB_AUTO_DREAM_MIN_SESSIONS` | Minimum sessions before consolidation triggers (default: 2) |
| TLS | `CRAB_CA_BUNDLE` | Path to custom CA certificate bundle (PEM) |
| TLS | `SSL_CERT_FILE` / `SSL_CERT_DIR` | Standard OpenSSL CA overrides |

## Comparison

| | Crab Code | Claude Code | OpenCode| Codex CLI |
|--|-----------|-------------|---------|-----------|
| Open Source | Apache 2.0 | Proprietary | MIT | Apache 2.0 |
| Language | Rust | TypeScript | TypeScript | Rust |
| Models | Any provider | Anthropic | Any provider | OpenAI only |
| MCP | 3 transports | 6 transports | LSP | 2 transports |

## Architecture

26 Rust crates in 4 layers. See [`docs/architecture.md`](docs/architecture.md) for details.

```
Entry     cli · daemon · acp
Engine    agent · engine · session · tui · remote
Service   api · tools · commands · mcp · skill · plugin · telemetry · ide · sandbox · swarm · job · fs · memory · process
Foundation core · common · config · auth
```

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Contributing

PRs welcome.

## License

[Apache License 2.0](LICENSE)

