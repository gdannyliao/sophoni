# 工作区选择 UI

## 背景与动机

当前工作区路径硬编码在 `App.svelte` 的 `WORKSPACE_ROOT = "/tmp/sophoni"`，用户无法选择真实项目目录。Agent 只能操作临时目录里的文件，实际能力被严重限制。

本次新增工作区选择 UI——用户通过系统目录选择器选择工作区，路径持久化到 config.toml，下次启动自动恢复。切换工作区时清空当前对话（会话持久化是后续 spec）。

## 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 工作区数量 | 单工作区（记住上次） | 覆盖 90% 场景，不做多工作区切换 |
| 路径存储 | config.toml | 和 risk_level、provider 统一，已有读写逻辑 |
| 首次行为 | 引导用户选择 | 未选工作区时禁用输入，引导点"打开工作区" |
| 会话关系 | 切换工作区时清空对话 | 防止上下文错乱，会话持久化留后续 spec |
| 目录选择器 | tauri-plugin-dialog | Tauri 官方插件，跨平台原生对话框 |

## 数据流

### 首次启动（config 无 workspace_path）

```
App 初始化 → invoke("get_workspace_path") → None
  → Sidebar 显示"未选择工作区" + "打开工作区"按钮
  → 对话流输入框禁用，placeholder "请先选择工作区"
  → 用户点"打开工作区" → dialog.open({ directory: true })
  → 返回路径 → invoke("set_workspace_path", { path })
  → config.toml 写入 → App 更新 workspacePath → 输入框启用
```

### 后续启动（config 有 workspace_path）

```
App 初始化 → invoke("get_workspace_path") → Some("/Users/.../project")
  → Sidebar 显示路径 + "切换"按钮
  → 对话流直接可用
```

### 切换工作区

```
用户点"切换" → dialog.open({ directory: true })
  → 返回新路径 → invoke("set_workspace_path", { path })
  → 清空 events / fileChanges / summary
  → 更新 workspacePath
```

## 组件改动

### 后端

| 文件 | 动作 | 内容 |
|------|------|------|
| `Cargo.toml` | 修改 | 加 `tauri-plugin-dialog` 依赖 |
| `domain.rs` | 修改 | `AgentConfig` 加 `workspace_path: Option<String>` |
| `config.rs` | 修改 | 解析 workspace_path（parse_workspace_path）；新增 `save_workspace_path(path)` |
| `lib.rs` | 修改 | 新增 `get_workspace_path()` / `set_workspace_path(path)` IPC；`run_agent_task` 从 config 读 workspace（不再从前端参数接收） |
| `tauri.conf.json` | 可能修改 | dialog 插件权限配置 |

### 前端

| 文件 | 动作 | 内容 |
|------|------|------|
| `package.json` | 修改 | 加 `@tauri-apps/plugin-dialog` |
| `src/lib/api.ts` | 修改 | 加 `getWorkspacePath()` / `setWorkspacePath(path)` |
| `src/lib/components/Sidebar.svelte` | 修改 | 工作区区域：路径显示 + "打开/切换工作区"按钮，用 dialog 选目录 |
| `src/App.svelte` | 修改 | 去掉 `WORKSPACE_ROOT` 硬编码；初始化从后端读 workspacePath；切换时清空对话；未选时禁用输入 |

## run_agent_task 的 workspace 来源

当前 `run_agent_task(workspace_root: String, prompt: String)` 从前端接收 workspace_root。改为后端从 config 读取——前端调用时不传 workspace，后端启动任务时自己读 config 的 workspace_path。

```rust
// 改前
async fn run_agent_task(workspace_root: String, prompt: String) {
    let tools = ToolDispatcher::new(PathBuf::from(&workspace_root));
}

// 改后
async fn run_agent_task(prompt: String) {
    let (config, _) = AgentConfig::load()?;
    let workspace = config.workspace_path
        .ok_or(AppError::Config("未选择工作区".into()))?;
    let tools = ToolDispatcher::new(PathBuf::from(&workspace));
}
```

前端 `runAgentTask(prompt)` 签名相应简化（去掉 workspaceRoot 参数）。

## Sidebar 工作区区域

```
┌─────────────────────┐
│ ◈ Sophoni           │
│                     │
│ 会话                 │
│ ▸ 修复编译错误        │
│ ▸ 跑 git status     │
│                     │
├─────────────────────┤  ← 未选工作区时
│ 未选择工作区          │
│ [📁 打开工作区]       │
│                     │
│ MiniMax-M3          │
│ [⚙ 设置]             │
└─────────────────────┘

┌─────────────────────┐
│ ◈ Sophoni           │
│                     │
│ 会话                 │
│ ▸ 修复编译错误        │
│                     │
├─────────────────────┤  ← 已选工作区时
│ 📁 ~/work/project   │  ← 路径（截断显示）
│ [切换]               │
│                     │
│ MiniMax-M3          │
│ [⚙ 设置]             │
└─────────────────────┘
```

## 切换行为

切换工作区时，App.svelte 执行：
1. `events = []`（清空对话流）
2. `fileChanges = []`（清空文件变更）
3. `summary = ""`（清空摘要）
4. 更新 `workspacePath`（传给 Conversation 的 workspacePath prop）

## 未选工作区时

- Conversation 的输入框 `disabled`，placeholder 显示"请先选择工作区"
- 对话流区域显示提示文字"未选择工作区，点击左侧打开工作区"
- "查看修改"按钮 `disabled`

## 成功标准

1. **选目录**：点"打开工作区"弹出系统目录选择器，选完后 Sidebar 显示路径。
2. **持久化**：选完工作区后重启 App，自动恢复上次选择。
3. **切换清空**：切换工作区时对话流清空，不留旧上下文。
4. **未选禁用**：未选工作区时输入框禁用，引导用户选择。
5. **Agent 在真实目录工作**：选 `/Users/.../real-project` 后，Agent 能读写该目录的文件。
6. **验收通过**：`pnpm accept` ok=true。

## 明确不做（YAGNI）

- **会话持久化** — 对话历史存 SQLite + 工作区绑定，后续 spec。
- **多工作区列表** — 单工作区 + 记住上次。
- **工作区历史** — 不记录最近打开的多个目录。
- **工作区验证** — 不检查是否 git 仓库、是否有 package.json 等。
- **拖拽目录** — 不支持拖拽到窗口选工作区。
