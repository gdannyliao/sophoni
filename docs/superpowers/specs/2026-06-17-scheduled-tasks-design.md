# 定时任务设计规格（Scheduled Tasks）

**日期**: 2026-06-17
**关联**: 给 sophoni 加定时自动化能力。用户通过对话让 agent 设置定时任务，应用开着时每天固定时间自动触发。

## 目标

让用户通过自然语言设置"每天 HH:MM 用 prompt X 跑一遍 agent"——用户说"每天 9 点跑 pnpm accept"，agent 调工具创建定时任务。到点后 sophoni 自动创建新会话、执行 prompt，用户打开应用能看到对话流和结果。

## 非目标（明确不做）

- **不做后台守护**：应用关了不跑。不碰 launchd/cron。
- **不做 cron 表达式**：只支持"每天 HH:MM"。
- **不做工作区绑定**：定时任务是全局的，用当前配置的 workspace 跑。
- **不做条件触发**：不评估"只在工作日""只在代码有变更时"等条件。
- **不做跨设备同步**：定时任务存在本地 DB。

## 核心决策（对话中确认）

| # | 决策 | 选择 |
|---|------|------|
| 1 | 运行条件 | 应用开着时定时触发（不需要后台守护） |
| 2 | 任务内容 | 定时跑预设 prompt（让 agent 自主完成） |
| 3 | 调度模式 | 每天固定时间 HH:MM，不支持 cron |
| 4 | 执行方式 | 自动创建新会话并跑（和手动从 welcome 发消息一样） |
| 5 | 高危命令 | 自动拒绝（无人值守安全第一，不弹窗） |
| 6 | **设置方式** | **用户通过对话设置（agent 调工具），不是手动填表单** |
| 7 | 管理方式 | SchedulePanel UI 查看/暂停/删除（不做新建表单） |

## 架构

### 整体流程

```
用户对话："每天 9 点跑 pnpm accept"
    ↓
agent 调 create_scheduled_task(prompt="跑 pnpm accept", hour=9, minute=0)
    ↓
存 DB + Scheduler spawn tokio task
    ↓
（到点）fire → run_agent_task_core(prompt, None, ...)
    ↓（和手动发消息完全一样的事件流）
emit agent-event → 前端收到 conversation_created → 新会话出现
```

### 数据模型（storage.rs）

```sql
CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id TEXT PRIMARY KEY NOT NULL,
    prompt TEXT NOT NULL,
    hour INTEGER NOT NULL,              -- 0-23
    minute INTEGER NOT NULL,            -- 0-59
    enabled INTEGER NOT NULL DEFAULT 1, -- 0=暂停 1=启用
    last_run_at TEXT,                   -- ISO8601，上次触发时间
    created_at TEXT NOT NULL
);
```

**迁移**：在 `Storage::migrate()` 末尾加 `CREATE TABLE IF NOT EXISTS scheduled_tasks ...`。

**CRUD 方法（storage.rs）：**
- `create_scheduled_task(prompt, hour, minute) -> ScheduledTask`
- `list_scheduled_tasks() -> Vec<ScheduledTask>`
- `update_scheduled_task(id, prompt?, hour?, minute?, enabled?) -> ()`
- `delete_scheduled_task(id) -> ()`
- `update_task_last_run(id, time) -> ()`（fire 后更新）

