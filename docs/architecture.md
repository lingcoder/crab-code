# Crab Code 架构设计

> 版本：v2.1
> 更新：2026-04-06

---

## 一、全局架构鸟瞰

### 四层架构

| 层级 | Crate | 职责 |
|------|-------|------|
| **Layer 4** 入口层 | `crates/cli` `crates/daemon` | CLI 入口 (clap)、后台守护进程 |
| **Layer 3** 引擎层 | `agent` `session` | 多 Agent 协调、会话管理、上下文压缩 |
| **Layer 2** 服务层 | `tools` `mcp` `api` `fs` `process` `plugin` `telemetry` `tui` `bridge` | 工具系统、MCP 协议栈、多模型 API 客户端、文件/进程操作、TUI 组件、IDE 桥接 |
| **Layer 1** 基础层 | `core` `common` `config` `auth` | 领域模型、配置热重载、Auth 认证 |

> 依赖方向：上层依赖下层，禁止反向依赖。`core` 定义 `Tool` trait 避免 tools/agent 循环依赖。

### 架构全景图

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Layer 4: 入口层                              │
│  ┌──────────────────────────┐   ┌────────────────────────────────┐  │
│  │      crates/cli          │   │       crates/daemon            │  │
│  │  clap 子命令 + tokio rt  │   │  后台守护 + session pool      │  │
│  └────────────┬─────────────┘   └──────────────┬─────────────────┘  │
├───────────────┼────────────────────────────────┼────────────────────┤
│               │        Layer 3: 引擎层         │                    │
│  ┌────────────▼─────────────┐   ┌──────────────▼─────────────────┐  │
│  │         agent            │   │          session               │  │
│  │  Agent 编排 + 任务分发   │   │  会话状态 + 上下文压缩 + 记忆  │  │
│  └──┬───────────┬───────────┘   └───────┬──────────────┬─────────┘  │
├─────┼───────────┼───────────────────────┼──────────────┼────────────┤
│     │           │   Layer 2: 服务层     │              │            │
│  ┌──▼────┐  ┌───▼───┐  ┌────┐  ┌───────▼──┐  ┌───────▼────┐      │
│  │ tools │  │  mcp  │  │tui │  │   api    │  │  telemetry │      │
│  │ 21+   │  │JSON-  │  │rata│  │LlmBack- │  │OpenTelemetry│      │
│  │内置   │  │RPC    │  │tui │  │end 枚举  │  │  traces    │      │
│  └┬────┬─┘  └───────┘  └────┘  └──────────┘  └────────────┘      │
│   │    │                                                           │
│  ┌▼──┐ ┌▼──────┐  ┌──────┐                                        │
│  │fs │ │process │  │plugin│                                        │
│  │glob│ │子进程  │  │WASM  │                                        │
│  │grep│ │信号    │  │沙箱  │                                        │
│  └───┘ └───────┘  └──────┘                                        │
├───────────────────────────────────────────────────────────────────┤
│                      Layer 1: 基础层                               │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐          │
│  │   core   │  │  common  │  │  config  │  │   auth   │          │
│  │领域模型  │  │错误/工具 │  │多层配置  │  │OAuth/Key │          │
│  │Tool trait│  │路径/文本 │  │CRAB.md   │  │Keychain  │          │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘          │
└───────────────────────────────────────────────────────────────────┘
```

### 与 Claude Code 五层架构映射

| Claude Code (TS) | 路径 | Crab Code (Rust) | 说明 |
|-------------------|------|-------------------|------|
| **入口层** entrypoints/ | `cli.tsx` `main.tsx` | `crates/cli` `crates/daemon` | CC 用 React/Ink 渲染，Crab 用 ratatui |
| **命令层** commands/ | `query.ts` `QueryEngine.ts` | `agent` + `session` | CC 的 query loop 对应 agent 编排 |
| **工具层** tools/ | 52 Tool 目录 | `tools` + `mcp` | CC 工具与 MCP 混在 services/，Crab 独立拆分 |
| **服务层** services/ | `api/` `mcp/` `oauth/` `compact/` | `api` `mcp` `auth` `plugin` `telemetry` | CC 服务层较扁平，Crab 按职责细分 |
| **基础层** utils/ types/ | `Tool.ts` `context.ts` | `core` `common` `config` | CC 类型散落各处，Crab 集中到 core |

### 核心设计哲学

1. **core 零 I/O** — 纯数据结构和 trait 定义，可被 CLI/GUI/WASM 任意前端复用
2. **消息循环驱动** — 一切围绕 query loop：用户输入 → API 调用 → 工具执行 → 结果返回
3. **Workspace 隔离** — 14 个 library crate 职责正交（~176 模块），增量编译只触发改动部分
4. **Feature Flag 控依赖** — 不用 Bedrock 不编译 AWS SDK，不用 WASM 不编译 wasmtime

---

## 二、为什么选 Rust

### 2.1 Go vs Rust 对比

| 维度 | Go | Rust | 结论 |
|------|-----|------|------|
| **开发效率** | 快，学习曲线低 | 慢 2-3x，生命周期/所有权摩擦 | Go 胜 |
| **CLI 生态** | cobra 成熟 | clap 同样成熟 | 平手 |
| **TUI** | Charm (bubbletea) 优秀 | ratatui 优秀 | 平手 |
| **GUI 扩展** | 弱（fyne/gio 小众） | **强**（Tauri 2.0 桌面+移动端） | **Rust 胜** |
| **WASM** | Go→WASM ~10MB+，性能差 | **一等公民**，产物小、性能原生 | **Rust 胜** |
| **FFI/跨语言** | cgo 有性能惩罚 | **零开销 FFI**，C ABI 原生 | **Rust 胜** |
| **AI/ML 生态** | 绑定少 | candle、burn、ort(ONNX) | **Rust 胜** |
| **序列化** | encoding/* 够用 | serde **统治级** | **Rust 胜** |
| **编译速度** | 10-30s | 5-15min | Go 胜 |
| **交叉编译** | 极简单 | 中等（需 target 工具链） | Go 胜 |
| **招人** | 开发者池大 | 开发者池小 | Go 胜 |

### 2.2 选择 Rust 的 5 个核心理由

1. **未来扩展天花板高** — CLI → Tauri 桌面 → 浏览器 WASM → 移动端，核心逻辑 100% 共享
2. **Tauri 生态** — Electron 主流替代，内存 20-30MB vs 150MB+，包体 5-15MB vs 100MB+
3. **三方库质量** — serde、tokio、ratatui、clap 均为各领域顶尖实现
4. **本地 AI 推理** — 未来可通过 candle/burn 集成本地模型，无需 cgo 桥接
5. **插件沙箱** — wasmtime 本身就是 Rust 写的，WASM 插件系统天然适配

### 2.3 性能预期对比

| 指标 | TypeScript/Bun | Rust | 倍数 |
|------|---------------|------|------|
| **冷启动** | ~135ms | ~5-10ms | **15-25x** |
| **内存占用（空闲）** | ~80-150MB | ~5-10MB | **10-20x** |
| **API 流式处理** | 基线 | ~同等 | 1x（I/O bound） |
| **终端 UI 渲染** | 较慢（React 开销） | 快（ratatui 零开销） | **3-5x** |
| **JSON 序列化** | 快（V8 内置） | 最快（serde 零拷贝） | **2-3x** |
| **二进制大小** | ~100MB+（含 runtime） | ~10-20MB | **5-10x** |

---

## 三、核心库替代方案

共 28 个 TS → Rust 映射，按功能分组：

### 3.1 CLI / UI

| # | 功能 | TypeScript 原版 | Rust 替代 | 版本 | 文档 |
|---|------|----------------|-----------|------|------|
| 1 | CLI 框架 | Commander.js | clap (derive) | 4.x | [docs.rs/clap](https://docs.rs/clap) |
| 2 | 终端 UI | React/Ink | ratatui + crossterm | 0.30 / 0.29 | [ratatui.rs](https://ratatui.rs) |
| 3 | 终端样式 | chalk | crossterm Style | 0.29 | [docs.rs/crossterm](https://docs.rs/crossterm) |
| 4 | Markdown 渲染 | marked | pulldown-cmark | 0.13 | [docs.rs/pulldown-cmark](https://docs.rs/pulldown-cmark) |
| 5 | 语法高亮 | highlight.js | syntect | 5.x | [docs.rs/syntect](https://docs.rs/syntect) |
| 6 | 模糊搜索 | Fuse.js | nucleo | 0.5 | [docs.rs/nucleo](https://docs.rs/nucleo) |

### 3.2 网络 / API

| # | 功能 | TypeScript 原版 | Rust 替代 | 版本 | 文档 |
|---|------|----------------|-----------|------|------|
| 7 | HTTP 客户端 | axios/undici | reqwest | 0.13 | [docs.rs/reqwest](https://docs.rs/reqwest) |
| 8 | WebSocket | ws | tokio-tungstenite | 0.29 | [docs.rs/tokio-tungstenite](https://docs.rs/tokio-tungstenite) |
| 9 | 流式 SSE | Anthropic SDK | eventsource-stream | 0.2 | [docs.rs/eventsource-stream](https://docs.rs/eventsource-stream) |
| 10 | OAuth | google-auth-library | oauth2 | 5.x | [docs.rs/oauth2](https://docs.rs/oauth2) |

### 3.3 序列化 / 校验

| # | 功能 | TypeScript 原版 | Rust 替代 | 版本 | 文档 |
|---|------|----------------|-----------|------|------|
| 11 | JSON | 内置 JSON | serde + serde_json | 1.x / 1.x | [serde.rs](https://serde.rs) |
| 12 | YAML | yaml | serde_yml | 0.0.12 | [docs.rs/serde_yml](https://docs.rs/serde_yml) |
| 13 | TOML | — | toml | 0.8 | [docs.rs/toml](https://docs.rs/toml) |
| 14 | Schema 校验 | Zod | schemars | 1.x | [docs.rs/schemars](https://docs.rs/schemars) |

### 3.4 文件系统 / 搜索

| # | 功能 | TypeScript 原版 | Rust 替代 | 版本 | 文档 |
|---|------|----------------|-----------|------|------|
| 15 | Glob | glob | globset | 0.4 | [docs.rs/globset](https://docs.rs/globset) |
| 16 | Grep/搜索 | ripgrep 绑定 | grep crate 家族 | 0.3 | [docs.rs/grep](https://docs.rs/grep) |
| 17 | Gitignore | — | ignore | 0.4 | [docs.rs/ignore](https://docs.rs/ignore) |
| 18 | 文件监听 | chokidar | notify | 8.x | [docs.rs/notify](https://docs.rs/notify) |
| 19 | Diff | diff | similar | 3.x | [docs.rs/similar](https://docs.rs/similar) |
| 20 | 文件锁 | proper-lockfile | fd-lock | 4.0 | [docs.rs/fd-lock](https://docs.rs/fd-lock) |

### 3.5 系统 / 进程

| # | 功能 | TypeScript 原版 | Rust 替代 | 版本 | 文档 |
|---|------|----------------|-----------|------|------|
| 21 | 子进程 | execa | tokio::process | 1.x | [docs.rs/tokio](https://docs.rs/tokio) |
| 22 | 进程树 | tree-kill | sysinfo | 0.38 | [docs.rs/sysinfo](https://docs.rs/sysinfo) |
| 23 | 系统目录 | — | directories | 6.x | [docs.rs/directories](https://docs.rs/directories) |
| 24 | Keychain | 自实现 | keyring | 3.x | [docs.rs/keyring](https://docs.rs/keyring) |

### 3.6 可观测性 / 缓存

| # | 功能 | TypeScript 原版 | Rust 替代 | 版本 | 文档 |
|---|------|----------------|-----------|------|------|
| 25 | OpenTelemetry | @opentelemetry/* | opentelemetry-rust | 0.31 | [docs.rs/opentelemetry](https://docs.rs/opentelemetry) |
| 26 | 日志/追踪 | console.log | tracing | 0.1 | [docs.rs/tracing](https://docs.rs/tracing) |
| 27 | LRU 缓存 | lru-cache | lru | 0.12 | [docs.rs/lru](https://docs.rs/lru) |
| 28 | 错误处理 | Error class | thiserror + anyhow | 2.x / 1.x | [docs.rs/thiserror](https://docs.rs/thiserror) |

---

## 四、Workspace 工程结构

### 4.1 完整目录树

```
crab-code/
├── Cargo.toml                         # workspace root
├── Cargo.lock
├── rust-toolchain.toml                # pinned toolchain
├── rustfmt.toml                       # 格式化配置
├── clippy.toml                        # lint 配置
├── .gitignore
├── LICENSE
│
├── crates/
│   ├── common/                        # crab-common: 共享基础
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs               # thiserror 统一错误枚举
│   │       ├── result.rs              # type Result<T>
│   │       ├── text.rs                # Unicode 宽度、ANSI strip
│   │       ├── path.rs                # 跨平台路径规范化
│   │       └── id.rs                  # ULID 生成
│   │
│   ├── core/                          # crab-core: 领域模型
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── message.rs             # Message, Role, ContentBlock
│   │       ├── conversation.rs        # Conversation, Turn
│   │       ├── tool.rs                # trait Tool + ToolContext + ToolOutput
│   │       ├── model.rs               # ModelId, TokenUsage, CostTracker
│   │       ├── permission/            # 权限系统（模块目录）
│   │       │   ├── mod.rs             # PermissionMode, PermissionPolicy, re-exports
│   │       │   ├── rule_parser.rs     # [P0] 规则 AST 解析："Bash(cmd:git*)" 格式
│   │       │   ├── path_validator.rs  # [P0] 文件路径权限引擎、symlink 解析
│   │       │   ├── denial_tracker.rs  # [P1] 连续拒绝计数、模式检测
│   │       │   ├── explainer.rs       # [P1] 人类可读的权限决策解释
│   │       │   └── shadowed_rules.rs  # [P2] 被遮蔽规则检测
│   │       ├── config.rs              # trait ConfigSource
│   │       ├── event.rs               # 领域事件枚举（crate 间解耦通信）
│   │       └── capability.rs          # Agent 能力声明
│   │
│   ├── config/                        # crab-config: 配置系统
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── settings.rs            # settings.json 读写、层级合并
│   │       ├── crab_md.rs             # CRAB.md 解析（项目/用户/全局）
│   │       ├── hooks.rs               # Hook 定义与触发
│   │       ├── feature_flag.rs        # [P1] 运行时 Feature Flag 管理（本地评估）
│   │       ├── policy.rs              # [P0] 权限策略限制、MDM/managed-path
│   │       ├── keybinding.rs          # [P1] 快捷键 schema/解析/校验/resolver
│   │       ├── config_toml.rs         # config.toml 多提供商配置
│   │       ├── hot_reload.rs          # settings.json 热重载监听
│   │       ├── permissions.rs         # 权限决策统一入口
│   │       ├── validation.rs          # [P1] Settings 校验引擎
│   │       ├── settings_cache.rs      # [P1] 记忆化 settings 缓存
│   │       ├── change_detector.rs     # [P2] 按 source 的变更检测
│   │       └── mdm.rs                 # [P2] 企业 MDM managed settings
│   │
│   ├── auth/                          # crab-auth: 认证
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── oauth.rs               # OAuth2 PKCE 流程
│   │       ├── keychain.rs            # 系统 Keychain (macOS/Win/Linux)
│   │       ├── api_key.rs             # API Key 管理
│   │       ├── bedrock_auth.rs        # AWS SigV4 签名 (feature)
│   │       ├── vertex_auth.rs         # GCP Vertex 认证
│   │       ├── aws_iam.rs             # AWS IAM Roles + IRSA
│   │       ├── gcp_identity.rs        # GCP Workload Identity Federation
│   │       └── credential_chain.rs    # 凭证链（优先级顺序解析）
│   │
│   ├── api/                           # crab-api: LLM API 客户端
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                 # LlmBackend 枚举 + create_backend()
│   │       ├── types.rs               # 内部统一请求/响应/事件类型
│   │       ├── anthropic/             # 独立 Anthropic Messages API client
│   │       │   ├── mod.rs
│   │       │   ├── client.rs          # HTTP + SSE + retry
│   │       │   ├── types.rs           # Anthropic 原生 API 类型
│   │       │   └── convert.rs         # Anthropic ↔ 内部类型转换
│   │       ├── openai/                # 独立 OpenAI Chat Completions client
│   │       │   ├── mod.rs
│   │       │   ├── client.rs          # HTTP + SSE + retry
│   │       │   ├── types.rs           # OpenAI 原生 API 类型
│   │       │   └── convert.rs         # OpenAI ↔ 内部类型转换
│   │       ├── bedrock.rs             # AWS Bedrock (feature, 包装 anthropic)
│   │       ├── vertex.rs              # Google Vertex (feature, 包装 anthropic)
│   │       ├── rate_limit.rs          # 共享速率限制、指数退避
│   │       ├── cache.rs               # Prompt cache (Anthropic 路径)
│   │       ├── error.rs
│   │       ├── streaming.rs           # 流式工具调用解析
│   │       ├── fallback.rs            # 多模型回退链
│   │       ├── capabilities.rs        # 模型能力协商与发现
│   │       ├── context_optimizer.rs   # 上下文窗口优化 + 智能截断
│   │       ├── retry_strategy.rs      # 增强重试策略
│   │       ├── error_classifier.rs    # 错误分类（可重试/不可重试）
│   │       ├── token_estimation.rs    # [P1] Token 数量近似估算
│   │       ├── ttft_tracker.rs        # [P1] Time-to-first-token 延迟统计
│   │       ├── fast_mode.rs           # [P1] 快速模式切换
│   │       └── usage_tracker.rs       # [P1] 使用量聚合（per-session/model）
│   │
│   ├── mcp/                           # crab-mcp: MCP façade + 协议适配层
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── protocol.rs            # JSON-RPC 消息定义
│   │       ├── client.rs              # MCP 客户端
│   │       ├── server.rs              # MCP 服务端
│   │       ├── manager.rs             # 生命周期管理，多 server 协调
│   │       ├── transport/
│   │       │   ├── mod.rs
│   │       │   ├── stdio.rs           # stdin/stdout 传输
│   │       │   └── ws.rs              # WebSocket (feature)
│   │       ├── resource.rs            # Resource 缓存、模板
│   │       ├── discovery.rs           # Server 自动发现
│   │       ├── sse_server.rs          # SSE server transport（crab 作为 server）
│   │       ├── sampling.rs            # MCP sampling（LLM 推理请求）
│   │       ├── roots.rs               # MCP roots（工作区根目录声明）
│   │       ├── logging.rs             # MCP logging 协议消息
│   │       ├── handshake.rs           # 初始化握手流程
│   │       ├── negotiation.rs         # 能力协商
│   │       ├── capability.rs          # 能力声明类型
│   │       ├── notification.rs        # 服务端通知推送
│   │       ├── progress.rs            # 进度报告
│   │       ├── cancellation.rs        # 请求取消机制
│   │       ├── health.rs              # 健康检查 + 心跳
│   │       ├── auth.rs                # [P1] MCP OAuth2/API key 认证
│   │       ├── channel_permissions.rs # [P1] Channel 级工具/资源权限
│   │       ├── elicitation.rs         # [P1] 用户输入请求处理
│   │       ├── env_expansion.rs       # [P1] 配置中 ${VAR} 环境变量展开
│   │       ├── official_registry.rs   # [P2] 官方 MCP server 注册表
│   │       └── normalization.rs       # [P2] 工具/资源名称规范化
│   │
│   ├── fs/                            # crab-fs: 文件系统
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── glob.rs                # globset 封装
│   │       ├── grep.rs                # ripgrep 内核集成
│   │       ├── gitignore.rs           # .gitignore 规则解析
│   │       ├── watch.rs               # notify 文件监听（含防抖、批量）
│   │       ├── lock.rs                # 文件锁 (fd-lock)
│   │       ├── diff.rs                # similar 封装, patch 生成
│   │       └── symlink.rs             # 符号链接处理 + 安全解析
│   │
│   ├── process/                       # crab-process: 子进程管理
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── spawn.rs               # 子进程启动、环境继承
│   │       ├── pty.rs                 # 伪终端 (feature = "pty")
│   │       ├── tree.rs                # 进程树 kill (sysinfo)
│   │       ├── signal.rs              # 信号处理、优雅关闭
│   │       └── sandbox.rs             # 沙箱策略 (feature = "sandbox")
│   │
│   ├── tools/                         # crab-tools: 工具系统
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── registry.rs            # ToolRegistry: 注册、查找
│   │       ├── executor.rs            # 带权限检查的统一执行器
│   │       ├── builtin/
│   │       │   ├── mod.rs
│   │       │   ├── bash.rs            # BashTool
│   │       │   ├── bash_security.rs   # Bash 安全检查
│   │       │   ├── bash_classifier.rs # [P0] Bash 命令分类（read-only/write/dangerous）
│   │       │   ├── read.rs            # ReadTool
│   │       │   ├── read_enhanced.rs   # 增强文件读取（PDF/图片/Notebook）
│   │       │   ├── edit.rs            # EditTool (diff-based)
│   │       │   ├── write.rs           # WriteTool
│   │       │   ├── glob.rs            # GlobTool
│   │       │   ├── grep.rs            # GrepTool
│   │       │   ├── lsp.rs             # LSP 集成工具
│   │       │   ├── web_search.rs      # WebSearchTool
│   │       │   ├── web_fetch.rs       # WebFetchTool
│   │       │   ├── web_cache.rs       # 网页缓存
│   │       │   ├── web_formatter.rs   # 网页格式化
│   │       │   ├── web_browser.rs     # [P2] Playwright/CDP 浏览器自动化
│   │       │   ├── agent.rs           # AgentTool (子 Agent)
│   │       │   ├── send_message.rs    # [P0] SendMessageTool（跨 Agent 消息）
│   │       │   ├── skill.rs           # [P0] SkillTool（按名称调用 skill）
│   │       │   ├── notebook.rs        # NotebookTool
│   │       │   ├── task.rs            # TaskCreate/Get/List/Update
│   │       │   ├── todo_write.rs      # [P1] TodoWriteTool（结构化 TODO）
│   │       │   ├── team.rs            # TeamCreate/Delete
│   │       │   ├── mcp_tool.rs        # MCP 工具适配器
│   │       │   ├── mcp_resource.rs    # [P1] ListMcpResources + ReadMcpResource
│   │       │   ├── mcp_auth.rs        # [P1] MCP 服务器认证工具
│   │       │   ├── worktree.rs        # Git Worktree 工具
│   │       │   ├── ask_user.rs        # 用户交互工具
│   │       │   ├── image_read.rs      # 图片读取工具
│   │       │   ├── plan_mode.rs       # 计划模式工具
│   │       │   ├── plan_file.rs       # 计划文件操作
│   │       │   ├── plan_approval.rs   # 计划审批工具
│   │       │   ├── verify_plan.rs     # [P1] 计划执行验证
│   │       │   ├── config_tool.rs     # [P1] ConfigTool（编程式 settings 读写）
│   │       │   ├── brief.rs           # [P1] BriefTool（对话摘要）
│   │       │   ├── snip.rs            # [P1] SnipTool（裁剪大工具输出）
│   │       │   ├── sleep.rs           # [P1] SleepTool（异步等待）
│   │       │   ├── tool_search.rs     # [P1] ToolSearchTool（搜索可用工具）
│   │       │   ├── monitor.rs         # [P2] MonitorTool（文件/进程监控）
│   │       │   ├── workflow.rs        # [P2] WorkflowTool（多步工作流）
│   │       │   ├── send_user_file.rs  # [P2] SendUserFileTool
│   │       │   ├── powershell.rs      # PowerShellTool
│   │       │   ├── cron.rs            # CronCreate/Delete/List
│   │       │   └── remote_trigger.rs  # RemoteTriggerTool
│   │       ├── permission.rs          # 工具权限检查逻辑
│   │       ├── sandbox.rs             # 工具沙箱策略
│   │       ├── schema.rs              # 工具 schema 转换
│   │       └── tool_use_summary.rs    # [P1] 工具结果摘要生成
│   │
│   ├── session/                       # crab-session: 会话管理
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── conversation.rs        # 对话状态机，多轮管理
│   │       ├── context.rs             # 上下文窗口管理
│   │       ├── compaction.rs          # 消息压缩策略（5 级）
│   │       ├── micro_compact.rs       # [P0] 微压缩：逐条替换大工具结果
│   │       ├── auto_compact.rs        # [P1] 自动压缩触发器 + 清理
│   │       ├── snip_compact.rs        # [P1] Snip 压缩："[snipped]" 标记
│   │       ├── history.rs             # 会话持久化、恢复、搜索、导出
│   │       ├── memory.rs              # 记忆系统 (文件持久化)
│   │       ├── memory_types.rs        # [P1] 记忆类型 schema (user/project/feedback)
│   │       ├── memory_relevance.rs    # [P1] 记忆相关性匹配与评分
│   │       ├── memory_extract.rs      # [P2] 自动记忆提取
│   │       ├── memory_age.rs          # [P2] 记忆老化与衰减
│   │       ├── team_memory.rs         # [P2] 团队记忆路径与加载
│   │       ├── cost.rs                # token 计数、费用追踪
│   │       ├── template.rs            # 会话模板 + 快速恢复
│   │       └── migration.rs           # [P2] 数据迁移系统
│   │
│   ├── agent/                         # crab-agent: 多 Agent 系统
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── coordinator.rs         # Agent 编排、workers pool + 工作窃取调度
│   │       ├── query_loop.rs          # 核心消息循环
│   │       ├── task.rs                # TaskList, 依赖图
│   │       ├── team.rs                # Team 创建、成员管理
│   │       ├── message_bus.rs         # Agent 间消息 (tokio::mpsc)
│   │       ├── message_router.rs      # Agent 间消息路由
│   │       ├── worker.rs              # 子 Agent worker
│   │       ├── system_prompt/         # 系统提示（模块目录）
│   │       │   ├── mod.rs             # re-exports
│   │       │   ├── builder.rs         # [重构] 主组装逻辑（原 system_prompt.rs）
│   │       │   ├── sections.rs        # [P0] 模块化 section 架构 + 动态边界
│   │       │   └── cache.rs           # [P1] per-section 记忆化缓存
│   │       ├── token_budget.rs        # [P1] Token 预算管理
│   │       ├── stop_hooks.rs          # [P1] 停止条件钩子
│   │       ├── summarizer.rs          # 对话摘要生成
│   │       ├── rollback.rs            # 回滚机制
│   │       ├── error_recovery.rs      # 错误恢复策略
│   │       ├── retry.rs               # 自动重试机制
│   │       ├── slash_commands.rs      # 斜杠命令注册与执行
│   │       ├── repl_commands.rs       # REPL 命令（/undo /branch /fork）
│   │       ├── effort.rs              # 模型 effort 级别
│   │       ├── git_context.rs         # Git 上下文收集
│   │       ├── pr_context.rs          # PR 上下文收集
│   │       ├── prompt_suggestion.rs   # [P2] 后续 prompt 建议
│   │       └── tips.rs                # [P2] 上下文提示
│   │
│   ├── tui/                           # crab-tui: 终端 UI（21 组件）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── app.rs                 # App 状态机，主循环
│   │       ├── event.rs               # crossterm Event → AppEvent 映射分发
│   │       ├── layout.rs              # 布局计算
│   │       ├── runner.rs              # TUI 运行器（启动/停止）
│   │       ├── keybindings.rs         # 快捷键配置绑定
│   │       ├── ansi.rs                # ANSI 转义 → ratatui Span 转换
│   │       ├── components/
│   │       │   ├── mod.rs
│   │       │   ├── input.rs           # 多行输入框 + Vim motion
│   │       │   ├── markdown.rs        # Markdown 渲染
│   │       │   ├── syntax.rs          # 代码高亮 (syntect)
│   │       │   ├── spinner.rs         # 加载指示器
│   │       │   ├── diff.rs            # Diff 可视化（unified）
│   │       │   ├── select.rs          # 选择列表
│   │       │   ├── dialog.rs          # 确认/权限对话框
│   │       │   ├── cost_bar.rs        # token/费用状态栏
│   │       │   ├── task_list.rs       # 任务进度面板
│   │       │   ├── autocomplete.rs    # 自动补全弹出框
│   │       │   ├── code_block.rs      # 代码块 + 复制按钮
│   │       │   ├── command_palette.rs # 命令面板（Ctrl+P）
│   │       │   ├── input_history.rs   # 输入历史（↑↓）
│   │       │   ├── loading.rs         # 加载动画组件
│   │       │   ├── notification.rs    # Toast 通知系统
│   │       │   ├── progress_indicator.rs # 进度指示器
│   │       │   ├── search.rs          # 全局搜索面板
│   │       │   ├── shortcut_hint.rs   # 快捷键提示栏
│   │       │   ├── status_bar.rs      # 增强状态栏
│   │       │   ├── tool_output.rs     # 工具输出折叠显示
│   │       │   ├── output_styles.rs   # [P1] 输出格式化样式配置
│   │       │   ├── permission_dialog.rs # [P1] 专用权限提示对话框
│   │       │   ├── session_sidebar.rs # [P1] 会话侧边栏
│   │       │   └── context_collapse.rs # [P2] 上下文折叠/展开
│   │       ├── vim/
│   │       │   ├── mod.rs
│   │       │   ├── motion.rs
│   │       │   ├── operator.rs
│   │       │   └── mode.rs
│   │       └── theme.rs               # 颜色主题（可自定义）
│   │
│   ├── plugin/                        # crab-plugin: 插件系统
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── skill.rs               # Skill 发现、加载、执行
│   │       ├── skill_builder.rs       # [P1] Skill builder API + MCP skill 加载
│   │       ├── bundled_skills.rs      # [P1] 内置 skill（commit/review/debug 等）
│   │       ├── wasm_runtime.rs        # WASM 沙箱 (feature = "wasm")
│   │       ├── manifest.rs            # 插件清单解析
│   │       ├── manager.rs             # 插件生命周期管理
│   │       ├── hook.rs                # 生命周期钩子执行
│   │       ├── hook_registry.rs       # [P0] 异步钩子注册表 + 事件广播
│   │       ├── hook_types.rs          # [P0] Agent/Http/Prompt 钩子 + SSRF guard
│   │       ├── hook_watchers.rs       # [P1] 文件变更触发钩子重注册
│   │       └── frontmatter_hooks.rs   # [P1] Frontmatter YAML 钩子注册
│   │
│   ├── telemetry/                     # crab-telemetry: 可观测性
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── tracer.rs              # OpenTelemetry tracer
│   │       ├── metrics.rs             # 自定义 metrics
│   │       ├── cost.rs                # 费用追踪
│   │       ├── export.rs              # [P1] 本地 OTLP 导出（无远程）
│   │       └── session_recorder.rs    # [P2] 会话录制（本地 transcript）
│   │
│   └── bridge/                        # [P1] crab-bridge: IDE 桥接/IPC
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── protocol.rs            # JSON-RPC IDE 通信消息类型
│           ├── repl_bridge.rs         # REPL 中继：IDE ↔ session
│           ├── remote_bridge.rs       # 远程连接：连接 daemon session
│           ├── ws_server.rs           # WebSocket server
│           ├── session_token.rs       # JWT session token
│           ├── trusted_device.rs      # 可信设备注册/验证
│           └── types.rs               # 共享类型
│
│   ├── cli/                           # crab-cli: 终端入口 (binary crate)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs                # #[tokio::main]
│   │       ├── commands/
│   │       │   ├── mod.rs
│   │       │   ├── chat.rs            # 默认交互模式
│   │       │   ├── run.rs             # 非交互单次执行
│   │       │   ├── session.rs         # ps, logs, attach, kill
│   │       │   ├── config.rs          # 配置管理
│   │       │   ├── mcp.rs             # MCP server 模式
│   │       │   └── serve.rs           # 服务模式
│   │       └── setup.rs               # 初始化、信号注册、panic hook
│   │
│   ├── daemon/                        # crab-daemon: 守护进程 (binary crate)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── protocol.rs            # IPC 消息协议
│   │       ├── server.rs              # 守护进程服务端
│   │       └── session_pool.rs        # 会话池管理
│
└── xtask/                             # 构建辅助脚本
    ├── Cargo.toml
    └── src/
        └── main.rs                    # codegen, release, bench
