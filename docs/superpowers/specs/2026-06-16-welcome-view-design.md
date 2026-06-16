# 开始页与纯对话模式

## 背景与动机

当前右侧始终显示 Conversation 组件（对话流 + 输入框）。无活跃会话时对话流空白、体验不好；未选工作区时输入框没有明确引导。

本次新增 WelcomeView 开始页——无活跃会话时显示居中的友好开始界面，用户可直接输入任务开始对话。未选工作区时 Agent 以纯对话模式运行（禁用文件/命令工具），选了工作区后切换为全功能模式。

## 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 开始页条件 | 无活跃会话（activeConversationId 为 null） | 简单明确，和会话生命周期一致 |
| 未选工作区行为 | 纯对话模式（Agent 能对话，禁用文件/命令工具） | 不阻断用户，需要操作文件时再选工作区 |
| 工作区选择位置 | WelcomeView 内部卡片（非阻断引导） | 和任务输入在同一视野，引导自然 |
| 最近会话 | 已选工作区且有历史时显示 | 快捷入口，未选工作区不显示（因为会话和工作区绑定） |

## 视图切换

App.svelte 右侧根据 `activeConversationId` 切换：

| 条件 | 视图 |
|------|------|
| `activeConversationId === null` | WelcomeView |
| `activeConversationId !== null` | Conversation |

切换时机：
- 用户在 WelcomeView 输入任务点"开始" → `runDemo` → `conversation_created` 事件设置 activeConversationId → 自动切到 Conversation
- 用户点击历史会话 → `selectConversation` 设置 activeConversationId → 切到 Conversation
- 用户在 Sidebar 点"新对话" → activeConversationId 清空 → 切回 WelcomeView（后续 spec，本次不做新对话按钮）

## WelcomeView 组件

新建 `src/lib/components/WelcomeView.svelte`，居中布局：

```
        ◈
   开始新对话
   副标题（根据模式变化）

  ┌─────────────────────────┐
  │ 描述你想让 Agent 做什么... │  ← 大输入框（textarea）
  │                         │
  └─────────────────────────┘
  [模式标签]            [开始]

  ┌─────────────────────────┐
  │ 📁 工作区状态卡片         │
  └─────────────────────────┘

  最近会话（可选）
  💬 修复编译错误
  💬 更新 README
```

### 工作区状态卡片

| 状态 | 显示 |
|------|------|
| 未选工作区 | "💬 纯对话模式" + 蓝色"选择工作区以启用文件读写和命令执行" + "选择"按钮 |
| 已选工作区 | "✓ 全功能模式" + 当前路径（截断显示） |

### 最近会话

仅在 `workspacePath !== null && conversations.length > 0` 时显示。点击会话项调 `onSelectConversation(id)`。

### props

```typescript
export let workspacePath: string | null = null;
export let conversations: ConversationSummary[] = [];
export let onStart: (prompt: string) => void = () => {};
export let onSelectConversation: (id: string) => void = () => {};
export let onSelectWorkspace: () => void = () => {};
```

## 纯对话模式后端支持

### WorkspaceMode 枚举

新增于 `tools.rs`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMode {
    Full,       // 全功能：所有工具可用
    ChatOnly,   // 纯对话：文件/命令工具禁用
}
```

### ToolDispatcher 改动

`ToolDispatcher` 加 `workspace_mode: WorkspaceMode` 字段。

`dispatch` 方法在 `ChatOnly` 模式下，对以下工具返回错误结果（is_error=true）：
- ReadFile / WriteFile / EditFile / ListFiles / Grep
- RunCommand
- ReadAcceptanceReport / ReadRuntimeLog / ListAcceptanceRuns

错误消息："未选择工作区，此操作不可用。请在左侧选择工作区。"

构造方式：
```rust
ToolDispatcher::new(root)
    .with_risk_level(level)
    .with_confirm_handler(handler)
    .with_workspace_mode(mode)
```

### run_agent_task 改动

`lib.rs` 的 `run_agent_task` 命令：
- config 有 workspace_path → `WorkspaceMode::Full`，workspace = config.workspace_path
- config 无 workspace_path → `WorkspaceMode::ChatOnly`，workspace = 临时空目录（`/tmp/sophoni-chat`）

### SYSTEM_PROMPT 调整

`agent.rs` 的 `system_prompt(level, mode)` 加 mode 参数：
- `ChatOnly`：提示"当前为纯对话模式，文件操作和命令执行不可用。你可以回答问题、生成代码片段、解释概念。如果用户需要文件操作，提示选择工作区。"
- `Full`：现有 prompt 不变

`tool_schemas(level, mode)` 同理——ChatOnly 模式不返回文件/命令工具的 schema（模型根本看不到这些工具）。

## App.svelte 改动

- 去掉右侧始终渲染 `<Conversation>` 的逻辑
- `activeConversationId === null` 时渲染 `<WelcomeView>`
- WelcomeView 的 `onStart` → 调 `runDemo(prompt)`
- WelcomeView 的 `onSelectConversation` → 调 `selectConversation(id)`
- WelcomeView 的 `onSelectWorkspace` → 复用 Sidebar 的 `selectWorkspace` 逻辑（需要提到 App 层或用 event）

工作区选择函数需要从 Sidebar 提到 App 层共享（Sidebar 和 WelcomeView 都能触发）。

## 成功标准

1. **无活跃会话时显示开始页**：打开 App 或无活跃会话时，右侧显示 WelcomeView 而非空白对话流。
2. **纯对话模式**：未选工作区时输入任务能开始对话，Agent 能回答问题但不能操作文件。
3. **全功能模式**：选了工作区后，Agent 能读写文件和执行命令。
4. **工作区引导**：WelcomeView 的工作区卡片引导用户选择工作区。
5. **最近会话入口**：选了工作区后 WelcomeView 显示历史会话快捷入口。
6. **验收通过**：`pnpm accept` ok=true。

## 明确不做（YAGNI）

- **新对话按钮** — 后续 spec（当前通过完成/退出会话回到 WelcomeView）。
- **会话模板/预设任务** — 不做。
- **WelcomeView 动画/过渡** — 不做。
- **多工作区** — 后续 spec。