**领域类型（domain.rs）：**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    pub id: Uuid,
    pub prompt: String,
    pub hour: u32,       // 0-23
    pub minute: u32,     // 0-59
    pub enabled: bool,
    pub last_run_at: Option<String>,  // ISO8601
    pub created_at: String,
}
```

### 定时任务工具（tool_spec.rs，3 个新 ToolSpec）

用户通过对话设置/查看/删除定时任务，agent 调这些工具：

**`create_scheduled_task`**
```json
{
  "name": "create_scheduled_task",
  "description": "创建定时任务。到指定时间自动创建新会话并用 prompt 跑一遍 agent。用户说'每天 X 点做 Y'时用此工具。",
  "parameters": {
    "prompt": "到点要执行的 prompt（必填）",
    "hour": "小时 0-23（必填）",
    "minute": "分钟 0-59（必填）"
  }
}
```
dispatch：存 DB + 通知 Scheduler reload。返回创建的 ScheduledTask（含 id）。

**`list_scheduled_tasks`**
```json
{
  "name": "list_scheduled_tasks",
  "description": "列出所有定时任务。用户问'我有哪些定时任务'时用此工具。",
  "parameters": {}
}
```
dispatch：从 DB 读全部任务，格式化成人类可读列表。

**`delete_scheduled_task`**
```json
{
  "name": "delete_scheduled_task",
  "description": "删除定时任务。用户说'取消每天 X 点的任务'时用此工具。",
  "parameters": {
    "id": "任务 ID（必填，从 list_scheduled_tasks 获取）"
  }
}
```
dispatch：从 DB 删除 + 通知 Scheduler 停掉对应 tokio task。

**工具依赖**：这 3 个工具需要访问 Storage + Scheduler。Storage 可以每次 open（和现有工具访问 fs 一样），Scheduler 通过 `Arc<Scheduler>` 注入。tool_spec.rs 的 `build_tool_registry` 加 `scheduler: Option<Arc<Scheduler>>` 参数。定时任务工具不在 ChatOnly 拦截范围（`available_in_chat_only = true`）。

**system_prompt 补充**：
```
- create_scheduled_task：创建定时任务。用户说"每天 X 点做 Y"时，提取时间（hour/minute）和任务（prompt）创建。
- list_scheduled_tasks：列出所有定时任务。
- delete_scheduled_task：删除定时任务（需先 list 获取 id）。
```

### 调度引擎（scheduler.rs，新文件）

```rust
pub struct Scheduler {
    app: AppHandle,                    // 用于 emit 事件
    cancel: Arc<AtomicBool>,           // 共享 AppState 的 cancel
    handles: Arc<Mutex<HashMap<Uuid, JoinHandle<()>>>>,  // 每个任务一个 tokio task
}
```

**`start()`**：从 DB 加载所有 enabled 任务，为每个调 `spawn_task`。

**`spawn_task(task)`**：
```
let handle = tokio::spawn(async move {
    loop {
        let next = 计算下一次触发时间(task.hour, task.minute);
        let delay = next - now;
        tokio::time::sleep(delay).await;
        fire(task).await;
        update_task_last_run(task.id, now);
    }
});
```

**`fire(task)`**：
- 构造 `AutoRejectConfirmHandler`（confirm 永远返回 false）
- 构造 `AppEventSink { app }`
- 调 `run_agent_task_core(task.prompt.clone(), None, cancel, handler, sink)`
- `None` = 新会话

**`reload_task(task_id)`**：停掉旧 tokio task（abort），从 DB 重新加载该任务，如果 enabled 则 spawn 新的。工具 create/delete 后调用。

**`计算下一次触发时间(hour, minute)`**：
- 取当前时间
- 构造今天的 HH:MM:00
- 如果已过（>= now），返回明天的 HH:MM:00
- 如果没到，返回今天的 HH:MM:00

**自动拒绝 handler：**
```rust
struct AutoRejectConfirmHandler;
#[async_trait]
impl ConfirmHandler for AutoRejectConfirmHandler {
    async fn confirm(&self, _command: &str, _reason: &str) -> bool {
        false
    }
}
```

### IPC 命令（lib.rs）

管理 UI 用（查看/暂停/删除），不走 agent：
```rust
#[tauri::command]
async fn list_scheduled_tasks() -> Result<Vec<ScheduledTask>, AppError>

#[tauri::command]
async fn update_scheduled_task(id: String, enabled: Option<bool>) -> Result<(), AppError>
// 仅暂停/恢复（UI 用），更新后通知 Scheduler::reload_task

