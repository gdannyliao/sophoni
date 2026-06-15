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

## 配置 GLM API Key

Agent 循环通过 GLM API 调用模型。创建 `~/.config/sophoni/config.toml`：

```toml
api_key = "你的 GLM API Key"
model = "glm-4.6"                    # 可选，默认 glm-4.6
base_url = "https://open.bigmodel.cn/api/paas/v4"  # 可选
```

推荐收紧文件权限：

```bash
chmod 600 ~/.config/sophoni/config.toml
```

启动后，设置页会显示「已配置 (model: glm-4.6)」。

## 当前能力

- 三栏桌面工作台。
- Rust Core Runtime 基础领域模型。
- SQLite schema 骨架。
- 工作区文件读写和 diff。
- 命令风险分类。
- GLM Agent 循环（read_file / write_file 工具，支持取消和超时）。
- 事件流实时推送到前端。

## 尚未实现
- 真实 Function Calling 工具循环。
- 高级结构索引。
- 真实命令执行器。
- macOS Keychain 设置页。
