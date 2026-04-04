<div align="center">

# 🦀 Crab Code

**Claude Code 的开源替代品，Rust 从零构建。**

*受 Claude Code 的 Agentic 工作流启发 — 开源、Rust 原生、支持任意 LLM。*

[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#参与贡献)

[English](README.md)

</div>

---

> **项目状态：架构与规划阶段** — 项目设计已完成，核心开发即将启动。Star 关注进展。

## Crab Code 是什么？

[Claude Code](https://docs.anthropic.com/en/docs/claude-code) 开创了 Agentic Coding CLI 这一品类 — 一个不只是建议代码，而是能自主思考、规划和执行的 AI 智能体，直接在你的终端里工作。

**Crab Code** 将这种 Agentic Coding 体验带入开源世界，使用 Rust 从零独立构建：

- 🔓 **完全开源** — 没有功能裁剪，没有黑盒
- ⚡ **Rust 原生性能** — 毫秒级启动，极低内存，无 Node.js 开销
- 🌍 **模型无关** — Claude、GPT、DeepSeek、Qwen、Ollama 或任何 OpenAI 兼容 API
- 🔒 **真正安全** — OS 级沙箱 (Landlock/Seatbelt) + 应用层权限控制
- 🔄 **工作流兼容** — 直接使用你现有的 `CLAUDE.md`、`settings.json` 和 MCP 配置

## 为 Claude Code 用户设计

如果你已经在使用 Claude Code，Crab Code 的目标是让你感到熟悉。我们兼容 Claude Code 用户已经熟知的工作流和配置规范：

| Claude Code | Crab Code |
|-------------|-----------|
| `claude` | `crab` |
| `CLAUDE.md` | 直接读取 `CLAUDE.md`（+ `CRAB.md` 扩展） |
| `settings.json` | 兼容 `settings.json` |
| `/slash` 命令 | 相同的斜杠命令 |
| MCP servers | 相同的 MCP 配置格式 |
| 权限模式 | 相同的权限模型 |

## 功能特性

> 🚧 **积极开发中** — Star 关注进展，欢迎参与共建。

### 第一阶段：核心功能（Agentic Coding 对齐）

- [ ] Agent 循环 — 模型推理 + 工具执行循环
- [ ] 内置工具 — 文件读写编辑、Bash、Glob、Grep
- [ ] 权限系统 — allowlist、ask、deny 模式
- [ ] `CLAUDE.md` 支持 — 项目记忆，多级配置
- [ ] `settings.json` 兼容
- [ ] MCP 客户端 — stdio 和 SSE 传输
- [ ] 对话历史与会话恢复
- [ ] 上下文窗口管理与压缩
- [ ] Git 感知操作
- [ ] 交互式终端 UI (ratatui)

### 第二阶段：超越

- [ ] 多模型支持 — 按任务切换 provider
- [ ] OS 级沙箱 — Landlock (Linux) / Seatbelt (macOS)
- [ ] 插件系统 via wire protocol (JSONL over stdin/stdout)
- [ ] 多 Agent 协作
- [ ] 支持自部署，适配内网离线环境

## 架构

```
┌─────────────────────────────────────────┐
│           终端 UI (ratatui)              │
├─────────────────────────────────────────┤
│            智能体核心引擎                 │
│  ┌────────┐ ┌──────────┐ ┌───────────┐ │
│  │  工具  │ │ LLM 循环  │ │  上下文   │ │
│  │  系统  │ │          │ │   管理    │ │
│  └────────┘ └──────────┘ └───────────┘ │
├─────────────────────────────────────────┤
│  LLM 提供商   │   工具系统 (MCP)         │
│  ┌──────┐┌────┐ │ ┌──────┐┌─────────┐ │
│  │Claude││ .. │ │ │ 内置  ││  MCP    │ │
│  │GPT   ││ .. │ │ │ 工具  ││ Servers │ │
│  │本地  ││    │ │ │      ││         │ │
│  └──────┘└────┘ │ └──────┘└─────────┘ │
├─────────────────────────────────────────┤
│    沙箱 (Landlock / Seatbelt / 应用层)   │
├─────────────────────────────────────────┤
│          权限与配置层                     │
│   (settings.json / CLAUDE.md / CRAB.md) │
└─────────────────────────────────────────┘
```

## 快速开始

> 代码尚未就绪 — 开发将从 M0（workspace 骨架 + CI）启动，详见下方 [开发路线图](#开发路线图)。

```bash
# 即将推出
git clone https://github.com/crabforge/crab-code.git
cd crab-code && cargo build --release
crab
```

## 配置

Crab Code 兼容 Claude Code 的配置规范，同时支持可选扩展：

```bash
# 你现有的配置直接生效
~/.claude/settings.json          # Crab Code 直接读取
your-project/CLAUDE.md           # Crab Code 直接读取

# Crab Code 扩展（可选）
~/.config/crab-code/config.toml  # 多 provider 配置、额外设置
your-project/CRAB.md             # 额外的项目指令
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

# 不同任务使用不同 provider
[routing]
planning = "default"        # 规划用 Claude
execution = "local"         # 简单编辑用本地模型
```

## 对比

| | Crab Code | Claude Code | Codex CLI |
|--|-----------|-------------|-----------|
| 开源 | ✅ Apache 2.0 | ❌ 闭源 | ✅ Apache 2.0 |
| 实现语言 | Rust | TypeScript (Node.js) | Rust |
| 模型无关 | ✅ 任意 provider | Anthropic + OpenAI 兼容¹ | 仅 OpenAI |
| 工作流兼容 | ✅ 读取 CLAUDE.md | — | ❌ |
| 自部署 | ✅ | ❌ | ✅ |
| 沙箱 | OS 级 + 应用层 | 应用层（7 层） | OS 级（内核） |
| 插件系统 | ✅ (wire protocol) | ✅ (Skills + Plugins) | ✅ (Plugins) |
| MCP 支持 | ✅ | ✅ (6 传输) | ✅ (2 传输) |
| 多 Agent | ✅ | ✅ (Coordinator + Swarm) | ✅ (Coordinator + Worker) |
| TUI 框架 | ratatui | Ink (React) | ratatui |

> ¹ Claude Code 通过 `CLAUDE_CODE_USE_OPENAI=1` 新增了实验性 OpenAI 兼容端点支持（Ollama、DeepSeek、vLLM 等）。

## 开发路线图

```
M0  项目骨架 + CI               ███░░░░░░░  Workspace、GitHub Actions、deny.toml
M1  领域模型 (core+common)      ░░░░░░░░░░  消息/工具/事件类型 + tracing
M2  流式 API (api+auth)         ░░░░░░░░░░  Anthropic + OpenAI 兼容 provider
M3  核心工具 (tools+fs+proc)    ░░░░░░░░░░  6 个内置工具 + 安全测试
M4  Agent 循环 (session+agent)  ░░░░░░░░░░  query loop + readline REPL [可自用]
M5  终端 UI (tui)               ░░░░░░░░░░  ratatui 交互式 REPL
M6  配置 + 上下文管理            ░░░░░░░░░░  权限、CLAUDE.md、上下文压缩
M7  MCP + 多 Agent              ░░░░░░░░░░  MCP 客户端、AgentTool、技能系统
```

<!-- 详细架构和规划文档在独立的私有仓库中维护 -->

## 参与贡献

我们需要你的帮助！Crab Code 从零独立构建，有大量工作要做。

<!-- 贡献指南即将推出 -->

```
需要帮助的方向：
├── Agent 循环与工具执行
├── 配置兼容层
├── LLM provider 集成
├── 终端 UI (ratatui)
├── MCP 客户端实现
├── OS 级沙箱
├── 测试与性能基准
└── 文档与国际化
```

## 许可证

[Apache License 2.0](LICENSE)

---

<div align="center">

<br>

**由 [CrabForge](https://github.com/crabforge) 社区用 🦀 打造**

*Claude Code 展示了 Agentic Coding 的未来，Crab Code 让每个人都能参与构建。*

</div>
