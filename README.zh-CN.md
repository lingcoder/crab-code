<div align="center">

<img src="assets/logo-horizontal.svg" width="360" alt="Crab Code" />

**Claude Code 的开源替代品，从零用 Rust 构建。**

*受 Claude Code 的 Agentic 工作流启发 -- 开源、Rust 原生、支持任意 LLM。*

[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/crabforge/crab-code/actions/workflows/ci.yml/badge.svg)](https://github.com/crabforge/crab-code/actions/workflows/ci.yml)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#贡献)

[English](README.md) | **中文**

</div>

---

> **状态：积极开发中** -- 已实现 32 个内置工具、6 种权限模式、扩展思维、多 Agent 协调，16 个 crate 共 3100+ 测试。

## 什么是 Crab Code？

[Claude Code](https://docs.anthropic.com/en/docs/claude-code) 开创了 Agentic Coding CLI -- 一个不仅能建议代码，还能在终端中自主思考、规划和执行的 AI。

**Crab Code** 将这种体验带到开源世界，完全用 Rust 从零独立构建：

- **完全开源** -- Apache 2.0 许可，无功能限制，无黑盒
- **Rust 原生性能** -- 瞬时启动，极低内存，无 Node.js 开销
- **模型无关** -- Claude、GPT、DeepSeek、Qwen、Ollama 或任何 OpenAI 兼容 API
- **安全可控** -- 6 种权限模式 (default, acceptEdits, dontAsk, bypassPermissions, plan, auto)
- **MCP 兼容** -- 支持 stdio、SSE、WebSocket 传输
- **对齐 Claude Code** -- CLI 参数、斜杠命令、工具和工作流与 Claude Code 行为一致

## 快速开始

```bash
git clone https://github.com/crabforge/crab-code.git
cd crab-code
cargo build --release

# 设置 API 密钥
export ANTHROPIC_API_KEY=sk-ant-...

# 交互式 TUI 模式
./target/release/crab

# 单次模式
./target/release/crab "解释这个代码库"

# 打印模式（非交互）
./target/release/crab -p "修复 main.rs 中的 bug"

# 使用其他提供商
./target/release/crab --provider openai --model gpt-4o "重构认证模块"
```

### 兼容 Claude Code 配置

Crab Code 支持 Claude Code 的 `settings.json` 格式，包括 `env` 字段：

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

## 核心功能

### Agent 循环

- **SSE 流式输出** -- LLM API 实时逐 token 输出
- **工具执行循环** -- 模型推理 -> 工具调用 -> 权限检查 -> 执行 -> 结果 -> 下一轮
- **扩展思维** -- 支持 budget_tokens 深度推理（Anthropic thinking blocks）
- **重试 + 降级** -- 瞬态错误自动重试，过载时自动切换备用模型
- **Effort 级别** -- low/medium/high/max 映射到 API 参数

### 内置工具（32 个）

| 类别 | 工具 |
|------|------|
| 文件 I/O | Read, Write, Edit, Glob, Grep |
| 执行 | Bash, PowerShell (Windows) |
| Agent | AgentTool（子 Agent）, TeamCreate, TeamDelete, SendMessage |
| 任务 | TaskCreate, TaskGet, TaskList, TaskUpdate, TaskStop, TaskOutput |
| 调度 | CronCreate, CronDelete, CronList |
| 规划 | EnterPlanMode, ExitPlanMode |
| Notebook | NotebookEdit, NotebookRead |
| Web | WebFetch, WebSearch |
| LSP | LSP（跳转定义、查找引用、悬浮提示、符号） |
| Worktree | EnterWorktree, ExitWorktree |
| 远程 | RemoteTrigger |
| 其他 | AskUserQuestion, Skill |

### 权限系统

6 种模式，与 Claude Code 对齐：

- **default** -- 危险操作前提示确认
- **acceptEdits** -- 自动批准文件编辑，Bash 仍需确认
- **dontAsk** -- 自动批准一切
- **bypassPermissions** -- 跳过所有检查
- **plan** -- 只读模式，执行前需批准计划
- **auto** -- AI 分类器自动批准低风险操作

支持工具级过滤：`--allowedTools` / `--disallowedTools`，支持 glob 模式（如 `Bash(git:*)`、`Edit`）。

### 斜杠命令（20+）

`/help` `/clear` `/compact` `/cost` `/status` `/memory` `/init` `/model` `/config` `/permissions` `/resume` `/history` `/export` `/doctor` `/diff` `/review` `/plan` `/exit` `/fast` `/effort` `/add-dir` `/files` `/thinking` 等。

### LLM 提供商

- **Anthropic** -- Messages API + SSE 流式（默认：`claude-sonnet-4-6`）
- **OpenAI 兼容** -- Chat Completions API（GPT、DeepSeek、Qwen、Ollama、vLLM 等）
- **AWS Bedrock** -- SigV4 签名 + 推理配置文件发现
- **GCP Vertex AI** -- ADC 认证

### MCP（模型上下文协议）

- 支持 stdio、SSE、WebSocket 传输
- `McpToolAdapter` 将 MCP 工具桥接到原生 `Tool` trait
- 通过 `~/.crab/settings.json` 或 `--mcp-config` 配置

### 会话管理

- 自动保存对话历史
- `--continue` / `-c` 继续上次会话
- `--resume <id>` 恢复指定会话
- `--fork-session` 恢复时分叉
- `--name` 友好会话名称
- 上下文窗口 80% 阈值自动压缩

### Hook 系统

- `PreToolUse` / `PostToolUse` / `UserPromptSubmit` 触发器
- Shell 命令执行，返回 Allow / Deny / Modify
- 在 `settings.json` 中配置

### 交互式 TUI

- ratatui + crossterm 终端界面
- Markdown 渲染 + 语法高亮
- Vim 模式编辑
- 斜杠命令和文件路径自动补全
- 权限确认对话框
- 状态栏实时费用追踪

## 架构

4 层 16 crate 的 Rust workspace：

```
第 4 层（入口）    cli          daemon        xtask
                    |              |
第 3 层（编排）    agent         session
                    |              |
第 2 层（服务）    api   tools   mcp   tui   plugin   telemetry
                    |     |      |     |      |         |
第 1 层（基础）    common   core   config   auth
```

关键设计决策：
- **异步运行时**: tokio（多线程）
- **LLM 派发**: `enum LlmBackend` -- 零动态派发，穷举匹配
- **工具系统**: `trait Tool` + JSON Schema 发现，`ToolRegistry` + `ToolExecutor`
- **TUI**: ratatui + crossterm，即时模式渲染
- **错误处理**: `thiserror`（库）+ `anyhow`（应用）

> 完整架构详见 [`docs/architecture.md`](docs/architecture.md)

## 配置

```bash
# 全局配置
~/.crab/settings.json        # API 密钥、提供商设置、MCP 服务器
~/.crab/memory/              # 持久化记忆文件
~/.crab/sessions/            # 保存的会话
~/.crab/skills/              # 全局 Skill 定义

# 项目配置
your-project/CRAB.md         # 项目指令（类似 CLAUDE.md）
your-project/.crab/settings.json  # 项目级覆盖
your-project/.crab/skills/   # 项目 Skill
```

## CLI 用法

```bash
crab                              # 交互式 TUI 模式
crab "你的提示"                    # 单次模式
crab -p "你的提示"                 # 打印模式（非交互）
crab -c                           # 继续上次会话
crab --provider openai            # 使用 OpenAI 兼容提供商
crab --model opus                 # 模型别名 (sonnet/opus/haiku)
crab --permission-mode plan       # 规划模式
crab --effort high                # 设置 effort 级别
crab --resume <session-id>        # 恢复指定会话
crab doctor                       # 运行诊断
crab auth login                   # 配置认证
```

## 构建与开发

```bash
cargo build --workspace                    # 构建全部
cargo test --workspace                     # 运行所有测试（3100+）
cargo clippy --workspace -- -D warnings    # Lint 检查
cargo fmt --all --check                    # 格式检查
cargo run --bin crab                       # 运行 CLI
```

## 对比

| | Crab Code | Claude Code | [OpenCode](https://github.com/anomalyco/opencode) | Codex CLI |
|--|-----------|-------------|----------|-----------|
| 开源 | Apache 2.0 | 闭源 | MIT | Apache 2.0 |
| 语言 | Rust | TypeScript (Bun) | TypeScript | Rust |
| 模型无关 | 任意提供商 | Anthropic + AWS/GCP | 任意提供商 | 仅 OpenAI |
| 自托管 | 支持 | 不支持 | 支持 | 支持 |
| MCP | stdio + SSE + WS | 6 种传输 | LSP | 2 种传输 |
| TUI | ratatui | Ink (React) | Custom | ratatui |
| 内置工具 | 32 | 30+ | ~10 | ~10 |
| 权限模式 | 6 | 6 | 2 | 3 |

## 贡献

欢迎参与！以下是需要帮助的方向：

- 端到端集成测试
- OS 级沙箱（Landlock / Seatbelt / Windows Job Object）
- MCP WebSocket 传输测试
- 更多 LLM 提供商测试
- 文档与国际化

## 许可证

[Apache License 2.0](LICENSE)

---

<div align="center">

**由 [CrabForge](https://github.com/crabforge) 社区用 Rust 打造**

*Claude Code 展示了 Agentic Coding 的未来，Crab Code 让每个人都能参与。*

</div>
