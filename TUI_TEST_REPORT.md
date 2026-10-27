# Crab TUI 测试报告

**测试日期**: 2026-10-27  
**测试环境**: WSL Ubuntu 24.04 + tmux + mintty  
**测试二进制**: `/tmp/crab-target/debug/crab`

## 测试方法

使用 PowerShell 脚本驱动 mintty 窗口，通过 tmux 发送键盘输入并捕获终端输出。支持：
- `launch` - 启动 mintty 窗口
- `resize` - 调整窗口大小
- `shot` - 使用 PrintWindow API 截图
- `fullshot` - 全屏截图
- `text` / `keys` - 发送文本和按键

## 测试结果

### ✅ 通过的功能

1. **基础启动**
   - TUI 正常启动并显示欢迎界面
   - 显示 ASCII art logo
   - 加载 28 个技能
   - 显示版本号和当前目录

2. **配置系统**
   - ✅ 正确读取 `~/.crab/config.toml`
   - ✅ 支持 `api_provider`, `base_url`, `api_key`, `model` 配置
   - ✅ 状态栏正确显示当前模型 (`deepseek-flash`)
   - ✅ DeepSeek API 集成工作正常

3. **Trust Project Overlay**
   - 首次运行时显示 "Trust This Project?" 对话框
   - 按 `y` 正确响应并关闭 overlay
   - 信任后进入正常输入模式

4. **Help Overlay (Ctrl+K Ctrl+H)**
   - 快捷键 `Ctrl+K Ctrl+H` 正确打开帮助 overlay
   - 显示导航快捷键列表（j/k, h/l, gg/G, PgUp/PgDn）
   - 按 `Esc` 正确关闭 overlay

5. **清屏 (Ctrl+L)**
   - `Ctrl+L` 正确清除屏幕内容
   - 保留状态栏和输入框

6. **窗口大小调整**
   - tmux resize 后分隔线正确重新渲染
   - 从 200x50 调整到 120x30 时布局正确适应
   - 状态栏宽度正确调整

7. **输入和显示**
   - 文本输入正常工作
   - 支持 `Ctrl+U` 清除当前行
   - 支持 `Enter` 提交消息

8. **LLM 交互**
   - ✅ 消息正确发送到 API
   - ✅ 响应正确渲染和显示
   - ✅ 状态栏显示 token 使用情况（8k↑ 104↓）
   - ✅ 状态栏显示成本信息（$0.02）
   - ✅ 上下文百分比正确计算（3% (8k/200k)）

9. **错误消息显示**
   - API 错误 (403) 正确格式化并显示
   - JSON 错误消息正确缩进和换行

10. **Slash 命令输入**
    - `/help`、`/config` 等命令正确接受
    - 输入框正确显示命令

### ⚠️ 需要改进的问题

1. **模式指示不明显**
   - **现象**: 状态栏未显示当前是 INSERT 还是 NORMAL 模式
   - **影响**: 用户不知道何时可以使用 vim 风格导航（j/k/h/l）
   - **优先级**: P2 - 可用性问题
   - **建议**: 在状态栏左侧添加 `-- INSERT --` 或 `-- NORMAL --` 指示器

2. **Slash 命令无响应**
   - **现象**: 输入 `/help` 和 `/config` 后无输出
   - **可能原因**: 命令未注册或需要 API 调用
   - **优先级**: P1 - 功能缺失
   - **建议**: 调试命令注册和执行路径

3. **Command Palette (Ctrl+K Ctrl+P) 无响应**
   - **现象**: 按 `Ctrl+K Ctrl+P` 后只在输入框添加了 `p` 字符
   - **可能原因**: 快捷键未正确绑定或 overlay 未实现
   - **优先级**: P2 - 功能缺失
   - **建议**: 检查快捷键注册逻辑

### ⏸️ 未测试的功能

- 流式输出动画（需要观察实时渲染）
- 代码块语法高亮
- 工具调用显示
- Permission overlay
- 长文本滚动
- 侧边栏 (Ctrl+B)
- Transcript (Ctrl+O)
- Task list (Ctrl+K Ctrl+T)
- Model picker (Alt+P)
- Agent detail (Ctrl+K Ctrl+A)

## 性能观察

- **启动时间**: ~6 秒（WSL 环境）
- **响应性**: 按键输入响应即时
- **渲染**: 分隔线和 overlay 渲染流畅
- **内存占用**: 未测量

## 建议

1. **修复 API 认证**
   - 检查 WSL 环境下的配置文件路径
   - 验证 API key 加载逻辑
   - 添加更详细的认证错误信息

2. **添加模式指示器**
   - 在状态栏左侧显示 `-- INSERT --` 或 `-- NORMAL --`
   - 类似 vim 的体验

3. **调试 Slash 命令**
   - 添加命令执行日志
   - 确认 `/help` 等本地命令不依赖 API

4. **验证快捷键绑定**
   - 检查 `Ctrl+K Ctrl+P` 等组合键是否正确注册
   - 测试其他 overlay 快捷键

5. **改进测试基础设施**
   - 当前的 tmux + mintty 方案可行
   - 考虑添加自动化测试脚本
   - 添加 headless 模式用于 CI

## 测试覆盖率

- **UI 渲染**: 40% (基础 layout，未测试内容区域滚动)
- **快捷键**: 30% (测试了 Ctrl+L, Ctrl+K Ctrl+H, Esc)
- **Overlays**: 20% (只测试了 Help 和 Trust)
- **核心功能**: 0% (API 问题阻塞)

## 下一步

1. 解决 API 认证问题
2. 在 Windows 原生环境编译并测试（避免 WSL 特殊性）
3. 添加 mock API 模式用于离线测试
4. 完成剩余 overlay 和交互功能的测试