```

### 4.2 Crate 统计

| 类型 | 数量 | 说明 |
|------|------|------|
| Library crate | 15 | `crates/*`（含新增 `bridge`） |
| Binary crate | 2 | `crates/cli` `crates/daemon` |
| 辅助 crate | 1 | `xtask` |
| **合计** | **18** | — |
| 模块总数 | ~241 | 分布于 15 个 library crate（含 65 个新增文件） |
| 测试总数 | ~2654 | `cargo test --workspace`（2026-04-06） |

> 注：[P0]/[P1]/[P2] 标记表示 CCB 功能对齐优先级。未标记文件为已实现。

---

## 五、Crate 依赖关系

### 5.1 依赖关系图

```
                       ┌────────────┐
                       │ crates/cli │
                       └────┬───────┘
                            │ 依赖所有 crate
             ┌──────────────┼──────────────┐
             │              │              │
        ┌────▼────┐   ┌────▼─────┐  ┌─────▼────────┐
        │   tui   │   │  agent   │  │crates/daemon │
        └────┬────┘   └────┬─────┘  └──────┬───────┘
              │             │               │
              │        ┌────▼─────┐         │
              │        │ session  │◄────────┘
              │        └────┬─────┘
              │             │
              │        ┌────▼─────┐
              │        │  tools   │◄────────┐
                       └┬──┬──┬──┘         │
                        │  │  │            │
               ┌────────┘  │  └────────┐   │
               │           │           │   │
          ┌────▼──┐   ┌────▼──┐   ┌────▼───▼──┐
          │  fs   │   │  mcp  │   │  process   │
          └───┬───┘   └───┬───┘   └──────┬─────┘
              │           │              │
              │      ┌────▼──────────────┘
              │      │
         ┌────▼──────▼───┐    ┌───────────┐
         │     api       │    │  plugin   │
         └───────┬───────┘    └─────┬─────┘
                 │                  │
         ┌───────▼───────┐         │
         │     auth      │         │
         └───────┬───────┘         │
                 │                 │
         ┌───────▼─────────────────▼─────┐
         │            config             │
         └───────────────┬───────────────┘
                         │
         ┌───────────────▼───────────────┐
         │             core              │
         └───────────────┬───────────────┘
                         │
         ┌───────────────▼───────────────┐
         │            common             │
         └───────────────────────────────┘

                   ┌────────────┐
                   │ telemetry  │ ←── 独立旁路，任意 crate 可选依赖
                   └────────────┘
                   ┌────────────┐
                   │  bridge    │ ←── Layer 2 服务层，IDE 桥接
                   └────────────┘
```

### 5.2 依赖清单（自底向上）

| # | Crate | 内部依赖 | 说明 |
|---|-------|---------|------|
| 1 | **common** | 无 | 零依赖基础层 |
| 2 | **core** | common | 纯领域模型 |
| 3 | **config** | core, common | 配置读写合并 |
| 4 | **auth** | config, common | 认证凭证管理 |
| 5 | **api** | core, auth, common | LlmBackend 枚举 + Anthropic/OpenAI-compatible 独立 client |
| 6 | **fs** | common | 文件系统操作 |
| 7 | **process** | common | 子进程管理 |
| 8 | **mcp** | core, common | MCP 协议客户端/服务端 |
| 9 | **telemetry** | common | 独立旁路，可选 |
| 10 | **bridge** | core, config, common | [P1] IDE 桥接 / WebSocket IPC |
| 11 | **tools** | core, fs, process, mcp, config, common | 40+ 内置工具 |
| 12 | **session** | core, api, config, common | 会话 + 上下文压缩 + 记忆系统 |
| 13 | **agent** | core, session, tools, common | Agent 编排 |
| 14 | **plugin** | core, common | 技能/WASM 沙箱 + 钩子系统 |
| 15 | **tui** | core, session, config, common | 终端 UI（不直接依赖 tools，通过 core::Event 接收工具状态） |
| 16 | **cli** (bin) | 所有 crate | 极薄入口 |
| 17 | **daemon** (bin) | core, session, api, tools, config, agent, bridge, common | 后台服务 |

### 5.3 依赖方向原则

```
规则 1: 上层 → 下层，禁止反向
规则 2: 同层 crate 不互相依赖（fs ↔ process 禁止）
规则 3: core 通过 trait 解耦（Tool trait 定义在 core，实现在 tools）
规则 4: telemetry 是旁路，不参与主依赖链
规则 5: cli/daemon 只做组装，不含业务逻辑
```

---

## 六、各 Crate 详细设计

### 6.1 `crates/common/` — 共享基础

**职责**：零业务逻辑的纯工具层，所有 crate 的最底层依赖

**目录结构**

```
src/
├── lib.rs
├── error.rs              // thiserror 统一错误类型
├── result.rs             // type Result<T> = std::result::Result<T, Error>
├── text.rs               // Unicode 宽度、ANSI strip、Bidi 处理
├── path.rs               // 跨平台路径规范化
└── id.rs                 // ULID 生成
```

**核心类型**

```rust
// error.rs — common 层基础错误（仅包含零外部依赖的变体）
// Http/Api/Mcp/Tool/Auth 等错误留在各自 crate，避免 common 引入 reqwest 等重依赖
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {message}")]
    Config { message: String },

    #[error("{0}")]
    Other(String),
}

// result.rs
pub type Result<T> = std::result::Result<T, Error>;

// text.rs
pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(strip_ansi(s).as_str())
}

pub fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    // 按显示宽度截断，处理 CJK 字符
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
    // 统一正斜杠、解析 ~、移除冗余 ..
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .expect("failed to resolve home directory")
        .home_dir()
        .to_path_buf()
}
```

**各 crate 独立错误类型示例**

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
    Common(#[from] crab_common::Error),
}

// crates/mcp/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP error: code={code}, message={message}")]
    Mcp { code: i32, message: String },

    #[error("transport error: {0}")]
    Transport(String),

    #[error(transparent)]
    Common(#[from] crab_common::Error),
}

// crates/tools/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool {name}: {message}")]
    Execution { name: String, message: String },

    #[error(transparent)]
    Common(#[from] crab_common::Error),
}

// crates/auth/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("auth error: {message}")]
    Auth { message: String },

    #[error(transparent)]
    Common(#[from] crab_common::Error),
}
```

> 每个 crate 定义自己的 `Error` + `type Result<T>`，通过 `#[from] crab_common::Error` 实现向上层转换。
> 上层 crate（如 agent）在需要统一处理时可用 `anyhow::Error` 或自定义聚合 enum。

**外部依赖**：`thiserror`, `unicode-width`, `strip-ansi-escapes`, `ulid`, `dunce`, `directories`

---

### 6.2 `crates/core/` — 领域模型

**职责**：纯数据结构 + trait 定义，不含任何 I/O 操作。定义"是什么"，不定义"怎么做"。

**目录结构**

```
src/
├── lib.rs
├── message.rs        // Message, Role, ContentBlock, ToolUse, ToolResult
├── conversation.rs   // Conversation, Turn, 上下文窗口抽象
├── tool.rs           // trait Tool { fn name(); fn execute(); fn schema(); }
├── model.rs          // ModelId, TokenUsage, CostTracker
├── permission.rs     // PermissionMode, PermissionPolicy
├── config.rs         // trait ConfigSource, 配置层级合并逻辑
├── event.rs          // 领域事件枚举（crate 间解耦）
└── capability.rs     // Agent 能力声明
```

**核心类型定义**

```rust
// message.rs — 消息模型（对标 CC src/types/message.ts）
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
// tool.rs — Tool trait（对标 CC src/Tool.ts）
// 返回 Pin<Box<dyn Future>> 而非原生 async fn，因为需要 dyn Trait 的 object safety
// （Arc<dyn Tool> 要求 trait 是 object-safe，RPITIT 的 impl Future 不满足此要求）
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::permission::PermissionMode;
use crab_common::Result;

/// 工具来源分类 — 决定权限矩阵中的列
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// 内置工具（Bash/Read/Write/Edit/Glob/Grep 等）
    BuiltIn,
    /// 外部 MCP server 提供的工具（不可信来源，Default/TrustProject 需 Prompt）
    McpExternal { server_name: String },
    /// 子 Agent 创建（AgentTool，TrustProject 信任自动放行）
    AgentSpawn,
}

pub trait Tool: Send + Sync {
    /// 工具唯一标识名
    fn name(&self) -> &str;

    /// 工具描述（用于 system prompt）
    fn description(&self) -> &str;

    /// JSON Schema 描述输入参数
    fn input_schema(&self) -> Value;

    /// 执行工具，返回结果
    /// 长时间执行的工具应通过 ctx.cancellation_token 检查取消信号
    fn execute(&self, input: Value, ctx: &ToolContext) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>>;

    /// 工具来源（默认 BuiltIn）— 影响权限检查矩阵
    fn source(&self) -> ToolSource {
        ToolSource::BuiltIn
    }

    /// 是否需要用户确认（默认 false）
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// 是否只读（只读工具可跳过确认）
    fn is_read_only(&self) -> bool {
        false
    }
}

// ─── 工具实现示例 ───
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

/// 工具执行上下文
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub permission_mode: PermissionMode,
    pub session_id: String,
    /// 取消令牌 — 长时间执行的工具（如 Bash）应定期检查并提前退出
    pub cancellation_token: CancellationToken,
    /// 权限策略（来自配置合并结果）
    pub permission_policy: crate::permission::PermissionPolicy,
}

/// 工具输出内容块 — 支持文本、图片、结构化 JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputContent {
    Text { text: String },
    Image { media_type: String, data: String },
    Json { value: Value },
}

/// 工具执行结果
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
// model.rs — 模型与 Token 追踪
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
// event.rs — 领域事件（crate 间解耦通信）
use crate::model::TokenUsage;
use crate::permission::PermissionMode;

#[derive(Debug, Clone)]
pub enum Event {
    // ─── 消息生命周期 ───
    /// 新一轮对话开始
    TurnStart { turn_index: usize },
    /// API 返回消息开始
    MessageStart,
    /// 文本增量
    ContentDelta(String),
    /// 消息结束
    MessageEnd { usage: TokenUsage },

    // ─── 工具执行 ───
    /// 工具调用开始
    ToolUseStart { id: String, name: String },
    /// 工具输入增量（流式）
    ToolUseInput(String),
    /// 工具执行结果
    ToolResult { id: String, content: String, is_error: bool },

    // ─── 权限交互 ───
    /// 请求用户确认工具执行权限
    PermissionRequest { tool_name: String, input_summary: String, request_id: String },
    /// 用户权限回复
    PermissionResponse { request_id: String, approved: bool },

    // ─── 上下文压缩 ───
    /// 开始压缩
    CompactStart { strategy: String, before_tokens: u64 },
    /// 压缩完成
    CompactEnd { after_tokens: u64, removed_messages: usize },

    // ─── Token 预警 ───
    /// token 使用率超过阈值（80%/90%/95%）
    TokenWarning { usage_percent: u8, used: u64, limit: u64 },

    // ─── 错误 ───
    Error(String),
}
```

```rust
// permission.rs — 权限模型
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// 所有工具需确认
    Default,
    /// 信任项目内文件操作
    TrustProject,
    /// 全部自动批准（危险）
    Dangerously,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub allowed_tools: Vec<String>,
    /// denied_tools 支持 glob 模式匹配（如 "mcp__*"、"bash"），
    /// 使用 globset crate 进行匹配，支持 * / ? / [abc] 等语法
    pub denied_tools: Vec<String>,
}
```

**外部依赖**：`serde`, `serde_json`, `tokio-util` (sync), `crab-common`（注意：`std::pin::Pin` / `std::future::Future` 是标准库，无额外依赖）

**Feature Flags**：无（纯类型定义）

---

### 6.3 `crates/config/` — 配置系统

**职责**：多层级配置的读写与合并（对标 CC `src/services/remoteManagedSettings/` + `src/context/` 配置部分）

**目录结构**

```
src/
├── lib.rs
├── settings.rs           // settings.json 读写、层级合并
├── crab_md.rs            // CRAB.md 解析（项目/用户/全局）
├── hooks.rs              // Hook 定义与触发
├── feature_flag.rs       // Feature Flag 集成
├── policy.rs             // 权限策略、限制
├── keybinding.rs         // 快捷键配置
├── config_toml.rs        // config.toml 多提供商配置格式
├── hot_reload.rs         // settings.json 热重载（notify 监听）
└── permissions.rs        // 权限决策统一入口
```

**配置层级（三级合并，低优先级 → 高优先级）**

```
1. 全局默认   ~/.config/crab-code/settings.json
2. 用户覆盖   ~/.crab-code/settings.json
3. 项目覆盖   .crab-code/settings.json
```

**核心类型**

`Settings` 结构体涵盖：`api_provider`、`api_base_url`、`api_key`、`model`、`small_model`、`permission_mode`、`system_prompt`、`mcp_servers`、`hooks`、`theme` 等字段。三级配置通过 `load_merged_settings()` 逐级合并（全局 → 用户 → 项目），高优先级字段覆盖低优先级。

```rust
// crab_md.rs — CRAB.md 解析
pub struct CrabMd {
    pub content: String,
    pub source: CrabMdSource,
}

pub enum CrabMdSource {
    Global,   // ~/.crab/CRAB.md
    User,     // 用户目录
    Project,  // 项目根目录
}

/// 按优先级收集所有 CRAB.md 内容
pub fn collect_crab_md(project_dir: &std::path::Path) -> Vec<CrabMd> {
    // 全局 → 用户 → 项目，逐级叠加
    // ...
}
```

**外部依赖**：`serde`, `serde_json`, `jsonc-parser`, `directories`, `crab-core`, `crab-common`

**Feature Flags**：无

---

### 6.4 `crates/auth/` — 认证

**职责**：所有认证方式的统一管理（对标 CC `src/services/oauth/` + 认证相关代码）

**目录结构**

```
src/
├── lib.rs
├── oauth.rs              // OAuth2 PKCE 流程
├── keychain.rs           // 系统 Keychain（macOS/Windows/Linux）
├── api_key.rs            // API Key 管理（环境变量 / 文件）
├── bedrock_auth.rs       // AWS SigV4 签名 (feature = "bedrock")
├── vertex_auth.rs        // GCP Vertex AI 认证
├── aws_iam.rs            // AWS IAM Roles + IRSA（Pod 级别）
├── gcp_identity.rs       // GCP Workload Identity Federation
└── credential_chain.rs   // 凭证链（优先级顺序探测：env → keychain → file → IAM）
```

**核心接口**

```rust
// lib.rs — 统一认证接口
pub enum AuthMethod {
    ApiKey(String),
    OAuth(OAuthToken),
    Bedrock(BedrockCredentials),
}

/// 认证提供者 trait
/// 返回 Pin<Box<dyn Future>> 而非原生 async fn，因为需要 dyn Trait 的 object safety
/// （Box<dyn AuthProvider> 要求 trait 是 object-safe，RPITIT 的 impl Future 不满足此要求）
/// 实现内部通过 tokio::sync::RwLock 保护 token 缓存，
/// get_auth() 读锁热路径，refresh() 写锁刷新
pub trait AuthProvider: Send + Sync {
    /// 获取当前有效的认证信息（读锁，通常 <1μs）
    fn get_auth(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<AuthMethod>> + Send + '_>>;
    /// 刷新认证（如 OAuth token 过期）— 可能触发网络请求
    fn refresh(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>>;
}

// api_key.rs
pub fn resolve_api_key() -> Option<String> {
    // 优先级: 环境变量 → keychain → 配置文件
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .or_else(|| keychain::get("crab-code", "api-key").ok())
}

// keychain.rs — 使用 auth crate 本地的 AuthError，而非 crab_common::Error
// （common 层不包含 Auth 变体，Auth 错误定义在 crates/auth/src/error.rs）
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

**外部依赖**：`keyring`, `oauth2`, `reqwest`, `crab-config`, `crab-common`

**Feature Flags**

```toml
[features]
default = []
bedrock = ["aws-sdk-bedrockruntime", "aws-config"]
```

---

### 6.5 `crates/api/` — LLM API 客户端

**职责**：封装所有 LLM API 通信，两个独立 client 分别实现两大 API 标准（对标 CC `src/services/api/`）

**核心设计**：不使用统一 trait 抽象——Anthropic Messages API 和 OpenAI Chat Completions API
差异过大（消息格式、流式事件粒度、工具调用协议），强行统一会产生"最小公约数"陷阱，
丢失 provider 特有能力（Anthropic 的 prompt cache / extended thinking，OpenAI 的 logprobs / structured output）。

采用**两个完全独立的 client + 枚举派发**：
- `anthropic/` — 完整的 Anthropic Messages API client，有自己的类型、SSE 解析、认证
- `openai/` — 完整的 OpenAI Chat Completions client，覆盖所有兼容端点（Ollama/DeepSeek/vLLM/Gemini 等）
- `LlmBackend` 枚举 — 编译时确定，零动态派发，穷举 match 保证不遗漏

agent/session 层通过 `LlmBackend` 枚举交互，内部统一的 `MessageRequest` / `StreamEvent`
是 Crab Code 自己的数据模型，不是 API 抽象。各 client 内部独立完成格式转换。

**目录结构**

```
src/
├── lib.rs                // LlmBackend 枚举 + create_backend()
├── types.rs              // 内部统一的请求/响应/事件类型（Crab Code 自有格式）
├── anthropic/            // 完整独立的 Anthropic Messages API client
│   ├── mod.rs
│   ├── client.rs         // HTTP + SSE + retry
│   ├── types.rs          // Anthropic API 原生请求/响应类型
│   └── convert.rs        // Anthropic 类型 ↔ 内部类型
├── openai/               // 完整独立的 OpenAI Chat Completions client
│   ├── mod.rs
│   ├── client.rs         // HTTP + SSE + retry
│   ├── types.rs          // OpenAI API 原生请求/响应类型
│   └── convert.rs        // OpenAI 类型 ↔ 内部类型
├── bedrock.rs            // AWS Bedrock 适配 (feature = "bedrock"，包装 anthropic client)
├── vertex.rs             // Google Vertex 适配 (feature = "vertex"，包装 anthropic client)
├── rate_limit.rs         // 共享的速率限制、指数退避
├── cache.rs              // Prompt cache 管理 (仅 Anthropic 路径使用)
├── error.rs
├── streaming.rs          // 流式工具调用解析（部分工具参数流式输入）
├── fallback.rs           // 多模型回退链（主模型失败→备用模型）
├── capabilities.rs       // 模型能力协商与发现
├── context_optimizer.rs  // 上下文窗口优化 + 智能截断策略
├── retry_strategy.rs     // 增强重试策略（退避 + jitter）
└── error_classifier.rs   // 错误分类（可重试/不可重试/速率限制）
```

**核心接口**

```rust
// types.rs — Crab Code 内部统一类型（不是 API 抽象，是自有数据模型）
use crab_core::message::Message;
use crab_core::model::{ModelId, TokenUsage};

/// 内部消息请求 — 各 client 内部转为自己的 API 格式
#[derive(Debug, Clone)]
pub struct MessageRequest<'a> {
    pub model: ModelId,
    pub messages: std::borrow::Cow<'a, [Message]>,
    pub system: Option<String>,
    pub max_tokens: u32,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
}

/// 内部统一流式事件 — 各 client 将自己的 SSE 格式映射到此枚举
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
// lib.rs — 枚举派发（不用 dyn trait，编译时确定，零动态派发开销）
use futures::stream::{self, Stream, StreamExt};
use either::Either;

/// LLM 后端枚举 — provider 数量有限（2 个标准 + 2 个云变体），枚举完全够用
/// 如果将来需要第三方扩展 provider，可在 Phase 2 通过 WASM 插件系统支持
pub enum LlmBackend {
    Anthropic(anthropic::AnthropicClient),
    OpenAi(openai::OpenAiClient),
    // Bedrock 和 Vertex 本质是 Anthropic API 的不同入口，包装 AnthropicClient
    #[cfg(feature = "bedrock")]
    Bedrock(anthropic::AnthropicClient),  // 不同 auth + base_url
    #[cfg(feature = "vertex")]
    Vertex(anthropic::AnthropicClient),   // 不同 auth + base_url
}

impl LlmBackend {
    /// 流式发送消息
    pub fn stream_message<'a>(
        &'a self,
        req: types::MessageRequest<'a>,
    ) -> impl Stream<Item = crab_common::Result<types::StreamEvent>> + Send + 'a {
        match self {
            Self::Anthropic(c) => Either::Left(c.stream(req)),
            Self::OpenAi(c) => Either::Right(c.stream(req)),
            // Bedrock/Vertex 走 Anthropic 路径
        }
    }

    /// 非流式发送（用于压缩等轻量任务）
    pub async fn send_message(
        &self,
        req: types::MessageRequest<'_>,
    ) -> crab_common::Result<(crab_core::message::Message, crab_core::model::TokenUsage)> {
        match self {
            Self::Anthropic(c) => c.send(req).await,
            Self::OpenAi(c) => c.send(req).await,
        }
    }

    /// Provider 名称
    pub fn name(&self) -> &str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::OpenAi(_) => "openai",
        }
    }
}

/// 根据配置构造后端
pub fn create_backend(settings: &crab_config::Settings) -> LlmBackend {
    match settings.api_provider.as_deref() {
        Some("openai") | Some("ollama") | Some("deepseek") => {
            let base_url = settings.api_base_url.as_deref()
                .unwrap_or("https://api.openai.com/v1");
            let api_key = std::env::var("OPENAI_API_KEY").ok()
                .or_else(|| settings.api_key.clone());
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
// anthropic/client.rs — Anthropic Messages API (完整独立实现)
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

    /// 流式调用 — POST /v1/messages, stream: true
    pub fn stream<'a>(
        &'a self,
        req: crate::types::MessageRequest<'a>,
    ) -> impl Stream<Item = crab_common::Result<crate::types::StreamEvent>> + Send + 'a {
        // 1. MessageRequest → Anthropic 原生请求 (self::types::AnthropicRequest)
        // 2. POST /v1/messages, 设置 stream: true
        // 3. 解析 Anthropic SSE: message_start / content_block_delta / message_stop
        // 4. self::convert::to_stream_event() 映射为内部 StreamEvent
        // ...
    }

    /// 非流式调用
    pub async fn send(
        &self,
        req: crate::types::MessageRequest<'_>,
    ) -> crab_common::Result<(crab_core::message::Message, crab_core::model::TokenUsage)> {
        // ...
    }
}
```

```rust
// openai/client.rs — OpenAI Chat Completions API (完整独立实现)
//
// 覆盖所有兼容 /v1/chat/completions 的后端:
// OpenAI、Ollama、DeepSeek、vLLM、TGI、LiteLLM、Azure OpenAI、Google Gemini (OpenAI 兼容端点)
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

    /// 流式调用 — POST /v1/chat/completions, stream: true
    pub fn stream<'a>(
        &'a self,
        req: crate::types::MessageRequest<'a>,
    ) -> impl Stream<Item = crab_common::Result<crate::types::StreamEvent>> + Send + 'a {
        // 1. MessageRequest → OpenAI 原生请求 (self::types::ChatCompletionRequest)
        //    - system prompt → messages[0].role="system"
        //    - ContentBlock::ToolUse → tool_calls 数组
        //    - ContentBlock::ToolResult → role="tool" 消息
        // 2. POST /v1/chat/completions, stream: true
        // 3. 解析 OpenAI SSE: data: {"choices":[{"delta":...}]}
        // 4. self::convert::to_stream_event() 映射为内部 StreamEvent
        // ...
    }

    /// 非流式调用
    pub async fn send(
        &self,
        req: crate::types::MessageRequest<'_>,
    ) -> crab_common::Result<(crab_core::message::Message, crab_core::model::TokenUsage)> {
        // ...
    }
}
```

**两大 API 标准核心差异**（各 client 内部 `convert.rs` 处理，不外泄给上层）

| 维度 | Anthropic Messages API | OpenAI Chat Completions API |
|------|----------------------|---------------------------|
| system prompt | 独立 `system` 字段 | `messages[0].role="system"` |
| 消息内容 | `content: Vec<ContentBlock>` | `content: string` |
| 工具调用 | `ContentBlock::ToolUse` | `tool_calls` 数组 |
| 工具结果 | `ContentBlock::ToolResult` | `role="tool"` 消息 |
| 流式格式 | `content_block_delta` 事件 | `choices[].delta` |
| token 统计 | `input_tokens` / `output_tokens` | `prompt_tokens` / `completion_tokens` |
| 特有能力 | prompt cache, extended thinking | logprobs, structured output |

```rust
// rate_limit.rs — 共享的速率限制与退避
use std::time::Duration;

pub struct RateLimiter {
    pub remaining_requests: u32,
    pub remaining_tokens: u32,
    pub reset_at: std::time::Instant,
}

/// 指数退避策略
pub fn backoff_delay(attempt: u32) -> Duration {
    let base = Duration::from_millis(500);
    let max = Duration::from_secs(30);
    let delay = base * 2u32.pow(attempt.min(6));
    delay.min(max)
}
```

**外部依赖**：`reqwest`, `tokio`, `serde`, `eventsource-stream`, `futures`, `either`, `crab-core`, `crab-auth`, `crab-common`

**Feature Flags**

```toml
[features]
default = []
bedrock = ["aws-sdk-bedrockruntime", "aws-config"]
vertex = ["gcp-auth"]
proxy = ["reqwest/socks"]
```

---

### 6.6 `crates/mcp/` — MCP façade

**职责**：Crab 自己的 MCP façade 与协议适配层（对标 CC `src/services/mcp/`）

MCP 是让 LLM 连接外部工具/资源的开放协议，基于 JSON-RPC 2.0。  
`crab-mcp` 不把底层 SDK 直接暴露给 `cli` / `tools` / `session`，而是在 crate 内部吸收官方 SDK，实现稳定的 Crab 侧接口：`McpClient`、`McpManager`、`ToolRegistryHandler`、`mcp__<server>__<tool>` 命名与配置发现逻辑都留在这一层。

**目录结构**

```
src/
├── lib.rs
├── protocol.rs             // Crab 自己的 MCP façade 类型
├── client.rs               // MCP 客户端 façade（内部可委托 rmcp）
├── server.rs               // MCP 服务端 façade（暴露自身工具给外部）
├── manager.rs              // 生命周期管理，多 server 协调
├── transport/
│   ├── mod.rs              // 兼容用 Transport trait / 本地传输抽象
│   ├── stdio.rs            // legacy stdin/stdout 传输
│   └── ws.rs               // WebSocket 传输 (feature = "ws")
├── resource.rs             // Resource 缓存、模板
├── discovery.rs            // Server 自动发现
├── sse_server.rs           // SSE server transport（crab 作为 MCP server）
├── sampling.rs             // MCP sampling（服务端请求 LLM 推理）
├── roots.rs                // MCP roots（工作区根目录声明）
├── logging.rs              // MCP logging 协议（结构化日志消息）
├── handshake.rs            // 初始化握手流程（initialize/initialized）
├── negotiation.rs          // 能力协商（客户端/服务端能力集合）
├── capability.rs           // 能力声明类型（resources/tools/prompts/sampling）
├── notification.rs         // 服务端通知推送（工具变更/资源更新）
├── progress.rs             // 进度报告（长时间工具执行）
├── cancellation.rs         // 请求取消机制（$/cancelRequest）
└── health.rs               // 健康检查 + 心跳（自动重连）
```

**边界原则**

- `crab-mcp` 对外暴露的是 Crab 自己的 façade 类型，不是 `rmcp` 原生类型
- client 侧 stdio / HTTP 连接优先复用官方 `rmcp`
- 配置层只保留 `stdio` / `http` / `ws` 三种 transport
- 上层 crate 只依赖 `crab_mcp::*`，不直接依赖底层 MCP SDK
- `protocol.rs` 继续承载 Crab 侧稳定数据结构，避免 SDK 类型扩散

**核心类型**

```rust
// protocol.rs — JSON-RPC 2.0 消息
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

/// MCP 工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// MCP 资源定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}
```

```rust
// transport/mod.rs — 传输抽象
// 返回 Pin<Box<dyn Future>> 而非原生 async fn，因为需要 dyn Trait 的 object safety
// （Box<dyn Transport> 要求 trait 是 object-safe，RPITIT 的 impl Future 不满足此要求）
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use std::future::Future;
use std::pin::Pin;

pub trait Transport: Send + Sync {
    /// 发送请求，等待响应
    fn send(&self, req: JsonRpcRequest) -> Pin<Box<dyn Future<Output = crab_common::Result<JsonRpcResponse>> + Send + '_>>;
    /// 发送通知（无需响应）
    fn notify(&self, method: &str, params: serde_json::Value) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>>;
    /// 关闭传输
    fn close(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>>;
}

// ─── 传输实现示例 ───
// impl Transport for StdioTransport {
//     fn send(&self, req: JsonRpcRequest) -> Pin<Box<dyn Future<Output = crab_common::Result<JsonRpcResponse>> + Send + '_>> {
//         Box::pin(async move {
//             self.write_message(&req).await?;
//             self.read_response().await
//         })
//     }
//     // ... notify, close 同理
// }
```

```rust
// client.rs — MCP 客户端 façade
use crate::protocol::{McpToolDef, ServerCapabilities, ServerInfo};

pub struct McpClient {
    server_name: String,
    server_info: ServerInfo,
    capabilities: ServerCapabilities,
    tools: Vec<McpToolDef>,
}

impl McpClient {
    /// 通过官方 SDK 连接 stdio MCP server
    pub async fn connect_stdio(...) -> crab_common::Result<Self> { /* ... */ }

    /// 通过官方 SDK 连接 HTTP MCP endpoint
    pub async fn connect_streamable_http(...) -> crab_common::Result<Self> { /* ... */ }

    /// 调用 MCP 工具
    pub async fn call_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> crab_common::Result<serde_json::Value> {
        // ...
    }

    /// 读取 MCP 资源
    pub async fn read_resource(&self, uri: &str) -> crab_common::Result<String> {
        // ...
    }

    pub fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }
}
```

**外部依赖**：`tokio`, `serde`, `serde_json`, `rmcp`, `crab-core`, `crab-common`

**Feature Flags**

```toml
[features]
default = []
ws = ["tokio-tungstenite"]
```

---

### 6.7 `crates/fs/` — 文件系统操作

**职责**：所有文件系统相关操作的封装（对标 CC 中 GlobTool/GrepTool/FileReadTool 底层逻辑）

**目录结构**

```
src/
├── lib.rs
├── glob.rs               // globset 封装
├── grep.rs               // ripgrep 内核集成
├── gitignore.rs          // .gitignore 规则解析与过滤
├── watch.rs              // notify 文件监听（含防抖 + 批量聚合）
├── lock.rs               // 文件锁 (fd-lock)
├── diff.rs               // similar 封装，edit/patch 生成
└── symlink.rs            // 符号链接处理 + 安全路径解析（防逃逸）
```

**核心接口**

```rust
// glob.rs — 文件模式匹配
use std::path::{Path, PathBuf};

