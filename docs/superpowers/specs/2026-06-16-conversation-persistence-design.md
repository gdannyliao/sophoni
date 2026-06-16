# 会话持久化与工作区绑定

## 背景与动机

当前 Agent 产出（events / fileChanges / summary）是纯前端内存状态，刷新即丢。Sidebar 会话列表是 3 个硬编码静态条目。用户无法查看历史会话、无法回顾之前的任务执行过程。

本次新增会话持久化——每次任务自动创建会话记录，事件流存入 SQLite，Sidebar 显示动态会话列表，点击可加载历史。

## 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 会话粒度 | 一个任务 = 一个会话 | 简单直观，和当前"点发送跑一轮"一致 |
| 持久化内容 | 完整事件流（events JSON） | 点击历史会话能重现完整对话 |
| 会话标题 | Agent summary（运行中用会话 ID） | 语义化，跑完才有意义 |
| 工作区关系 | 当前单工作区 → workspaces 表 | config workspace_path 映射到 workspace_id |
| 存储方式 | conversations 表加 events_json 字段 | 扁平 JSON 简单，不启用 task_runs/tool_calls 细表 |
| 多工作区 | 不做 | 后续 spec |

## 数据模型

### 表结构改动

`conversations` 表加 `events_json` 字段：

```sql
ALTER TABLE conversations ADD COLUMN events_json TEXT NOT NULL DEFAULT '[]';
```

完整 conversations 表结构（扩展后）：

| 字段 | 类型 | 说明 |
|------|------|------|
| id | TEXT PK | UUID |
| workspace_id | TEXT FK | 关联 workspaces.id |
| title | TEXT | 会话标题（summary 或 UUID） |
| events_json | TEXT | 完整事件流 JSON 数组 |
| created_at | TEXT | ISO 时间戳 |
| updated_at | TEXT | ISO 时间戳（事件流更新时刷新） |

### 表使用范围

| 表 | 用法 |
|---|------|
| `workspaces` | 已有，存工作区（config workspace_path → workspace_id） |
| `conversations` | 扩展，加 events_json 存完整事件流 |
| `task_runs` / `tool_calls` / `file_changes` | 不用（留未来按需启用） |

## Storage CRUD

新增方法（storage.rs）：

| 方法 | 签名 | 说明 |
|------|------|------|
| `get_or_create_workspace` | `(path: &str) -> AppResult<Workspace>` | 按 path 查找，不存在则创建 |
| `create_conversation` | `(workspace_id: &Uuid, title: &str) -> AppResult<Conversation>` | 创建会话，events_json 为空 |
| `list_conversations` | `(workspace_id: &Uuid) -> AppResult<Vec<Conversation>>` | 按 updated_at 倒序列出 |
| `get_conversation` | `(id: &Uuid) -> AppResult<Conversation>` | 加载单个会话（含 events_json） |
| `update_conversation_events` | `(id: &Uuid, events_json: &str)` | 任务完成后写入事件流 |
| `update_conversation_title` | `(id: &Uuid, title: &str)` | 用 summary 替换临时标题 |

### 迁移

DB 实际从未被 App 使用过（空壳），但 migrate 用 `CREATE TABLE IF NOT EXISTS`。`events_json` 列需要 `ALTER TABLE ADD COLUMN` + 捕获"列已存在"错误（SQLite 没有原生 IF NOT EXISTS for ADD COLUMN）。

## Agent 循环集成

### 任务开始

```
run_agent_task 启动
  → get_or_create_workspace(workspace_path) → workspace_id
  → create_conversation(workspace_id, title=conversation_id) → conversation
  → emit "conversation-created" 事件到前端（含 conversation_id）
```

### 任务运行中

Agent 循环照常 emit `agent-event`（逐条），前端实时更新对话流。事件流不逐条写 DB（性能 + 复杂度），只在任务结束时一次性写入。

### 任务结束

```
Agent 循环结束
  → update_conversation_events(conversation_id, events_json)
  → update_conversation_title(conversation_id, summary)
  → summary 为空时保留 UUID 标题
```

### AppState 改动

`AppState` 加 `storage: Storage` 字段。DB 文件位置：`~/.config/sophoni/sophoni.db`（和 config.toml 同目录）。App 启动时 `Storage::open(db_path)` 初始化。

