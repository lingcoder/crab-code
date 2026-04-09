<div align="center">

<img src="assets/logo-horizontal.svg" width="360" alt="Crab Code" />

**Open-source alternative to Claude Code, built from scratch in Rust.**

*Inspired by Claude Code's agentic workflow -- open source, Rust-native, works with any LLM.*

[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/lingcoder/crab-code/actions/workflows/ci.yml/badge.svg)](https://github.com/lingcoder/crab-code/actions/workflows/ci.yml)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#contributing)

**English** | [**中文**](README.zh-CN.md)

</div>

---

> **Status: Active Development** -- 49 built-in tools, 6 permission modes, extended thinking, multi-agent coordination, structured message TUI with 187 spinner verbs, 3800+ tests across 17 crates, 110k+ LOC. Zero `todo!()` macros.

## What is Crab Code?

[Claude Code](https://docs.anthropic.com/en/docs/claude-code) pioneered the agentic coding CLI -- an AI that doesn't just suggest code, but thinks, plans, and executes autonomously in your terminal.

**Crab Code** brings this experience to the open-source world, independently built from the ground up in Rust:

- **Fully open source** -- Apache 2.0, no feature-gating, no black box
- **Rust-native performance** -- instant startup, minimal memory, no Node.js overhead
- **Model agnostic** -- Claude, GPT, DeepSeek, Qwen, Ollama, or any OpenAI-compatible API
- **Secure** -- 6 permission modes (default, acceptEdits, dontAsk, bypassPermissions, plan, auto)
- **MCP compatible** -- stdio, SSE, and WebSocket transports
- **Claude Code aligned** -- CLI flags, slash commands, tools, and workflows match Claude Code behavior

## Quick Start

```bash
git clone https://github.com/lingcoder/crab-code.git
cd crab-code
cargo build --release

# Set your API key
export ANTHROPIC_API_KEY=sk-ant-...

# Interactive TUI mode
./target/release/crab

# Single-shot mode
./target/release/crab "explain this codebase"

# Print mode (non-interactive)
./target/release/crab -p "fix the bug in main.rs"

# With a specific provider
./target/release/crab --provider openai --model gpt-4o "refactor auth module"
```

### Claude Code Compatible Configuration

Crab Code supports Claude Code's `settings.json` format, including the `env` field:

```json
// ~/.crab/settings.json
{
  "env": {
    "ANTHROPIC_AUTH_TOKEN": "cr_...",
    "ANTHROPIC_BASE_URL": "http://your-proxy/api"
  },
  "model": "claude-opus-4-6"
}
```

## Features

### Core Agent Loop

- **Streaming SSE** -- real-time token-by-token output from LLM APIs
- **Tool execution cycle** -- model reasoning -> tool call -> permission check -> execute -> result -> next turn
- **Extended thinking** -- budget_tokens support for deep reasoning (Anthropic thinking blocks)
- **Retry + fallback** -- automatic retry on transient errors, model fallback on overload
- **Effort levels** -- low/medium/high/max mapped to API parameters

### Built-in Tools (49)

| Category | Tools |
|----------|-------|
| File I/O | Read, Write, Edit, Glob, Grep |
| Execution | Bash, PowerShell (Windows) |
| Agent | AgentTool (sub-agents), TeamCreate, TeamDelete, SendMessage |
| Tasks | TaskCreate, TaskGet, TaskList, TaskUpdate, TaskStop, TaskOutput |
| Scheduling | CronCreate, CronDelete, CronList |
| Planning | EnterPlanMode, ExitPlanMode |
| Notebook | NotebookEdit, NotebookRead |
| Web | WebFetch, WebSearch |
| LSP | LSP (go-to-definition, references, hover, symbols) |
| Worktree | EnterWorktree, ExitWorktree |
| Remote | RemoteTrigger |
| Other | AskUserQuestion, Skill |

### Permission System

6 modes aligned with Claude Code:

- **default** -- prompt for potentially dangerous operations
- **acceptEdits** -- auto-approve file edits, prompt for Bash
- **dontAsk** -- auto-approve everything (no prompts)
- **bypassPermissions** -- skip all checks
- **plan** -- read-only mode, requires plan approval before execution
- **auto** -- AI classifier auto-approves low-risk operations

Plus tool-level filtering with `--allowedTools` / `--disallowedTools` supporting glob patterns (`Bash(git:*)`, `Edit`).

### Slash Commands (20+)

`/help` `/clear` `/compact` `/cost` `/status` `/memory` `/init` `/model` `/config` `/permissions` `/resume` `/history` `/export` `/doctor` `/diff` `/review` `/plan` `/exit` `/fast` `/effort` `/add-dir` `/files` `/thinking` and more.

### LLM Providers

- **Anthropic** -- Messages API with SSE streaming (default: `claude-sonnet-4-6`)
- **OpenAI-compatible** -- Chat Completions API (GPT, DeepSeek, Qwen, Ollama, vLLM, etc.)
- **AWS Bedrock** -- SigV4 signing with inference profile discovery
- **GCP Vertex AI** -- ADC authentication

### MCP (Model Context Protocol)

- stdio, SSE, and WebSocket transports
- `McpToolAdapter` bridges MCP tools to native `Tool` trait
- Configure via `~/.crab/settings.json` or `--mcp-config`

### Session Management

- Auto-save conversation history
- `--continue` / `-c` resume last session
- `--resume <id>` resume specific session
- `--fork-session` fork on resume
- `--name` friendly session names
- Auto-compaction at 80% context window threshold

### Hook System

- `PreToolUse` / `PostToolUse` / `UserPromptSubmit` triggers
- Shell command execution with Allow / Deny / Modify responses
- Configure in `settings.json`

### Interactive TUI

- ratatui + crossterm terminal UI
- Markdown rendering with syntax highlighting
- Vim mode editing
- Autocomplete for slash commands and file paths
- Permission dialogs
- Cost tracking status bar

## Architecture

4-layer, 16-crate Rust workspace:

```
Layer 4 (Entry)     cli          daemon        xtask
                      |              |
Layer 3 (Orch)     agent         session
                      |              |
Layer 2 (Service)  api   tools   mcp   tui   plugin   telemetry
                      |     |      |     |      |         |
Layer 1 (Found)    common   core   config   auth
```

Key design decisions:
- **Async runtime**: tokio (multi-threaded)
- **LLM dispatch**: `enum LlmBackend` -- zero dynamic dispatch, exhaustive match
- **Tool system**: `trait Tool` with JSON Schema discovery, `ToolRegistry` + `ToolExecutor`
- **TUI**: ratatui + crossterm, immediate-mode rendering
- **Error handling**: `thiserror` for libraries, `anyhow` for application

> Full architecture details: [`docs/architecture.md`](docs/architecture.md)

## Configuration

```bash
# Global config
~/.crab/settings.json        # API keys, provider settings, MCP servers
~/.crab/memory/              # Persistent memory files
~/.crab/sessions/            # Saved conversation sessions
~/.crab/skills/              # Global skill definitions

# Project config
your-project/CRAB.md         # Project instructions (like CLAUDE.md)
your-project/.crab/settings.json  # Project-level overrides
your-project/.crab/skills/   # Project-specific skills
```

## CLI Usage

```bash
crab                              # Interactive TUI mode
crab "your prompt"                # Single-shot mode
crab -p "your prompt"             # Print mode (non-interactive)
crab -c                           # Continue last session
crab --provider openai            # Use OpenAI-compatible provider
crab --model opus                 # Model alias (sonnet/opus/haiku)
crab --permission-mode plan       # Plan mode
crab --effort high                # Set effort level
crab --resume <session-id>        # Resume a saved session
crab doctor                       # Run diagnostics
crab auth login                   # Configure authentication
```

## Build & Development

```bash
cargo build --workspace                    # Build all
cargo test --workspace                     # Run all tests (3800+)
cargo clippy --workspace -- -D warnings    # Lint
cargo fmt --all --check                    # Check formatting
cargo run --bin crab                       # Run CLI
```

## Comparison

| | Crab Code | Claude Code | [OpenCode](https://github.com/anomalyco/opencode) | Codex CLI |
|--|-----------|-------------|----------|-----------|
| Open Source | Apache 2.0 | Proprietary | MIT | Apache 2.0 |
| Language | Rust | TypeScript (Bun) | TypeScript | Rust |
| Model Agnostic | Any provider | Anthropic + AWS/GCP | Any provider | OpenAI only |
| Self-hosted | Yes | No | Yes | Yes |
| MCP Support | stdio + SSE + WS | 6 transports | LSP | 2 transports |
| TUI | ratatui | Ink (React) | Custom | ratatui |
| Built-in Tools | 49 | 52 | ~10 | ~10 |
| Permission Modes | 6 | 6 | 2 | 3 |

## Contributing

We'd love your help! See areas where we need contributions:

- Claude Code feature alignment
- OS-level sandboxing (Landlock / Seatbelt / Windows Job Object)
- End-to-end integration testing
- Additional LLM provider testing
- Documentation & i18n

## License

[Apache License 2.0](LICENSE)

---

<div align="center">

**Built with Rust by [lingcoder](https://github.com/lingcoder)**

*Claude Code showed us the future of agentic coding. Crab Code makes it open for everyone.*

</div>