pub struct GlobResult {
    pub matches: Vec<PathBuf>,
    pub truncated: bool,
}

/// 在目录中按 glob 模式搜索文件
pub fn find_files(
    root: &Path,
    pattern: &str,
    limit: usize,
) -> crab_common::Result<GlobResult> {
    // 使用 ignore crate（自动尊重 .gitignore）
    // 按修改时间排序
    // ...
}

// grep.rs — 内容搜索
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

/// 在目录中按正则搜索内容
pub fn search(opts: &GrepOptions) -> crab_common::Result<Vec<GrepMatch>> {
    // 使用 grep-regex + grep-searcher
    // 自动尊重 .gitignore
    // ...
}

// diff.rs — Diff 生成
pub struct EditResult {
    pub old_content: String,
    pub new_content: String,
    pub unified_diff: String,
}

/// 基于 old_string → new_string 的精确替换
pub fn apply_edit(
    file_content: &str,
    old_string: &str,
    new_string: &str,
) -> crab_common::Result<EditResult> {
    // 使用 similar 生成 unified diff
    // ...
}
```

**外部依赖**：`globset`, `grep-regex`, `grep-searcher`, `ignore`, `notify`, `similar`, `fd-lock`, `crab-common`

**Feature Flags**：无

---

### 6.8 `crates/process/` — 子进程管理

**职责**：子进程生命周期管理（对标 CC BashTool 底层执行逻辑）

**目录结构**

```
src/
├── lib.rs
├── spawn.rs              // 子进程启动、环境继承
├── pty.rs                // 伪终端分配 (feature = "pty")
├── tree.rs               // 进程树 kill (sysinfo)
├── signal.rs             // 信号处理、优雅关闭
└── sandbox.rs            // 沙箱策略 (feature = "sandbox")
```

**核心接口**

```rust
// spawn.rs — 子进程执行
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