`run_agent_task` 命令从 `state.storage` 获取 Storage 实例，传入 `run_agent_task_inner`。

### run_agent_task_inner 签名变化

当前签名：
```rust
pub async fn run_agent_task(
    provider: Box<dyn AgentProvider>,
    tools: &ToolDispatcher,
    sink: &dyn EventSink,
    cancel: &AtomicBool,
    system: SystemPrompt,
    user_task: String,
    schemas: Vec<AgentToolSchema>,
) -> AppResult<AgentTaskResult>
```

改为接受 storage + workspace_path：

```rust
pub async fn run_agent_task(
    provider: Box<dyn AgentProvider>,
    tools: &ToolDispatcher,
    sink: &dyn EventSink,
    cancel: &AtomicBool,
    system: SystemPrompt,
    user_task: String,
    schemas: Vec<AgentToolSchema>,
    storage: &Storage,
    workspace_path: &str,
) -> AppResult<AgentTaskResult>
```

## IPC 命令

新增：

| 命令 | 参数 | 返回 | 说明 |
|------|------|------|------|
| `list_conversations` | 无 | `Vec<ConversationSummary>` | 当前工作区的会话列表（id + title + updated_at，不含 events_json） |
| `get_conversation` | `id: String` | `Conversation` | 单个会话（含 events_json） |

`ConversationSummary` 是不含 events_json 的轻量结构（列表不需要事件流数据）。

`list_conversations` 从 config 读 workspace_path → get_or_create_workspace → list_conversations。

## 前端交互

### 类型（types.ts）

```typescript
export interface ConversationSummary {
  id: string;
  title: string;
  updatedAt: string;
}

export interface Conversation extends ConversationSummary {
  eventsJson: string;
}
```

### api.ts

```typescript
export async function listConversations(): Promise<ConversationSummary[]>
export async function getConversation(id: string): Promise<Conversation>
```

### Sidebar 会话列表动态化

- props 加 `conversations: ConversationSummary[]` 和 `activeConversationId: string | null`
- 不再硬编码 3 个静态条目
- 点击会话 → `onSelectConversation(id)`
- 当前运行中的会话高亮（activeConversationId 匹配）

### App.svelte 改动

- 新增 `conversations: ConversationSummary[]` 状态
- 新增 `activeConversationId: string | null` 状态
- onMount：选完工作区后调 `listConversations()` 加载列表
- 监听 `conversation-created` 事件：追加到列表 + 设为 active
- 任务完成：更新对应会话标题（UUID → summary）
- `onSelectConversation(id)`：调 `getConversation(id)` → 解析 eventsJson → 填充 events

### 数据流

```
用户输入任务 → 点发送
  → invoke("run_agent_task", { prompt })
  → 后端创建 conversation → emit conversation-created { id, title: uuid }
  → 前端：追加到 Sidebar 列表 + 高亮
  → Agent 循环 emit agent-event（逐条）→ 前端实时更新对话流
  → 任务完成 → 后端写 events_json + 更新 title=summary
  → 前端：Sidebar 标题从 UUID 更新为 summary

用户点击历史会话
  → invoke("get_conversation", { id })
  → 后端返回 Conversation（含 events_json）
  → 前端：JSON.parse(events_json) → 填充 events/fileChanges/summary
```

## 成功标准

1. **自动创建会话**：发送任务后 Sidebar 立即出现新会话条目（标题=UUID）。
2. **标题更新**：任务完成后 Sidebar 标题从 UUID 更新为 summary。
3. **历史加载**：点击 Sidebar 里的历史会话，对话流显示完整事件历史。
4. **持久化**：重启 App 后 Sidebar 仍显示之前的会话列表。
5. **工作区隔离**：会话关联到当前 workspace_path，不同工作区的会话互不干扰。
6. **验收通过**：`pnpm accept` ok=true。

## 明确不做（YAGNI）

- **多工作区 / 自动发现** — 后续 spec。
- **task_runs / tool_calls / file_changes 表** — 留未来按需启用。
- **会话删除** — 后续。
- **会话重命名** — 后续。
- **跨工作区搜索** — 后续。
- **事件流逐条写 DB** — 只在任务结束时一次性写入。
