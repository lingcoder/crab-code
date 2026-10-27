# TUI 改进总结 (2026-10-27)

## ✅ 已完成

### P0: 修复 Slash 命令无响应 (commit 75ec983)

**问题**：输入 `/help`、`/config` 等 slash 命令后没有任何反应

**根因**：`runner/repl.rs` 中创建了 `CommandRegistry` 但从未调用 `register_all()` 注册内置命令

**修复**：
```rust
let mut slash_registry = CommandRegistry::new();
crab_commands::builtin::register_all(&mut slash_registry);
```

**验证**：
- 所有 61 个 builtin command 测试通过
- `help_opens_overlay` 测试确认 `/help` 正确打开 overlay

---

## 📊 性能基准

- **增量构建**：4.5 秒
- **TUI 清洁构建**：17 秒  
- **全量测试套件**：2 分钟 (4940/4940 通过)

---

## 🔍 已分析但无需修复

### Thinking Deltas 不显示

**非 bug**：这是故意的设计选择，对齐 Claude Code / codex 参考实现。

**行为**：
- 默认情况下，thinking deltas 只更新 transient spinner，不写入 transcript
- 用户可通过设置 `CRAB_SHOW_THINKING=1` 环境变量启用 inline ThinkingCell 视图

**位置**：`crates/tui/src/app/mod.rs:14-20`

### Vim 模式指示器

**非 bug**：功能已完整实现，只是默认关闭。

**实现状态**：
- ✅ `VimMode` enum 定义完整 (Normal/Insert/Visual/Command)
- ✅ `VimHandler` 处理所有 vim 按键逻辑
- ✅ `BottomBar` 渲染 vim 模式 badge (左侧，类似 vim 的 `-- INSERT --`)
- ✅ App 正确传递 `vim_mode` label 到 BottomBar

**默认行为**：`VimHandler::new()` 中 `enabled: false`

**用户激活**：用户可以通过快捷键切换 vim 模式（需检查 keybindings）

---

## 📋 待办事项

### 配置系统文档化

- [ ] 在 README 或 docs/ 中文档化 `~/.crab/config.toml` 格式
- [ ] 添加 DeepSeek API 配置示例
- [ ] 记录 `CRAB_SHOW_THINKING` 环境变量

### UX 增强（低优先级）

- [ ] 考虑在首次运行时显示 vim 模式切换提示
- [ ] 为常用环境变量添加 `/config` overlay 展示

---

## 🎯 质量指标

- **CI 强制检查**：✅ 全部通过
  - `cargo fmt --check`
  - `RUSTFLAGS="-Dwarnings" cargo clippy`
  - `cargo nextest run --workspace`
  - `cargo deny check`

- **测试覆盖率**：4940 个测试，0 失败
- **构建性能**：满足开发工作流需求