#[tauri::command]
async fn delete_scheduled_task(id: String) -> Result<(), AppError>
// 删除后通知 Scheduler 停掉 tokio task
```

注意：`create_scheduled_task` 不需要 IPC 命令——创建走 agent 工具。UI 只管查看/暂停/删除。

Scheduler 存进 AppState，让 IPC 命令和工具都能调 reload。

### 前端

**api.ts：**
```typescript
export async function listScheduledTasks(): Promise<ScheduledTask[]>
export async function updateScheduledTask(id: string, enabled: boolean): Promise<void>
export async function deleteScheduledTask(id: string): Promise<void>
```

**types.ts：**
```typescript
export interface ScheduledTask {
  id: string;
  prompt: string;
  hour: number;
  minute: number;
  enabled: boolean;
  lastRunAt: string | null;
  createdAt: string;
}
```

**SchedulePanel.svelte（新组件）：**
- 从 Sidebar 底部"⏰ 定时"按钮打开（覆盖层，类似 SettingsPanel）
- 列表：`每天 09:00 · 跑 pnpm accept` + 上次触发时间 + 启用/暂停开关 + 删除按钮
- **无新建表单**（新建走对话）。顶部提示"在对话里说'每天 X 点做 Y'来添加定时任务"

**Sidebar.svelte：**
- 底部加"⏰ 定时"按钮

**App.svelte：**
- 管理 SchedulePanel 显示状态（类似 showSettings）

### 触发后的体验

定时任务到点触发时：
1. 自动出现新会话（Sidebar 多一项）
2. 对话流出现 user 气泡（预设 prompt）+ agent 执行过程 + summary
3. 和手动发消息的体验完全一致

## 测试策略

### storage.rs
1. `scheduled_task_crud`：创建 → 列表 → 更新 → 删除往返
2. `update_task_last_run`：fire 后更新 last_run_at

### scheduler.rs
3. `calculate_next_trigger_today`：当前 10:00，任务 14:00 → 返回今天 14:00
4. `calculate_next_trigger_tomorrow`：当前 15:00，任务 14:00 → 返回明天 14:00
5. `auto_reject_handler_returns_false`：confirm 永远 false

（fire 端到端靠手动验证）

## 成功标准

1. **对话设置**：用户说"每天 9 点跑 pnpm accept"，agent 调 create_scheduled_task 创建。
2. **自动触发**：应用开着，到 09:00 自动创建新会话、执行 prompt。
3. **结果可见**：用户看到新会话，含执行过程和 summary。
4. **UI 管理**：SchedulePanel 查看/暂停/删除定时任务。
5. **重启恢复**：应用重启后，任务列表和调度恢复。
6. **安全**：定时任务里 agent 尝试高危命令时自动拒绝。

## 影响文件

**后端新增：**
- `src-tauri/src/core/scheduler.rs`（调度引擎）

**后端修改：**
- `src-tauri/src/core/storage.rs`（migration + CRUD）
- `src-tauri/src/core/domain.rs`（ScheduledTask 类型 + AgentToolName/Args 加 3 个变体）
- `src-tauri/src/core/tool_spec.rs`（3 个定时任务 ToolSpec + build_tool_registry 加 scheduler 参数）
- `src-tauri/src/core/mod.rs`（注册 scheduler 模块）
- `src-tauri/src/lib.rs`（IPC 命令 + AppState 加 Scheduler + 启动时 start + system_prompt 补充）

**前端新增：**
- `src/lib/components/SchedulePanel.svelte`

**前端修改：**
- `src/lib/api.ts`（3 个 IPC 封装）
- `src/lib/types.ts`（ScheduledTask 类型）
- `src/lib/components/Sidebar.svelte`（定时按钮）
- `src/App.svelte`（管理 SchedulePanel 显示）

## 后续计划（明确不做）

- **cron 表达式**：支持"工作日""每周一"等。
- **条件触发**：只在代码有变更时跑、跳过周末等。
- **结果通知**：任务跑完后发系统通知。
- **跨设备同步**：定时任务云同步。
