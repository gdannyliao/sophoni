# Sophoni

Sophoni 是一个桌面端优先的工作区 AI Agent。当前仓库处于 MVP 基础骨架阶段。

## 技术栈

- Tauri 2
- Svelte
- TypeScript
- Rust
- SQLite

## 本地开发

```bash
pnpm install
pnpm dev
```

运行桌面端：

```bash
pnpm tauri dev
```

运行测试：

```bash
pnpm check
pnpm test
cargo test --manifest-path src-tauri/Cargo.toml
```

运行自动验收：

```bash
pnpm accept
```

`pnpm accept` 会执行类型检查、前端测试、Rust 测试，并启动 Web 入口做浏览器级 UI 验收。每次运行都会生成 `.sophoni/runs/<timestamp>/`，其中：

- `report.json`：结构化验收报告，Agent 优先读取 `ok` 和 `failureSummary` 判断状态。
- `events.log`：人类可读的阶段日志。
- `stdout.log` / `stderr.log`：完整命令输出。
- `browser.png`：Web UI 验收截图。

## 配置模型 API Key

Agent 循环通过 OpenAI 兼容协议调用模型，默认 GLM。配置文件位于
`~/.config/sophoni/config.toml`。

**多 Provider 格式（推荐，可在 GLM / MiniMax 间切换）：**

```toml
active = "glm"   # 或 "minimax"

[glm]
api_key = "你的 GLM API Key"
# model 和 base_url 可选，缺省走默认值
# model = "glm-4.6"
# base_url = "https://open.bigmodel.cn/api/paas/v4"

[minimax]
api_key = "你的 MiniMax API Key"
# model = "MiniMax-M3"
# base_url = "https://api.minimax.io/v1"
```

**单 Provider 平铺格式（兼容旧配置）：**

```toml
api_key = "你的 GLM API Key"
model = "glm-4.6"                                       # 可选，默认 glm-4.6
base_url = "https://open.bigmodel.cn/api/paas/v4"       # 可选
```

推荐收紧文件权限：

```bash
chmod 600 ~/.config/sophoni/config.toml
```

启动后，设置页会显示当前 provider 与 model（如 `已配置 (model: glm-4.6)`），
并支持切换命令风险等级（标准 / 宽松 / 完全访问）。

## 当前能力

- 三栏桌面工作台（会话 / 工作区 / 详情）。
- Rust Core Runtime 领域模型 + SQLite 持久化骨架。
- 工作区文件读写、编辑（edit_file）与 diff。
- GLM Agent 循环，支持取消（30s 单轮 / 120s 整体超时）。
- Function Calling 工具集（8 个）：
  - 文件：`read_file` / `write_file` / `edit_file` / `list_files` / `grep`
  - 执行：`run_command`（真实子进程执行，受风险等级管控）
  - 验收观测：`read_acceptance_report` / `read_runtime_log` / `list_acceptance_runs`
- 命令风险三档分级（标准 / 宽松 / 完全访问）+ 高危命令人工确认弹窗。
- 模型输出流式渲染（SSE → 后端 30ms 批量合并 → 前端 rAF 节流，避免 UI 卡顿）。
- 事件流实时推送到前端。
- 多 Provider 配置（GLM / MiniMax），含默认 model / base_url。

## 尚未实现
- macOS Keychain 存储 API Key（目前明文存 TOML，已收紧 0600 权限）。
- 在 UI 中切换 Provider / 编辑 API Key（配置层已支持，设置页仅只读展示）。
- 高级结构索引（代码树索引，加速大工作区检索）。