/// 执行命令并等待结果
pub async fn run(opts: SpawnOptions) -> crab_common::Result<SpawnOutput> {
    use tokio::process::Command;
    // 1. 构造 Command
    // 2. 设置 working_dir, env
    // 3. 如有 timeout 则 tokio::time::timeout 包裹
    // 4. 收集 stdout/stderr
    // ...
}

/// 执行命令并流式返回输出
pub async fn run_streaming(
    opts: SpawnOptions,
    on_stdout: impl Fn(&str) + Send,
    on_stderr: impl Fn(&str) + Send,
) -> crab_common::Result<i32> {
    // ...
}

// tree.rs — 进程树管理
/// 杀死进程及其所有子进程
pub fn kill_tree(pid: u32) -> crab_common::Result<()> {
    use sysinfo::{Pid, System};
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    // 递归查找子进程并 kill
    // ...
}

// signal.rs — 信号处理
/// 注册 Ctrl+C / SIGTERM 处理
pub fn register_shutdown_handler(
    on_shutdown: impl Fn() + Send + 'static,
) {
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        on_shutdown();
    });
}
```

**外部依赖**：`tokio` (process, signal), `sysinfo`, `crab-common`

**Feature Flags**

```toml
[features]
default = []
pty = ["portable-pty"]
sandbox = []
```

---

### 6.9 `crates/tools/` — 工具系统

**职责**：工具注册、查找、执行，包含所有内置工具（对标 CC `src/tools/`）

**目录结构**

```
src/
├── lib.rs
├── registry.rs       // ToolRegistry: 注册、查找、schema 生成
├── executor.rs       // 带权限检查的统一执行器
├── permission.rs     // 工具权限检查逻辑
│
├── builtin/          // 内置工具
│   ├── mod.rs        // register_all_builtins()
│   ├── bash.rs       // BashTool — shell 命令执行
│   ├── read.rs       // ReadTool — 文件读取
│   ├── edit.rs       // EditTool — diff-based 文件编辑
│   ├── write.rs      // WriteTool — 文件创建/覆写
│   ├── glob.rs       // GlobTool — 文件模式匹配
│   ├── grep.rs       // GrepTool — 内容搜索
│   ├── web_search.rs // WebSearchTool — 网络搜索
│   ├── web_fetch.rs  // WebFetchTool — 网页抓取
│   ├── agent.rs      // AgentTool — 子 Agent 启动
│   ├── notebook.rs   // NotebookTool — Jupyter 支持
│   ├── task.rs       // TaskCreate/Get/List/Update/Stop/Output
│   ├── mcp_tool.rs   // MCP 工具的 Tool trait 适配器
│   ├── lsp.rs        // LSP 集成工具
│   ├── worktree.rs   // Git Worktree 工具
│   ├── ask_user.rs   // 用户交互工具
│   ├── image_read.rs // 图片读取工具
│   ├── read_enhanced.rs // 增强文件读取
│   ├── bash_security.rs // Bash 安全检查
│   ├── plan_mode.rs  // 计划模式工具
│   ├── plan_file.rs  // 计划文件操作
│   ├── plan_approval.rs // 计划审批工具
│   ├── web_cache.rs  // 网页缓存
│   └── web_formatter.rs // 网页格式化
│
└── schema.rs         // 工具 schema → API tools 参数转换
```

**核心类型**

```rust
// registry.rs — 工具注册表
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

    /// 注册工具
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// 按名称查找
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// 获取所有工具的 JSON Schema（用于 API 请求）
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

    /// 列出所有工具名
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}
```

```rust
// executor.rs — 统一执行器
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

    /// 执行工具（含权限检查）
    ///
    /// **权限决策矩阵** (mode x tool_type x path_scope):
    ///
    /// | PermissionMode | read_only | write(项目内) | write(项目外) | dangerous | mcp_external | agent_spawn | denied_list |
    /// |----------------|-----------|--------------|--------------|-----------|-------------|-------------|-------------|
    /// | Default        | Allow     | **Prompt**   | **Prompt**   | **Prompt**| **Prompt**  | **Prompt**  | **Deny**    |
    /// | TrustProject   | Allow     | Allow        | **Prompt**   | **Prompt**| **Prompt**  | Allow       | **Deny**    |
    /// | Dangerously    | Allow     | Allow        | Allow        | Allow     | Allow       | Allow       | **Deny**    |
    ///
    /// - denied_list 任何模式下都拒绝（来自 settings.json `deniedTools`）
    /// - allowed_list 匹配则跳过普通 Prompt（但不免除 dangerous 检测）
    /// - dangerous = BashTool 含 `rm -rf`/`sudo`/`curl|sh`/`chmod`/`eval` 等高危模式
    /// - mcp_external: 外部 MCP server 提供的工具，Default/TrustProject 都需 Prompt（不可信来源）
    /// - agent_spawn: 子 Agent 创建，TrustProject 信任自动放行；子 Agent 继承父 Agent 的 permission_mode
    pub async fn execute(
        &self,
        tool_name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> crab_common::Result<ToolOutput> {
        let tool = self
            .registry
            .get(tool_name)
            .ok_or_else(|| crab_common::Error::Other(
                format!("tool not found: {tool_name}"),
            ))?;

        // 1. 检查 denied list — 任何模式下都拒绝
        //    denied_tools 支持 glob 匹配（如 "mcp__*"、"bash"）
        //    使用 globset 进行模式匹配，支持 * / ? / [abc] 等 glob 语法
        if ctx.permission_policy.denied_tools.iter().any(|pattern| {
            globset::Glob::new(pattern)
                .ok()
                .and_then(|g| g.compile_matcher().is_match(tool_name).then_some(()))
                .is_some()
        }) {
            return Ok(ToolOutput::error(format!("tool '{tool_name}' is denied by policy")));
        }

        // 2. Dangerously 模式短路 — 跳过所有权限检查（含 allowed_tools 和 dangerous 检测）
        //    放在 denied_tools 之后：即使 Dangerously 模式，denied_tools 仍然生效
        if ctx.permission_mode == PermissionMode::Dangerously {
            return tool.execute(input, ctx).await;
        }

        // 3. 检查 allowed list — 显式允许跳过 prompt
        let explicitly_allowed = ctx.permission_policy.allowed_tools.contains(&tool_name.to_string());

        // 4. 按矩阵决策（综合 tool.source() + mode + path_scope）
        // allowed_tools 只免除普通 Prompt，不免除 dangerous 检测
        let needs_prompt = if explicitly_allowed {
            self.is_dangerous_command(&input) // allowed_tools 只免除普通 Prompt，不免除 dangerous 检测
        } else {
            match tool.source() {
                // MCP 外部工具：Default/TrustProject 都需 Prompt（不可信来源）
                ToolSource::McpExternal { .. } => true,
                // 子 Agent 创建：TrustProject 信任放行，Default 需 Prompt
                ToolSource::AgentSpawn => {
                    ctx.permission_mode == PermissionMode::Default
                }
                // 内置工具：按原有矩阵
                ToolSource::BuiltIn => {
                    match ctx.permission_mode {
                        PermissionMode::Dangerously => unreachable!(), // 已在上方短路
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
            // 通过 event channel 请求用户确认
            let approved = self.request_permission(tool_name, &input, ctx).await?;
            if !approved {
                return Ok(ToolOutput::error("user denied permission"));
            }
        }

        tool.execute(input, ctx).await
    }

    /// 检查工具操作路径是否在项目目录内
    ///
    /// **TOCTOU + symlink 防护**：
    /// - 使用 `std::fs::canonicalize()` 解析符号链接后再比较，防止 symlink 绕过
    /// - 文件操作应使用 `O_NOFOLLOW`（或 Rust 等价方式）防止 TOCTOU 竞态
    /// - 注意：canonicalize 只能在路径存在时使用，不存在的路径需要 canonicalize 父目录
    fn is_path_in_project(&self, tool_name: &str, input: &serde_json::Value, project_dir: &std::path::Path) -> bool {
        // BashTool 特殊处理：input 包含 "command" 而非 "file_path"
        // 需要从命令字符串中解析出可能的路径引用
        if tool_name == "bash" {
            return self.bash_paths_in_project(input, project_dir);
        }

        // 其他工具：从 input 中提取 file_path/path 字段
        input.get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .map(|p| {
                let raw = std::path::Path::new(p);
                // 先 canonicalize 解析 symlink，防止 symlink 绕过项目边界
                // 路径不存在时 fallback: canonicalize 最近存在的祖先目录 + 剩余相对段
                let resolved = std::fs::canonicalize(raw).unwrap_or_else(|_| {
                    // 路径尚不存在（如即将创建的新文件），向上找到存在的祖先目录
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
            .unwrap_or(true) // 无路径参数的工具默认视为项目内
    }

    /// BashTool 路径检测：从 command 字符串中提取绝对路径并检查
    /// 在 TrustProject 模式下，引用项目外绝对路径的命令需要 Prompt
    ///
    /// **重要：这是 best-effort 启发式检测**
    /// shell 命令的路径提取无法做到 100% 准确（变量展开、子 shell、引号嵌套等）。
    /// 保守策略：当路径分析不确定时，返回 Uncertain → 映射到 Prompt。
    /// 具体场景：
    /// - 无法提取任何路径 token → Uncertain（可能有变量/子 shell 引用路径）
    /// - 包含 shell 元字符（$, `, $(...)）→ Uncertain（路径可能被动态构造）
    ///
    /// **核心原则：无法可靠解析时默认需要 Prompt，宁可多问不可漏放。**
    fn bash_paths_in_project(&self, input: &serde_json::Value, project_dir: &std::path::Path) -> bool {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // 保守策略：包含 shell 元字符时无法可靠提取路径，返回 false（需 Prompt）
        let shell_metacharacters = ['$', '`'];
        if cmd.chars().any(|c| shell_metacharacters.contains(&c)) || cmd.contains("$(") {
            return false; // Uncertain → 映射到 Prompt
        }

        // cd 到绝对路径会改变后续命令的工作目录，视同路径引用
        // 例如 `cd /etc && cat passwd` 实际操作项目外文件
        if cmd.starts_with("cd ") || cmd.contains("&& cd ") || cmd.contains("; cd ") || cmd.contains("|| cd ") {
            // 提取 cd 目标路径，检查是否在项目内
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
                            return false; // cd 到项目外 → Prompt
                        }
                    }
                }
            }
        }

        // 提取命令中所有绝对路径 token
        let abs_paths: Vec<&str> = cmd.split_whitespace()
            .filter(|token| token.starts_with('/') || token.starts_with("~/"))
            .collect();

        // 保守策略：无法提取路径时返回 false（Uncertain → Prompt）
        // 注：纯相对路径命令（如 `cargo build`）不含 / 前缀，会走到这里
        // 但这些命令通常是安全的项目内操作，所以仍返回 true
        if abs_paths.is_empty() {
            return true;
        }

        // 任一绝对路径在项目外 → 返回 false（需要 Prompt）
        abs_paths.iter().all(|p| {
            let expanded = if p.starts_with("~/") {
                crab_common::path::home_dir().join(&p[2..])
            } else {
                std::path::PathBuf::from(p)
            };
            expanded.starts_with(project_dir)
        })
    }

    /// 检测高危命令模式
    /// 覆盖：破坏性操作、提权、远程代码执行、文件覆写、链式危险命令
    ///
    /// **重要：所有模式匹配必须使用 shell tokenizer 排除引号内容**
    /// 下方所有 `cmd.contains(pattern)` 在实际实现时应替换为 tokenize-then-match：
    /// 1. 使用 `shell-words` crate（或等价 tokenizer）将 cmd 拆分为 token
    /// 2. 仅在非引号 token 中匹配危险模式
    /// 3. 示例：`echo "rm -rf /" > log.txt` 不应触发 `rm -rf` 检测（在引号内）
    ///    但 `> log.txt` 重定向在引号外，应正常检测
    /// 4. tokenizer 失败（如引号未闭合）时保守处理 → 视为 dangerous
    fn is_dangerous_command(&self, input: &serde_json::Value) -> bool {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // 1. 直接高危模式
        // 二级策略：Level 1 精确匹配（下方列表）+ Level 2 启发式检测（解释器 + -c/-e 组合）
        let dangerous_patterns = [
            // ── 破坏性文件操作 ──
            "rm -rf", "rm -fr",
            // ── 提权 ──
            "sudo ",
            // ── 磁盘/设备操作 ──
            "mkfs", "dd if=", "> /dev/",
            // ── 远程代码执行 (pipe to shell) ──
            "curl|sh", "curl|bash", "wget|sh", "wget|bash",
            "curl | sh", "curl | bash", "wget | sh", "wget | bash",
            // ── 权限修改 ──
            "chmod ", "chown ",
            // ── 动态执行（可绕过静态检测）──
            "eval ", "exec ", "source ",
            // ── 解释器内联执行（Level 1: 精确匹配 interpreter + -c/-e）──
            "python -c", "python3 -c", "perl -e", "node -e", "ruby -e",
            // ── 危险批量操作 ──
            "xargs ",      // xargs + 危险目标（如 xargs rm）
            "crontab",     // 定时任务修改
            "nohup ",      // 后台持久化执行
            // ── 文件覆写重定向 ──
            // （引号排除逻辑由函数级 tokenizer 统一处理，见函数头部注释）
            "> ",   // 覆写重定向
            ">> ",  // 追加重定向（写入敏感文件如 .bashrc）
        ];

        // Level 2 启发式：`find` + `-exec` 组合检测
        if cmd.contains("find ") && (cmd.contains("-exec") || cmd.contains("-execdir")) {
            return true;
        }

        // 2. 检查直接模式
        if dangerous_patterns.iter().any(|p| cmd.contains(p)) {
            return true;
        }

        // 3. 检查 pipe 到危险命令（如 `cat file | sudo tee`, `echo x | sh`）
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

        // 4. 检查 && / || 链中包含危险命令
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

**CC 工具对照表（CC 有 52 工具，以下为核心映射）**

| CC 工具 | Crab 工具 | 文件 |
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

**外部依赖**：`crab-core`, `crab-fs`, `crab-process`, `crab-mcp`, `crab-config`, `crab-common`

**Feature Flags**：无

---

### 6.10 `crates/session/` — 会话管理

**职责**：多轮对话的状态管理（对标 CC `src/services/compact/` + `src/services/SessionMemory/` + `src/services/sessionTranscript/`）

**目录结构**

```
src/
├── lib.rs
├── conversation.rs   // 对话状态机，多轮管理
├── context.rs        // 上下文窗口管理、自动压缩触发
├── compaction.rs     // 消息压缩策略（5 级：Snip/Microcompact/Summarize/Hybrid/Truncate）
├── history.rs        // 会话持久化、恢复、搜索、导出、统计
├── memory.rs         // 记忆系统（文件持久化）
├── cost.rs           // token 计数、费用追踪
└── template.rs       // 会话模板 + 快速恢复
```

**核心类型**

```rust
// conversation.rs — 对话状态机
use crab_core::message::Message;
use crab_core::model::TokenUsage;

pub struct Conversation {
    /// 会话 ID
    pub id: String,
    /// 系统提示
    pub system_prompt: String,
    /// 消息历史
    pub messages: Vec<Message>,
    /// 累计 token 使用量
    pub total_usage: TokenUsage,
    /// 上下文窗口上限
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

    /// 追加消息
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// 估算当前 token 数
    ///
    /// **当前**: text_len/4 粗略估算（误差 ±30%），适合 MVP 阶段快速实现
    /// **后续**: 接入 tiktoken-rs 精确计数（Claude tokenizer 兼容 cl100k_base）
    ///
    /// ```rust
    /// // TODO(M2+): 替换为精确计数
    /// // use tiktoken_rs::cl100k_base;
    /// // let bpe = cl100k_base().unwrap();
    /// // bpe.encode_with_special_tokens(text).len() as u64
    /// ```
    pub fn estimated_tokens(&self) -> u64 {
        let text_len: usize = self.messages.iter().map(|m| {
            m.content.iter().map(|c| match c {
                crab_core::message::ContentBlock::Text { text } => text.len(),
                _ => 100, // 工具调用按固定估算
            }).sum::<usize>()
        }).sum();
        (text_len / 4) as u64 // 临时方案：±30% 误差
    }

    /// 是否需要压缩
    pub fn needs_compaction(&self) -> bool {
        self.estimated_tokens() > self.context_window * 80 / 100
    }
}

// compaction.rs — 5 层压缩策略（按 token 使用率递进触发）
pub enum CompactionStrategy {
    /// Level 1 (70-80%): 裁剪旧工具调用的完整输出，仅保留摘要行
    Snip,
    /// Level 2 (80-85%): 大结果（>500 token）替换为 AI 生成的单行摘要
    Microcompact,
    /// Level 3 (85-90%): 用小模型总结旧消息
    Summarize,
    /// Level 4 (90-95%): 保留最近 N 轮 + 总结其余
    Hybrid { keep_recent: usize },
    /// Level 5 (>95%): 紧急截断，丢弃最早的消息
    Truncate,
}

use std::future::Future;
use std::pin::Pin;

/// 压缩客户端抽象 — 解耦 compaction 逻辑与具体 API 客户端
/// 便于测试（mock）和替换不同 LLM provider
pub trait CompactionClient: Send + Sync {
    /// 发送压缩/摘要请求，返回摘要文本
    fn summarize(
        &self,
        messages: &[crab_core::message::Message],
        instruction: &str,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<String>> + Send + '_>>;
}

// LlmBackend 通过枚举派发适配为 CompactionClient（在 crab-api 中）
// impl CompactionClient for LlmBackend { ... }

pub async fn compact(
    conversation: &mut Conversation,
    strategy: CompactionStrategy,
    client: &impl CompactionClient,
) -> crab_common::Result<()> {
    // 根据策略压缩消息，使用 client.summarize() 生成摘要
    // ...
}

// memory.rs — 记忆系统
pub struct MemoryStore {
    pub path: std::path::PathBuf, // ~/.crab-code/memory/
}

impl MemoryStore {
    /// 保存会话记忆
    pub fn save(&self, session_id: &str, content: &str) -> crab_common::Result<()> {
        // ...
    }

    /// 加载会话记忆
    pub fn load(&self, session_id: &str) -> crab_common::Result<Option<String>> {
        // ...
    }
}
```

**外部依赖**：`crab-core`, `crab-api`, `crab-config`, `tokio`, `serde_json`, `crab-common`

**Feature Flags**：无

---

### 6.11 `crates/agent/` — 多 Agent 系统

**职责**：Agent 编排、任务分发、消息循环（对标 CC `src/query.ts` + `src/QueryEngine.ts` + `src/coordinator/` + `src/tasks/`）

这是整个系统的**核心引擎**，实现了最关键的 query loop。

**目录结构**

```
src/
├── lib.rs
├── coordinator.rs        // Agent 编排、workers pool
├── query_loop.rs         // 核心消息循环（最重要的文件）
├── task.rs               // TaskList, TaskUpdate, 依赖图
├── team.rs               // Team 创建、成员管理
├── message_bus.rs        // Agent 间消息传递 (tokio::mpsc)
├── message_router.rs     // Agent 间消息路由（按 name/broadcast）
├── worker.rs             // 子 Agent worker 生命周期
├── system_prompt.rs      // 系统提示构建 + CRAB.md 注入
├── summarizer.rs         // 对话摘要生成
├── rollback.rs           // 回滚机制（/undo）
├── error_recovery.rs     // 错误恢复策略（自动重试/降级）
├── retry.rs              // 自动重试机制（指数退避）
└── repl_commands.rs      // REPL 命令（/undo /branch /fork /checkpoint）
```

**消息循环（核心）**

```rust
// query_loop.rs — 核心消息循环
// 对标 CC src/query.ts 的 query() 函数
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message};
use crab_session::Conversation;
use crab_tools::executor::ToolExecutor;
use crab_api::LlmBackend;
use tokio::sync::mpsc;

/// 消息循环：用户输入 → API → 工具执行 → 继续 → 直到无工具调用
pub async fn query_loop(
    conversation: &mut Conversation,
    api: &LlmBackend,
    tools: &ToolExecutor,
    event_tx: mpsc::Sender<Event>,
) -> crab_common::Result<()> {
    loop {
        // 1. 检查上下文是否需要压缩
        if conversation.needs_compaction() {
            // -> 见 [session#compaction]
            todo!("compact conversation");
        }

        // 2. 构造 API 请求（借用 messages 避免 clone）
        let req = crab_api::MessageRequest {
            model: crab_core::model::ModelId("claude-sonnet-4-20250514".into()),
            messages: std::borrow::Cow::Borrowed(&conversation.messages),
            system: Some(conversation.system_prompt.clone()),
            max_tokens: 16384,
            tools: tools.registry().tool_schemas(),
            temperature: None,
        };

        // 3. 流式发送到 API
        let mut stream = api.stream_message(req);

        // 4. 收集 assistant 响应
        let mut assistant_content: Vec<ContentBlock> = Vec::new();
        let mut has_tool_use = false;

        // （此处省略流式处理细节，收集 ContentBlock）
        // ...

        // 5. 将 assistant 消息加入对话
        conversation.push(Message {
            role: crab_core::message::Role::Assistant,
            content: assistant_content.clone(),
        });

        // 6. 如果没有工具调用，循环结束
        if !has_tool_use {
            break;
        }

        // 7. 按读写分区并发执行工具调用
        //    读工具（is_read_only=true）用 FuturesUnordered 并发（max 10）
        //    写工具串行执行，保证顺序一致性
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

        // 7a. 读工具并发执行（max 10 并发）
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

        // 7b. 写工具串行执行
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

        // 8. 将工具结果作为 user 消息加入对话
        conversation.push(Message {
            role: crab_core::message::Role::User,
            content: tool_results,
        });

        // 9. 回到步骤 1，继续循环
    }

    Ok(())
}
```

```rust
/// 按 is_read_only() 将工具调用分为读/写两组
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
// coordinator.rs — 多 Agent 编排
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// 运行中的 worker 句柄
pub struct RunningWorker {
    pub worker_id: String,
    pub cancel: CancellationToken,
    pub handle: tokio::task::JoinHandle<WorkerResult>,
}

/// 多 sub-agent 编排器
pub struct AgentCoordinator {
    backend: Arc<LlmBackend>,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryLoopConfig,
    event_tx: mpsc::Sender<Event>,
    running: HashMap<String, RunningWorker>,
    completed: Vec<WorkerResult>,           // 摘要（不含对话历史）
    cancel: CancellationToken,
}

impl AgentCoordinator {
    /// 启动一个新的 sub-agent worker
    pub async fn spawn_worker(
        &mut self,
        config: WorkerConfig,
        task_prompt: String,
    ) -> crab_common::Result<String>;

    /// 等待指定 worker 完成
    pub async fn wait_for(&mut self, worker_id: &str) -> Option<WorkerResult>;

    /// 等待所有 worker 完成
    pub async fn wait_all(&mut self) -> Vec<WorkerResult>;

    /// 取消指定 worker
    pub fn cancel_worker(&mut self, worker_id: &str) -> bool;

    /// 取消所有 worker
    pub fn cancel_all(&mut self);
}

// worker.rs — Sub-agent worker 生命周期
pub struct WorkerConfig {
    pub worker_id: String,
    pub system_prompt: String,
    pub max_turns: Option<usize>,
    pub max_duration: Option<std::time::Duration>,
    pub context_window: u64,
}

pub struct WorkerResult {
    pub worker_id: String,
    pub output: Option<String>,             // 最后一条 assistant 文本
    pub success: bool,
    pub usage: TokenUsage,
    pub conversation: Conversation,         // 完整对话历史
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
    /// 在 tokio 任务中运行独立 query loop
    pub fn spawn(self, task_prompt: String) -> tokio::task::JoinHandle<WorkerResult>;
}
```

**Task 系统（已实现）**

```rust
// task.rs — TaskStore + TaskItem + 依赖图
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
    pub blocks: Vec<String>,         // 此任务阻塞哪些任务
    pub blocked_by: Vec<String>,     // 此任务被哪些任务阻塞
}

/// 线程安全的任务存储，支持依赖图
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

**流式工具执行（StreamingToolExecutor）**

CC 在 API 流式响应期间，一旦 `tool_use` 的 JSON 完整解析出来就立即启动执行，
不等待 `message_stop` 事件。Crab Code 应实现同样的优化：

```
API SSE 流:  [content_block_start: tool_use] → [input_json_delta...] → [content_block_stop]
                                                                            │
                                              JSON 完整 → 立即 spawn 执行 ──┘
                                                         │
后续 block 继续流入 ◄────── 与工具执行并行 ──────────────►│ 工具结果就绪
```

```rust
/// 流式工具执行器 — 在 API 流式响应期间提前启动工具
pub struct StreamingToolExecutor {
    pending: Vec<tokio::task::JoinHandle<(String, crab_common::Result<ToolOutput>)>>,
}

impl StreamingToolExecutor {
    /// 当某个 tool_use block 的 JSON 完整解析后立即调用
    pub fn spawn_early(&mut self, id: String, name: String, input: Value, ctx: ToolContext, executor: Arc<ToolExecutor>) {
        let handle = tokio::spawn(async move {
            let result = executor.execute(&name, input, &ctx).await;
            (id, result)
        });
        self.pending.push(handle);
    }

    /// message_stop 后收割所有已完成/进行中的工具结果
    pub async fn collect_all(&mut self) -> Vec<(String, crab_common::Result<ToolOutput>)> {
        let mut results = Vec::new();
        for handle in self.pending.drain(..) {
            results.push(handle.await.expect("tool task panicked"));
        }
        results
    }
}
```

**外部依赖**：`crab-core`, `crab-session`, `crab-tools`, `crab-api`, `tokio`, `tokio-util`, `futures`, `crab-common`

**Feature Flags**：无

---

### 6.12 `crates/tui/` — 终端 UI

**职责**：所有终端界面渲染（对标 CC `src/components/` + `src/screens/` + `src/ink/` + `src/vim/`）

CC 使用 React/Ink 渲染终端 UI，Crab 使用 ratatui + crossterm 实现同等体验。

**目录结构**

```
src/
├── lib.rs
├── app.rs                  // App 状态机，主循环
├── event.rs                // crossterm Event → AppEvent 映射（KeyEvent/MouseEvent/Resize）
├── layout.rs               // 布局计算（面板分配、响应式）
├── runner.rs               // TUI 运行器（初始化/启动/停止终端）
├── keybindings.rs          // 快捷键配置绑定（可用户自定义）
├── ansi.rs                 // ANSI 转义 → ratatui Span 转换
│
├── components/             // UI 组件（21 个）
│   ├── mod.rs
│   ├── input.rs            // 多行输入框 + Vim motion
│   ├── markdown.rs         // Markdown 渲染 (pulldown-cmark → ratatui)
│   ├── syntax.rs           // 代码高亮 (syntect → ratatui Style)
│   ├── spinner.rs          // 加载指示器 (思考中/执行中)
│   ├── diff.rs             // Diff 可视化 (unified 红绿对比)
│   ├── select.rs           // 选择列表 (工具确认/slash 命令)
│   ├── dialog.rs           // 确认/权限对话框
│   ├── cost_bar.rs         // token/费用状态栏
│   ├── task_list.rs        // 任务进度面板
│   ├── autocomplete.rs     // 自动补全弹出框（输入时触发）
│   ├── code_block.rs       // 代码块 + 一键复制按钮
│   ├── command_palette.rs  // 命令面板（Ctrl+P，模糊搜索命令）
│   ├── input_history.rs    // 输入历史（↑↓ 浏览历史输入）
│   ├── loading.rs          // 加载动画组件（多种动画样式）
│   ├── notification.rs     // Toast 通知系统（顶部弹出/自动消失）
│   ├── progress_indicator.rs // 进度指示器（百分比 + 进度条）
│   ├── search.rs           // 全局搜索面板（内容/文件名搜索）
│   ├── shortcut_hint.rs    // 快捷键提示栏（底部常显）
│   ├── status_bar.rs       // 增强状态栏（模式/provider/token/延迟）
│   └── tool_output.rs      // 工具输出折叠显示（可展开/收起）
│
├── vim/                    // Vim 模式
│   ├── mod.rs
│   ├── motion.rs           // hjkl, w/b/e, 0/$, gg/G
│   ├── operator.rs         // d/c/y + motion
│   └── mode.rs             // Normal/Insert/Visual
│
└── theme.rs                // 颜色主题（dark/light/solarized/可自定义）
```

**App 主循环**

```rust
// app.rs — ratatui App
use ratatui::prelude::*;
use crossterm::event::{self, Event as TermEvent, KeyCode};
use crab_core::event::Event;
use tokio::sync::mpsc;

/// App 级共享资源（初始化一次，避免每次渲染重建）
pub struct SharedResources {
    pub syntax_set: syntect::parsing::SyntaxSet,
    pub theme_set: syntect::highlighting::ThemeSet,
}

impl SharedResources {
    pub fn new() -> Self {
        Self {
            syntax_set: syntect::parsing::SyntaxSet::load_defaults_newlines(),
            theme_set: syntect::highlighting::ThemeSet::load_defaults(),
        }
    }
}

pub struct App {
    /// 输入缓冲区
    input: String,
    /// 状态栏更新通道（watch channel — 只关心最新值，不堆积）
    status_watch_rx: tokio::sync::watch::Receiver<StatusBarData>,
    /// 消息显示区
    messages: Vec<DisplayMessage>,
    /// 组合状态（替代单一 enum，支持叠加层）
    state: UiState,
    /// 来自 agent 的事件
    event_rx: mpsc::Receiver<Event>,
    /// 共享资源（SyntaxSet/ThemeSet 等，初始化一次）
    resources: SharedResources,
}

/// 组合状态模式 — 主状态 + 叠加层 + 通知 + 焦点 + 活跃工具进度
pub struct UiState {
    /// 主交互状态
    pub main: MainState,
    /// 模态叠加层（同一时刻只有一个模态覆盖：权限对话框 或 命令面板）
    /// 使用 Option 而非 Vec：模态 UI 同时只展示一个，多个排队无意义
    pub overlay: Option<Overlay>,
    /// 非模态通知队列（toast 样式，自动消失，不阻塞输入）
    pub notifications: std::collections::VecDeque<Toast>,
    /// 当前焦点位置（决定键盘事件路由到哪个组件）
    pub focus: FocusTarget,
    /// 活跃工具执行进度（支持并发工具追踪）
    pub active_tools: Vec<ToolProgress>,
}

/// 非模态通知（类似 toast，显示后自动消失）
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub created_at: std::time::Instant,
    /// 显示持续时间（默认 3 秒）
    pub ttl: std::time::Duration,
}

pub enum ToastLevel {
    Info,
    Warning,
    Error,
}

/// 焦点目标 — 决定键盘事件路由
pub enum FocusTarget {
    /// 输入框（默认焦点）— 接收文本输入和 Enter 提交
    InputBox,
    /// 模态叠加层 — 接收 Esc 关闭、方向键选择、Enter 确认
    Overlay,
    /// 消息滚动区 — 接收 j/k/PgUp/PgDn 滚动
    MessageScroll,
}

// 焦点路由逻辑：
// - overlay.is_some() 时，焦点强制切到 FocusTarget::Overlay
// - overlay 关闭后，焦点回到 FocusTarget::InputBox
// - 用户按 Ctrl+Up/Down 可临时切到 MessageScroll 浏览历史

pub enum MainState {
    /// 等待用户输入
    Idle,
    /// API 调用中（显示 spinner）
    Thinking,
    /// 流式响应接收中 — 支持增量渲染
    Streaming(StreamingMessage),
}

/// 流式消息状态 — 支持 delta 追加 + 增量解析
/// 注意：这里的 "增量" 是指 **解析优化**（避免重复解析已处理的 Markdown），
/// 而非跳过渲染 — 每帧仍然完整渲染所有已解析的 blocks。
pub struct StreamingMessage {
    /// 已接收的完整文本
    pub buffer: String,
    /// 已解析到的偏移量（只需解析 buffer[parsed_offset..] 中的新增部分）
    pub parsed_offset: usize,
    /// 已解析的渲染块列表（Markdown → 结构化块，增量追加）
    pub parsed_blocks: Vec<RenderedBlock>,
    /// 是否完成
    pub complete: bool,
}

/// 已解析的渲染块（Markdown 解析结果的结构化表示）
pub enum RenderedBlock {
    Paragraph(String),
    CodeBlock { language: String, code: String },
    Heading { level: u8, text: String },
    List(Vec<String>),
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    BlockQuote(String),
    HorizontalRule,
    Link { text: String, url: String },
    Image { alt: String, url: String }, // placeholder — 终端无法渲染图片，显示 alt 文本
}

impl StreamingMessage {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            parsed_offset: 0,
            parsed_blocks: Vec::new(),
            complete: false,
        }
    }

    /// 追加增量文本
    pub fn append_delta(&mut self, delta: &str) {
        self.buffer.push_str(delta);
    }

    /// 增量解析：只解析 buffer[parsed_offset..] 中的新内容，追加到 parsed_blocks
    pub fn parse_pending(&mut self) {
        let new_content = &self.buffer[self.parsed_offset..];
        // 使用 pulldown-cmark 解析新增内容，生成 RenderedBlock
        // 注意：需要处理块边界（如未闭合的代码块跨 delta）
        // ...
        self.parsed_offset = self.buffer.len();
    }
}

pub enum Overlay {
    /// 权限确认对话框
    PermissionDialog { tool_name: String, request_id: String },
    /// 命令面板 (Ctrl+K)
    CommandPalette,
}

pub struct ToolProgress {
    pub id: String,
    pub name: String,
    pub started_at: std::time::Instant,
}

pub struct DisplayMessage {
    pub role: String,
    pub content: String,
    pub cost: Option<String>,
}

impl App {
    /// 主渲染循环
    /// 使用 crossterm::event::EventStream 替代 spawn_blocking+poll/read
    /// 避免竞态条件：poll 和 read 在不同线程调用时可能丢失事件
    pub async fn run(
        &mut self,
        terminal: &mut Terminal<impl Backend>,
    ) -> crab_common::Result<()> {
        use crossterm::event::EventStream;
        use futures::StreamExt;

        let mut term_events = EventStream::new();
        let target_fps = 30;
        let frame_duration = std::time::Duration::from_millis(1000 / target_fps);

        // 使用 tokio::time::interval 替代 sleep(saturating_sub)
        // MissedTickBehavior::Skip 确保：如果某帧处理超时，跳过错过的 tick 而非连续补帧
        let mut frame_tick = tokio::time::interval(frame_duration);
        frame_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // 终端输入（EventStream 是异步 Stream，无竞态）
                Some(Ok(term_event)) = term_events.next() => {
                    if let TermEvent::Key(key) = term_event {
                        match key.code {
                            KeyCode::Enter => {
                                self.submit_input();
                            }
                            KeyCode::Char(c) => {
                                self.input.push(c);
                            }
                            KeyCode::Esc => {
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }
                // Agent 事件
                Some(event) = self.event_rx.recv() => {
                    self.handle_agent_event(event);
                }
                // 状态栏刷新 — watch channel 通知（费用更新、token 计数变化等）
                // watch::Receiver 只保留最新值，多次写入只触发一次 changed()
                // 比 mpsc 更适合 "最新状态" 场景（不堆积，不丢更新）
                Ok(()) = self.status_watch_rx.changed() => {
                    let status = self.status_watch_rx.borrow().clone();
                    self.update_status_bar(status);
                }
                // 帧率定时器 — interval + Skip 比 sleep(saturating_sub) 更精确
                // 不会因为计算 saturating_sub 时的时间差导致帧率漂移
                _ = frame_tick.tick() => {
                    terminal.draw(|frame| self.render(frame))?;
                }
            }
        }
    }

    fn render(&self, frame: &mut Frame) {
        // 使用 ratatui Layout 分区
        // 上: 消息历史 (Markdown 渲染)
        // 中: 工具输出 / spinner
        // 下: 输入框 + 状态栏
        // ...
    }
}
```

**外部依赖**：`ratatui`, `crossterm`, `syntect`, `pulldown-cmark`, `crab-core`, `crab-session`, `crab-config`, `crab-common`

> tui 不直接依赖 tools，通过 `crab_core::Event` 枚举接收工具执行状态，由 crates/cli 负责组装 agent+tui。

**Feature Flags**：无（tui 本身是 cli 的可选依赖）

---

### 6.13 `crates/plugin/` — 插件系统

**职责**：技能/插件的发现、加载、执行（对标 CC `src/skills/` + `src/services/plugins/`）

**目录结构**

```
src/
├── lib.rs
├── skill.rs          // Skill 发现、加载、执行
├── wasm_runtime.rs   // WASM 插件沙箱 (wasmtime, feature = "wasm")
├── manifest.rs       // 插件清单解析 (skill.json)
└── hook.rs           // 生命周期钩子
```

**核心类型**

```rust
// skill.rs — 技能系统
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub trigger: SkillTrigger,
    pub instructions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SkillTrigger {
    /// 用户输入 /command 触发
    Command { name: String },
    /// 正则匹配用户输入
    Pattern { regex: String },
    /// 手动调用
    Manual,
}

/// Skill 触发器（已实现，#[derive(Default)] 默认为 Manual）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type")]
pub enum SkillTrigger {
    /// 用户输入 /command 触发
    Command { name: String },
    /// 正则匹配用户输入
    Pattern { regex: String },
    /// 手动调用（默认）
    #[default]
    Manual,
}

pub struct SkillRegistry {
    skills: Vec<SkillManifest>,
}

impl SkillRegistry {
    /// 从 ~/.crab/skills/ 和项目 .crab/skills/ 发现技能（已实现）
    pub fn discover(paths: &[std::path::PathBuf]) -> crab_common::Result<Self>;

    /// 按名称查找
    pub fn find(&self, name: &str) -> Option<&SkillManifest>;

    /// 按 /command 名匹配
    pub fn find_by_command(&self, command: &str) -> Option<&SkillManifest>;

    /// 所有技能列表
    pub fn all(&self) -> &[SkillManifest];
}
```

**外部依赖**：`crab-core`, `crab-common`, `wasmtime` (optional)

**Feature Flags**

```toml
[features]
default = []
wasm = ["wasmtime"]
```

---

### 6.14 `crates/telemetry/` — 可观测性

**职责**：分布式追踪和指标收集（对标 CC `src/services/analytics/` + `src/services/diagnosticTracking.ts`）

**目录结构**

```
src/
├── lib.rs
├── tracer.rs         // OpenTelemetry tracer 初始化
├── metrics.rs        // 自定义 metrics（API 延迟、工具执行时间等）
├── cost.rs           // 费用追踪
└── export.rs         // OTLP 导出
```

**核心接口**

```rust
// tracer.rs
use tracing_subscriber::prelude::*;

/// 初始化追踪系统
pub fn init(service_name: &str, endpoint: Option<&str>) -> crab_common::Result<()> {
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
        // 添加 OpenTelemetry layer 到 registry
    }

    #[cfg(not(feature = "otlp"))]
    let _ = (service_name, endpoint); // 抑制未使用警告

    registry.init();
    Ok(())
}
```

**外部依赖**：`tracing`, `tracing-subscriber`, `crab-common`；OTLP 相关为可选依赖

**Feature Flags**

```toml
[features]
default = ["fmt"]
fmt = ["tracing-subscriber/fmt"]                               # 本地日志格式化（默认）
otlp = [                                                       # OpenTelemetry OTLP 导出
    "opentelemetry",
    "opentelemetry-otlp",
    "opentelemetry-sdk",
    "tracing-opentelemetry",
]
```

> 默认只启用 `fmt`（本地 tracing-subscriber），不引入 opentelemetry 全家桶。
> 生产部署需要 OTLP 导出时通过 `cargo build -F otlp` 开启。

---

### 6.15 `crates/cli/` — 终端入口

**职责**：极薄的二进制入口，只做组装不含业务逻辑（对标 CC `src/entrypoints/cli.tsx`）

**目录结构**

```
src/
├── main.rs           // #[tokio::main] 入口
├── commands/         // clap 子命令定义
│   ├── mod.rs
│   ├── chat.rs       // 默认交互模式 (crab chat)
│   ├── run.rs        // 非交互单次执行 (crab run -p "...")
│   ├── session.rs    // ps, logs, attach, kill
│   ├── config.rs     // 配置管理 (crab config set/get)
│   ├── mcp.rs        // MCP server 模式 (crab mcp serve)
│   └── serve.rs      // 服务模式
└── setup.rs          // 初始化、信号注册、版本检查、panic hook
```

**Panic Hook 设计**

```rust
// setup.rs — 终端状态恢复 panic hook
// 必须在 terminal.init() 之后、进入主循环之前注册
pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // 1. 恢复终端状态（最重要 — 否则终端不可用）
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        // 2. 调用原始 hook（打印 panic 信息）
        original_hook(panic_info);
        // 推荐替代方案：使用 color-eyre::install() 自动处理，
        // 提供美化的 panic 报告 + backtrace
    }));
}
```

**入口代码**

```rust
// main.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "crab", version, about = "AI coding assistant")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// 直接传入 prompt（等同于 crab run -p）
    #[arg(short, long)]
    prompt: Option<String>,

    /// 权限模式
    #[arg(long, default_value = "default")]
    permission_mode: String,

    /// 指定模型
    #[arg(long)]
    model: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// 交互模式（默认）
    Chat,
    /// 单次执行
    Run {
        #[arg(short, long)]
        prompt: String,
    },
    /// 会话管理
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// 配置管理
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// MCP 模式
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 1. 初始化 telemetry
    crab_telemetry::init("crab-code", None)?;

    // 2. 加载配置
    let config = crab_config::load_merged_settings(None)?;

    // 3. 初始化认证
    let auth = crab_auth::resolve_api_key()
        .ok_or_else(|| anyhow::anyhow!("no API key found"))?;

    // 4. 分发命令
    match cli.command.unwrap_or(Commands::Chat) {
        Commands::Chat => {
            // 启动交互模式
            // ...
        }
        Commands::Run { prompt } => {
            // 单次执行
            // ...
        }
        _ => { /* ... */ }
    }

    Ok(())
}
```

**外部依赖**：所有 crate, `clap`, `tokio`, `anyhow`

**Feature Flags**

```toml
[features]
default = ["tui"]
tui = ["crab-tui"]
full = ["tui", "crab-plugin/wasm", "crab-api/bedrock", "crab-api/vertex"]
```

---

### 6.16 `crates/daemon/` — 后台守护进程

**职责**：后台持久运行的守护进程，管理多个会话（对标 CC `src/daemon/`）

**目录结构**

```
src/
└── main.rs
```

**IPC 通信设计**

```
CLI ◄─── Unix socket (Linux/macOS) / Named pipe (Windows) ───► Daemon
         协议: 长度前缀帧 + JSON 消息
         格式: [4 bytes: payload_len_le32][payload_json]
```

**IPC 消息协议**

```rust
/// CLI → Daemon 请求
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonRequest {
    /// 创建新 session 或 attach 到已有 session
    Attach { session_id: Option<String>, working_dir: PathBuf },
    /// 断开连接但保持 session 运行
    Detach { session_id: String },
    /// 列出活跃 session
    ListSessions,
    /// 终止 session
    KillSession { session_id: String },
    /// 发送用户输入
    UserInput { session_id: String, content: String },
    /// 健康检查
    Ping,
}

/// Daemon → CLI 响应/事件推送
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonResponse {
    /// attach 成功
    Attached { session_id: String },
    /// session 列表
    Sessions { list: Vec<SessionInfo> },
    /// 转发 agent Event（流式推送）
    Event(crab_core::event::Event),
    /// 错误
    Error { message: String },
    /// Pong
    Pong,
}
```

**Session Pool 管理**

```rust
pub struct SessionPool {
    /// 活跃 session（最多 N 个，默认 8）
    sessions: HashMap<String, SessionHandle>,
    /// 共享 API 连接池（所有 session 复用）
    api_client: Arc<LlmBackend>,
    /// 空闲超时自动清理（默认 30 分钟）
    idle_timeout: Duration,
}

pub struct SessionHandle {
    pub id: String,
    pub working_dir: PathBuf,
    pub created_at: Instant,
    pub last_active: Instant,
    /// 当前是否有 CLI 连接
    pub attached: bool,
    /// session 控制 channel
    pub tx: mpsc::Sender<DaemonRequest>,
}
```

**CLI attach/detach 流程**

```
1. CLI 启动 → 连接 daemon socket
2. 发送 Attach { session_id: None } → daemon 创建新 session
3. daemon 回复 Attached { session_id: "xxx" }
4. CLI 发送 UserInput → daemon 转发给 query_loop
5. daemon 流式推送 Event → CLI 渲染
6. CLI 退出 → 发送 Detach → session 保持后台运行
7. CLI 重新 Attach { session_id: "xxx" } → 恢复对话
```

**核心逻辑**

```rust
// main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 0. 日志初始化 — 使用 tracing-appender 日志轮转
    //    daemon 是长驻进程，必须有日志轮转防止磁盘写满
    let log_dir = directories::ProjectDirs::from("", "", "crab-code")
        .expect("failed to resolve project dirs")
        .data_dir()
        .join("logs");
    let file_appender = tracing_appender::rolling::daily(&log_dir, "daemon.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false) // 文件日志不需要 ANSI 颜色
        .init();

    // 1. PID 文件 + 单实例检查（fd-lock）
    // 2. 初始化共享 API 连接池
    // 3. 创建 SessionPool
    // 4. 监听 IPC socket
    // 5. accept loop: 每个 CLI 连接 spawn 独立 handler
    // 6. 定期清理空闲 session
    // ...
}
```

**外部依赖**：`crab-core`, `crab-session`, `crab-api`, `crab-tools`, `crab-config`, `crab-agent`, `crab-common`, `tokio`, `fd-lock`, `tracing-appender`

---

### 6.17 全局状态拆分：AppConfig / AppRuntime

CLI 和 Daemon 共享的全局状态拆分为 **不可变配置** 和 **可变运行时** 两部分，
避免单一 `Arc<RwLock<AppState>>` 导致读路径被写锁阻塞。

```rust
/// 不可变配置 — 启动时初始化，运行期间不变
/// Arc<AppConfig> 零锁共享，任意线程/task 可直接读取
pub struct AppConfig {
    /// 合并后的 settings.json
    pub settings: crab_config::Settings,
    /// CRAB.md 内容（全局 + 用户 + 项目）
    pub crab_md: Vec<crab_config::CrabMd>,
    /// 权限策略
    pub permission_policy: crab_core::permission::PermissionPolicy,
    /// 模型配置
    pub model_id: crab_core::model::ModelId,
    /// 项目根目录
    pub project_dir: std::path::PathBuf,
}

/// 可变运行时状态 — 运行期间频繁变化
/// Arc<RwLock<AppRuntime>> 读多写少，RwLock 读锁不互斥
pub struct AppRuntime {
    /// 费用追踪（每次 API 调用后写入）
    pub cost_tracker: crab_core::model::CostTracker,
    /// 活跃会话列表（daemon 模式下多个）
    pub active_sessions: Vec<String>,
    /// MCP 连接池（动态连接/断开）
    pub mcp_connections: std::collections::HashMap<String, crab_mcp::McpClient>,
}

// 使用方式：
// let config = Arc::new(AppConfig { ... });     // 启动时构建，之后只读
// let runtime = Arc::new(RwLock::new(AppRuntime { ... })); // 运行时读写
//
// // 热路径：零锁读取配置
// let model = &config.model_id;
//
// // 写路径：更新费用（短暂写锁）
// runtime.write().await.cost_tracker.record(&usage, cost);
```

---

## 七、设计原则

| # | 原则 | 说明 | 理由 |
|---|------|------|------|
| 1 | **core 零 I/O** | 纯数据结构和 trait，不含文件/网络/进程操作 | 可被 CLI/GUI/WASM 任意前端复用，单元测试无需 mock |
| 2 | **tools 独立 crate** | 21+ 工具编译量大，独立后增量编译只触发改动的工具 | 改一个工具不用重编全部 |
| 3 | **fs 和 process 分开** | 职责正交：fs 处理文件内容，process 处理执行 | GlobTool 不需要 sysinfo，BashTool 不需要 globset |
| 4 | **tui 可选** | cli bin 通过 feature flag 决定是否编译 tui | 未来 Tauri GUI 引 core+session+tools 但不引 tui |
| 5 | **api 与 session 分层** | api 只管 HTTP 通信，session 管业务状态 | 替换 API provider 不影响会话逻辑 |
| 6 | **feature flag 控可选依赖** | 不用 Bedrock 不编译 AWS SDK，不用 WASM 不编译 wasmtime | 减少编译时间和二进制体积 |
| 7 | **workspace.dependencies 统一版本** | 所有 crate 共享同一版本的三方库 | 避免依赖冲突和重复编译 |
| 8 | **binary crate 只做组装** | cli/daemon 只做组装，所有逻辑在 library crate 中 | 方便未来新增入口（desktop/wasm/mobile） |

---

## 八、Feature Flag 策略

### 8.1 各 Crate Feature 配置

```toml
# ─── crates/api/Cargo.toml ───
[features]
default = []
bedrock = ["aws-sdk-bedrockruntime", "aws-config"]  # AWS Bedrock provider
vertex = ["gcp-auth"]                                 # Google Vertex provider
proxy = ["reqwest/socks"]                             # SOCKS5 代理支持

# ─── crates/auth/Cargo.toml ───
[features]
default = []
bedrock = ["aws-sdk-bedrockruntime", "aws-config"]   # AWS SigV4 签名

# ─── crates/mcp/Cargo.toml ───
[features]
default = []
ws = ["tokio-tungstenite"]                            # WebSocket 传输

# ─── crates/plugin/Cargo.toml ───
[features]
default = []
wasm = ["wasmtime"]                                   # WASM 插件沙箱

# ─── crates/process/Cargo.toml ───
[features]
default = []
pty = ["portable-pty"]                                # 伪终端分配
sandbox = []                                          # 进程沙箱

# ─── crates/telemetry/Cargo.toml ───
[features]
default = ["fmt"]
fmt = ["tracing-subscriber/fmt"]                             # 本地日志（默认）
otlp = [                                                     # OTLP 导出
    "opentelemetry", "opentelemetry-otlp",
    "opentelemetry-sdk", "tracing-opentelemetry",
]

# ─── crates/cli/Cargo.toml ───
[features]
default = ["tui"]
tui = ["crab-tui"]                                    # 终端 UI（默认开启）
full = [                                              # 全功能构建
    "tui",
    "crab-plugin/wasm",
    "crab-api/bedrock",
    "crab-api/vertex",
    "crab-process/pty",
    "crab-telemetry/otlp",
]
minimal = []                                          # 最小构建（无 TUI）
```

### 8.2 构建组合

| 场景 | 命令 | 编译内容 |
|------|------|---------|
| 日常开发 | `cargo build` | cli + tui（默认） |
| 最小构建 | `cargo build --no-default-features -F minimal` | cli only, 无 tui |
| 全功能 | `cargo build -F full` | 所有 provider + WASM + PTY |
| 仅 library | `cargo build -p crab-core` | 单 crate 编译 |
| WASM 目标 | `cargo build -p crab-core --target wasm32-unknown-unknown` | core 层 WASM |

### 8.3 对标 CC Feature Flags

CC 源码中通过 `featureFlags.ts` 管理约 31 个运行时 flag，Crab Code 将其拆分为：

- **编译时 feature**：provider 选择、WASM 插件、PTY 等（Cargo features）
- **运行时 flag**：通过 `config/feature_flag.rs` 管理，支持远程下发

---

## 九、Workspace 配置

### 9.1 根 Cargo.toml

```toml
[workspace]
resolver = "2"
members = ["crates/*", "xtask"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
repository = "https://github.com/user/crab-code"
description = "AI coding assistant in Rust"

[workspace.dependencies]
# 完整依赖列表见根 Cargo.toml
# 主要分类：异步运行时(tokio)、序列化(serde)、CLI(clap)、HTTP(reqwest)、
# TUI(ratatui)、错误处理(thiserror/anyhow)、文件系统(globset/ignore)等

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
channel = "1.85.0"    # edition 2024 + async fn in trait 最低版本
components = ["rustfmt", "clippy", "rust-analyzer"]
```

### 9.3 rustfmt.toml

```toml
edition = "2024"
max_width = 100
tab_spaces = 4
use_field_init_shorthand = true
```

---

## 十、数据流设计

### 10.1 主数据流：Query Loop

```
用户输入
  │
  ▼
┌──────────┐    prompt     ┌──────────┐   HTTP POST    ┌──────────────┐
│crates/cli│──────────────►│  agent   │───────────────►│  Anthropic   │
│ (TUI)    │               │query_loop│   /v1/messages │  API Server  │
└──────────┘               └────┬─────┘◄───────────────┘──────────────┘
      ▲                         │          SSE stream
      │                         │
      │ Event::ContentDelta     │ 解析 assistant 响应
      │                         │
      │                    ┌────▼─────┐
      │                    │ 有工具   │──── 否 ──► 循环结束，显示结果
      │                    │ 调用？   │
      │                    └────┬─────┘
      │                         │ 是
      │                         ▼
      │                    ┌──────────┐   delegate    ┌────────────┐
      │                    │  tools   │──────────────►│  fs / mcp  │
      │                    │ executor │               │  process   │
      │                    └────┬─────┘◄──────────────┘────────────┘
      │                         │         ToolOutput
      │ Event::ToolResult       │
      └─────────────────────────┘
            工具结果追加到 messages，回到 query_loop 顶部
```

### 10.2 MCP 工具调用

```
┌──────────┐  call_tool   ┌──────────┐  Crab façade  ┌──────────────┐
│  tools   │─────────────►│   mcp    │──────────────►│  MCP Server  │
│ executor │              │  client  │               │  (外部进程)   │
└──────────┘              └────┬─────┘               └──────┬───────┘
                               │                             │
                               │     rmcp transport/client   │
                          ┌────▼─────────────────────────┐   │
                          │  stdio child process / HTTP  │   │
                          │  handshake / tools/list      │   │
                          │  tools/call / resources      │   │
                          └──────────────────────────────┘   │
                                                             │
                               ◄─────────────────────────────┘
                                     tool / resource result
```

### 10.3 上下文压缩决策流

```
┌──────────────┐
│ query_loop   │
│ 每轮开始     │
└──────┬───────┘
       │
       ▼
┌──────────────┐     estimated_tokens()
│ 估算当前     │──────────────────────────┐
│ token 数     │                          │
└──────────────┘                          ▼
                                   ┌──────────────┐
                                   │ > 70% 窗口？ │
                                   └──────┬───────┘
                                          │
                               ┌─── 否 ───┼─── 是 ───┐
                               │          │           │
                               ▼          │           ▼
                          正常继续         │    ┌──────────────┐
                                          │    │ 选择压缩策略 │
                                          │    └──────┬───────┘
                                          │           │
                                          │    ┌──────▼───────┐
                                          │    │  Snip        │ ← 70-80%
                                          │    │  Microcompact│ ← 80-85%
                                          │    │  Summarize   │ ← 85-90%
                                          │    │  Hybrid      │ ← 90-95%
                                          │    │  Truncate    │ ← > 95%
                                          │    └──────┬───────┘
                                          │           │
                                          │           ▼
                                          │    ┌──────────────┐
                                          │    │ 调用小模型   │
                                          │    │ 生成摘要     │
                                          │    └──────┬───────┘
                                          │           │
                                          │           ▼
                                          │    ┌──────────────┐
                                          │    │ 重建消息列表 │
                                          │    │ [summary] +  │
                                          │    │ 最近 N 轮    │
                                          │    └──────┬───────┘
                                          │           │
                                          └───────────┘
                                                      │
                                                      ▼
                                                继续 query_loop
```

---

## 十一、扩展系统设计

### 11.1 多模型支持架构（crab-api）

`crab-api` 的多模型回退与错误分类层，在 `LlmBackend` 枚举之上叠加：

```
用户请求
    │
    ▼
┌─────────────────┐
│    fallback.rs   │  — 多模型回退链（主→备1→备2）
└────────┬────────┘
         │
    ┌────▼────────────────┐
    │  retry_strategy.rs  │  — 增强重试（退避 + jitter）
    └────┬────────────────┘
         │
    ┌────▼────────────────┐
    │ error_classifier.rs │  — 错误分类（可重试/不可重试）
    └─────────────────────┘
```

**模块清单**：
- `fallback.rs` — 多模型回退链（主模型失败自动切换备用模型）
- `capabilities.rs` — 模型能力协商与发现
- `context_optimizer.rs` — 上下文窗口优化 + 智能截断
- `streaming.rs` — 流式工具调用解析
- `retry_strategy.rs` / `error_classifier.rs` — 增强重试与错误分类


### 11.2 MCP 协议栈（crab-mcp）

MCP 协议扩展模块：

- `crab-mcp` 是 Crab 的 MCP façade，不直接把底层 SDK 暴露给上层 crate
- client 侧 stdio / HTTP 连接由官方 SDK 承接，Crab 负责配置发现、命名、权限接入与工具桥接
- 配置主路径只保留 `stdio` / `http` / `ws`
- server / prompt / resource / tool registry 仍保留 Crab 自己的抽象层

| 模块 | 功能 |
|------|------|
| `handshake.rs` + `negotiation.rs` | initialize/initialized 握手，能力集协商 |
| `sampling.rs` | 服务端请求 LLM 推理（server → client sampling） |
| `roots.rs` | 工作区根目录声明（client 告知 server 可访问路径）|
| `logging.rs` | 结构化日志消息协议 |
| `sse_server.rs` | crab 作为 MCP server 提供 SSE 传输 |
| `capability.rs` | 能力声明类型 |
| `notification.rs` | 服务端通知推送 |
| `progress.rs` | 进度报告（长时间工具执行）|
| `cancellation.rs` | 请求取消（`$/cancelRequest` JSON-RPC 通知）|
| `health.rs` | 健康检查 + 心跳 |


### 11.3 Agent 可靠性（crab-agent）

**可靠性子系统**：
```
error_recovery → retry → rollback
summarizer（对话摘要生成）
repl_commands（/undo /branch /fork）
```


### 11.4 TUI 组件库（crab-tui，21 组件）

**交互组件**（用户操作）：
- `command_palette` — Ctrl+P 命令面板，模糊搜索所有命令
- `autocomplete` — 输入时弹出补全建议
- `search` — 全局搜索（文件名 + 内容）
- `input_history` — ↑↓ 浏览历史输入

**内容展示组件**：
- `code_block` — 代码块 + 复制按钮（syntect 高亮）
- `tool_output` — 工具输出折叠显示（可展开/收起）

**状态反馈组件**：
- `notification` — Toast 通知（顶部弹出，3s 自动消失）
- `progress_indicator` — 百分比进度条
- `loading` — 多样式加载动画（spin/dot/bar）
- `status_bar` — 增强状态栏（模式/provider/token 计数/响应延迟）
- `shortcut_hint` — 底部常显快捷键提示


### 11.5 Auth 云平台认证（crab-auth）

```
AWS 场景:
  aws_iam.rs → 支持 IRSA（Pod 级别 IAM 角色）+ 标准 IAM 凭证链
  credential_chain.rs → env → keychain → file → IRSA → instance metadata

GCP 场景:
  gcp_identity.rs → Workload Identity Federation
  vertex_auth.rs → GCP Vertex AI 专用认证
```
