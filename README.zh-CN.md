<div align="center">

<img src="assets/logo-horizontal.svg" width="360" alt="Crab Code" />

**Claude Code 的开源替代品，完全用 Rust 从零构建。**

[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/lingcoder/crab-code/actions/workflows/ci.yml/badge.svg)](https://github.com/lingcoder/crab-code/actions/workflows/ci.yml)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#贡献)

[**English**](README.md) | **中文**

</div>

---

> **积极开发中** — 4500+ 测试 · 24 crate · ~140k LOC

Crab Code 是一个 Rust 原生的 Agentic Coding CLI。它对齐 Claude Code 的工具集、权限模型和交互方式，同时支持任意 LLM 提供商（Anthropic / OpenAI / DeepSeek / Ollama / Bedrock / Vertex 等）。

## 快速开始

```bash
git clone https://github.com/lingcoder/crab-code.git && cd crab-code
cargo build --release
export ANTHROPIC_API_KEY=sk-ant-...

./target/release/crab                   # 交互式 TUI
./target/release/crab "解释这段代码"      # 单次模式
./target/release/crab -p "修复 bug"      # 非交互
```

更多用法见 `crab --help`。配置文件：`~/.crab/settings.json`

## 对比

| | Crab Code | Claude Code | [OpenCode](https://github.com/anomalyco/opencode) | Codex CLI |
|--|-----------|-------------|----------|-----------|
| 开源 | Apache 2.0 | 闭源 | MIT | Apache 2.0 |
| 语言 | Rust | TypeScript | TypeScript | Rust |
| 模型 | 任意 | Anthropic | 任意 | 仅 OpenAI |
| MCP | 3 传输 | 6 传输 | LSP | 2 传输 |

## 架构

24 个 Rust crate，4 层依赖。详见 [`docs/architecture.md`](docs/architecture.md)。

```
入口    cli · daemon
编排    agent · engine · session · tui · remote
服务    api · tools · mcp · skill · plugin · telemetry · ide · sandbox · job · acp
基础    core · common · config · auth · fs · memory · process
```

## 构建与测试

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## 贡献

欢迎 PR。

## 许可证

[Apache License 2.0](LICENSE)

---

<div align="center">

**Built with Rust by [lingcoder](https://github.com/lingcoder)**

</div>
