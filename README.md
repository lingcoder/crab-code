<div align="center">

# 🦀 Crab Code

**Open-source alternative to Claude Code, built from scratch in Rust.**

*Inspired by Claude Code's agentic workflow — open source, Rust-native, works with any LLM.*

[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#contributing)

[中文文档](README.zh-CN.md)

</div>

---

> **Status: Architecture & Planning Phase** — The project design is complete. Core development begins soon. Star & watch to follow the journey.

## What is Crab Code?

[Claude Code](https://docs.anthropic.com/en/docs/claude-code) pioneered the agentic coding CLI — an AI that doesn't just suggest code, but thinks, plans, and executes autonomously in your terminal.

**Crab Code** brings this agentic coding experience to the open-source world, independently built from the ground up in Rust:

- 🔓 **Fully open source** — no feature-gating, no black box
- ⚡ **Rust-native performance** — instant startup, minimal memory, no Node.js overhead
- 🌍 **Model agnostic** — Claude, GPT, DeepSeek, Qwen, Ollama, or any OpenAI-compatible API
- 🔒 **Truly secure** — OS-level sandbox (Landlock/Seatbelt) + application-level permission control
- 🔄 **Workflow compatible** — works with your existing `CLAUDE.md`, `settings.json`, and MCP configs

## Designed for Claude Code Users

If you're already using Claude Code, Crab Code is designed to feel familiar. We aim to be compatible with the workflow and config conventions that Claude Code users already know:

| Claude Code | Crab Code |
|-------------|-----------|
| `claude` | `crab` |
| `CLAUDE.md` | Reads `CLAUDE.md` (+ `CRAB.md` for extras) |
| `settings.json` | Compatible `settings.json` |
| `/slash` commands | Same slash commands |
| MCP servers | Same MCP config format |
| Permission modes | Same permission model |

## Features

> 🚧 **Under active development** — Star & watch to follow the journey.

### Phase 1: Core (Agentic Coding Parity)

- [ ] Agent loop — model reasoning + tool execution cycle
- [ ] Built-in tools — file read/write/edit, bash, glob, grep
- [ ] Permission system — allowlist, ask, deny modes
- [ ] `CLAUDE.md` support — project memory, multi-level config
- [ ] `settings.json` compatibility
- [ ] MCP client — stdio and SSE transports
- [ ] Conversation history and session resume
- [ ] Context window management and compression
- [ ] Git-aware operations
- [ ] Interactive terminal UI (ratatui)

### Phase 2: Beyond

- [ ] Multi-model support — switch providers per task
- [ ] OS-level sandboxing — Landlock (Linux) / Seatbelt (macOS)
- [ ] Plugin system via wire protocol (JSONL over stdin/stdout)
- [ ] Multi-agent coordination
- [ ] Self-hostable, air-gapped friendly

## Architecture

```
┌─────────────────────────────────────────┐
│           Terminal UI (ratatui)          │
├─────────────────────────────────────────┤
│            Agent Core Engine            │
│  ┌────────┐ ┌──────────┐ ┌───────────┐ │
│  │ Tools  │ │ LLM Loop │ │  Context  │ │
│  │ System │ │          │ │  Manager  │ │
│  └────────┘ └──────────┘ └───────────┘ │
├─────────────────────────────────────────┤
│  LLM Providers  │   Tool System (MCP)  │
│  ┌──────┐┌────┐ │ ┌──────┐┌─────────┐ │
│  │Claude││ .. │ │ │Built ││   MCP   │ │
│  │GPT   ││ .. │ │ │ -in  ││ Servers │ │
│  │Local ││    │ │ │      ││         │ │
│  └──────┘└────┘ │ └──────┘└─────────┘ │
├─────────────────────────────────────────┤
│   Sandbox (Landlock / Seatbelt / App)   │
├─────────────────────────────────────────┤
│         Permission & Config Layer       │
│   (settings.json / CLAUDE.md / CRAB.md) │
└─────────────────────────────────────────┘
```

## Quick Start

> Code is not yet available — implementation begins at M0 (workspace scaffold + CI). See [Roadmap](#roadmap) below.

```bash
# Coming soon
git clone https://github.com/crabforge/crab-code.git
cd crab-code && cargo build --release
crab
```

## Configuration

Crab Code is compatible with Claude Code's config conventions, with optional extensions:

```bash
# Your existing configs work as-is
~/.claude/settings.json          # Crab Code reads this
your-project/CLAUDE.md           # Crab Code reads this

# Crab Code extensions (optional)
~/.config/crab-code/config.toml  # Multi-provider config, extra settings
your-project/CRAB.md             # Additional project instructions
```

```toml
# ~/.config/crab-code/config.toml

[provider.default]
type = "anthropic"
model = "claude-sonnet-4-20250514"

[provider.local]
type = "ollama"
endpoint = "http://localhost:11434"
model = "deepseek-coder-v2"

# Use different providers for different tasks
[routing]
planning = "default"        # Use Claude for planning
execution = "local"         # Use local model for simple edits
```

## Comparison

| | Crab Code | Claude Code | Codex CLI |
|--|-----------|-------------|-----------|
| Open Source | ✅ Apache 2.0 | ❌ Proprietary | ✅ Apache 2.0 |
| Language | Rust | TypeScript (Node.js) | Rust |
| Model Agnostic | ✅ Any provider | Anthropic + OpenAI-compatible¹ | OpenAI only |
| Workflow Compatible | ✅ Reads CLAUDE.md | — | ❌ |
| Self-hosted | ✅ | ❌ | ✅ |
| Sandbox | OS-level + App-level | App-level (7 layers) | OS-level (kernel) |
| Plugin System | ✅ (wire protocol) | ✅ (Skills + Plugins) | ✅ (Plugins) |
| MCP Support | ✅ | ✅ (6 transports) | ✅ (2 transports) |
| Multi-Agent | ✅ | ✅ (Coordinator + Swarm) | ✅ (Coordinator + Worker) |
| TUI Framework | ratatui | Ink (React) | ratatui |

> ¹ Claude Code added experimental OpenAI-compatible endpoint support via `CLAUDE_CODE_USE_OPENAI=1` (Ollama, DeepSeek, vLLM, etc.).

## Roadmap

```
M0  Project scaffold + CI       ███░░░░░░░  Workspace, GitHub Actions, deny.toml
M1  Domain models (core+common) ░░░░░░░░░░  Message/Tool/Event types + tracing
M2  Streaming API (api+auth)    ░░░░░░░░░░  Anthropic + OpenAI-compatible providers
M3  Core tools (tools+fs+proc)  ░░░░░░░░░░  6 built-in tools + safety tests
M4  Agent loop (session+agent)  ░░░░░░░░░░  Query loop + readline REPL [Dogfooding]
M5  Terminal UI (tui)           ░░░░░░░░░░  ratatui interactive REPL
M6  Config + context mgmt       ░░░░░░░░░░  Permissions, CLAUDE.md, compression
M7  MCP + multi-agent           ░░░░░░░░░░  MCP client, AgentTool, skills
```

<!-- Detailed architecture and plan docs maintained in a separate private repo -->

## Contributing

We'd love your help! Crab Code is built independently from scratch, and there's a lot to build.

<!-- Contributing guidelines coming soon -->

```
Areas we need help with:
├── Agent loop & tool execution
├── Config compatibility layer
├── LLM provider integrations
├── Terminal UI (ratatui)
├── MCP client implementation
├── OS-level sandboxing
├── Testing & benchmarks
└── Documentation & i18n
```

## License

[Apache License 2.0](LICENSE)

---

<div align="center">

<br>

**Built with 🦀 by the [CrabForge](https://github.com/crabforge) community**

*Claude Code showed us the future of agentic coding. Crab Code makes it open for everyone.*

</div>
