# 定时任务设计规格（Scheduled Tasks）

**日期**: 2026-06-17
**关联**: 给 sophoni 加定时自动化能力。应用开着时，每天固定时间自动用预设 prompt 触发 agent 任务。

## 目标

让用户设置"每天 HH:MM 用 prompt X 跑一遍 agent"。到点后 sophoni 自动创建新会话、执行 prompt，用户打开应用能看到对话流和结果。适用于每日验收（`pnpm accept`）、日报生成、定期检查等场景。

## 非目标（明确不做）

- **不做后台守护**：应用关了不跑。用户已确认"应用开着时"就够。不碰 launchd/cron。
- **不做 cron 表达式**：只支持"每天 HH:MM"。以后要 cron 再加字段。
- **不做工作区绑定**：定时任务是全局的，用当前配置的 workspace 跑。
- **不做条件触发**：不评估"只在工作日""只在代码有变更时"等条件。到点就跑。
- **不做跨设备同步**：定时任务存在本地 DB。

## 核心决策（对话中确认）

| # | 决策 | 选择 |
|---|------|------|
| 1 | 运行条件 | 应用开着时定时触发（不需要后台守护） |
| 2 | 任务内容 | 定时跑预设 prompt（让 agent 自主完成），不是指定工具 |
| 3 | 调度模式 | 每天固定时间 HH:MM，不支持 cron |
| 4 | 执行方式 | 自动创建新会话并跑（和手动从 welcome 发消息一样） |
| 5 | 高危命令 | 自动拒绝（无人值守安全第一，不弹窗） |
| 6 | UI 入口 | Sidebar 独立入口（定时任务管理面板） |

## 架构

### 整体流程

```
应用启动 → Scheduler.start() → 加载 enabled 任务 → 每个任务启 tokio task
                                                        ↓
到点 HH:MM → fire(task) → run_agent_task_core(prompt, None, ...)
                            ↓（和手动发消息完全一样的事件流）
                          emit agent-event → 前端收到 conversation_created
                                            → 新会话出现在 Sidebar
                                            → 用户看到对话流和结果
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

### 调度引擎（scheduler.rs，新文件）

```rust
pub struct Scheduler {
    app: AppHandle,                    // 用于 emit 事件
    cancel: Arc<AtomicBool>,           // 共享 AppState 的 cancel
    confirm_pending: Arc<Mutex<HashMap<...>>>,  // 共享，但定时任务用自动拒绝 handler
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
- 构造 `AutoRejectConfirmHandler`（实现 ConfirmHandler，confirm 永远返回 false）
- 构造 `AppEventSink { app }`
- 调 `run_agent_task_core(task.prompt.clone(), None, cancel, handler, sink)`
- `None` = 新会话（和手动从 welcome 发消息一样）

**`reload_task(task_id)`**：停掉旧 tokio task（abort），从 DB 重新加载该任务，如果 enabled 则 spawn 新的。create/update/delete 后调用。

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
        false  // 无人值守，高危命令一律拒绝
    }
}
```

### IPC 命令（lib.rs）

```rust
#[tauri::command]
async fn list_scheduled_tasks(state: State<'_, AppState>) -> Result<Vec<ScheduledTask>, AppError>

#[tauri::command]
async fn create_scheduled_task(state: State<'_, AppState>, prompt: String, hour: u32, minute: u32) -> Result<ScheduledTask, AppError>
// 创建后通知 Scheduler::reload_task(new_id)

#[tauri::command]
async fn update_scheduled_task(state: State<'_, AppState>, id: String, prompt: Option<String>, hour: Option<u32>, minute: Option<u32>, enabled: Option<bool>) -> Result<(), AppError>
// 更新后通知 Scheduler::reload_task(id)

#[tauri::command]
async fn delete_scheduled_task(state: State<'_, AppState>, id: String) -> Result<(), AppError>
// 删除后通知 Scheduler 停掉对应 tokio task
```

Scheduler 需要存进 AppState（或作为独立 State），让 IPC 命令能调 reload。

### 前端

**api.ts：**
```typescript
export async function listScheduledTasks(): Promise<ScheduledTask[]>
export async function createScheduledTask(prompt: string, hour: number, minute: number): Promise<ScheduledTask>
export async function updateScheduledTask(id: string, updates: TaskUpdate): Promise<void>
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
- 从 Sidebar 底部"定时"按钮打开（覆盖层，类似 SettingsPanel）
- 列表：`每天 09:00 · pnpm accept` + 启用/暂停开关 + 编辑 + 删除
- 新建表单：prompt textarea + 时间选择（hour 下拉 0-23 + minute 下拉 0-59 或 `<input type="time">`）+ 保存
- 编辑：同表单，预填现有值

**Sidebar.svelte：**
- 底部加一个"⏰ 定时"按钮，onOpenSchedule 打开 SchedulePanel

**App.svelte：**
- 管理 SchedulePanel 的显示状态（类似 showSettings）
- 定时任务触发时，后端 emit 的 agent-event 流和手动发消息完全一样，前端无需特殊处理

### 触发后的体验

定时任务到点触发时，用户如果看着应用：
1. 自动出现一个新会话（Sidebar 多一项）
2. 对话流里出现 user 气泡（预设 prompt）+ agent 的执行过程 + summary
3. 和手动发消息的体验完全一致

用户如果没看应用（应用在后台）：
1. agent 任务照样跑完，结果存在会话里
2. 用户切回应用时，Sidebar 看到新会话，点开看结果

## 测试策略

### storage.rs
1. `scheduled_task_crud`：创建 → 列表 → 更新 → 删除往返
2. `update_task_last_run`：fire 后更新 last_run_at
3. `list_only_enabled`：过滤 enabled=false（或在 Rust 层过滤）

### scheduler.rs
4. `calculate_next_trigger_today`：当前 10:00，任务 14:00 → 返回今天 14:00
5. `calculate_next_trigger_tomorrow`：当前 15:00，任务 14:00 → 返回明天 14:00
6. `auto_reject_handler_returns_false`：confirm 永远 false

（fire 的端到端测试需要 mock run_agent_task_core 或用 integration test，初版靠手动验证）

## 成功标准

1. **设置任务**：在 SchedulePanel 设置"每天 09:00 · 跑 pnpm accept"，保存。
2. **自动触发**：应用开着，到 09:00 自动创建新会话、执行 prompt。
3. **结果可见**：用户打开应用看到新会话，含执行过程和 summary。
4. **暂停/恢复**：暂停任务后不再触发，恢复后继续。
5. **重启恢复**：应用重启后，任务列表和调度恢复。
6. **安全**：定时任务里 agent 尝试高危命令时自动拒绝。

## 影响文件

**后端新增：**
- `src-tauri/src/core/scheduler.rs`（调度引擎）

**后端修改：**
- `src-tauri/src/core/storage.rs`（migration + CRUD）
- `src-tauri/src/core/domain.rs`（ScheduledTask 类型）
- `src-tauri/src/core/mod.rs`（注册 scheduler 模块）
- `src-tauri/src/lib.rs`（IPC 命令 + AppState 加 Scheduler + 启动时 start）

**前端新增：**
- `src/lib/components/SchedulePanel.svelte`

**前端修改：**
- `src/lib/api.ts`（4 个 IPC 封装）
- `src/lib/types.ts`（ScheduledTask 类型）
- `src/lib/components/Sidebar.svelte`（定时按钮）
- `src/App.svelte`（管理 SchedulePanel 显示）

## 后续计划（明确不做）

- **cron 表达式**：支持"工作日""每周一"等。加 cron 字段 + cron 解析。
- **条件触发**：只在代码有变更时跑、跳过周末等。
- **结果通知**：任务跑完后发系统通知（macOS notification）。
- **跨设备同步**：定时任务云同步。
