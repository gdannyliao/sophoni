# GLM Agent Loop 设计规格

**日期**:2026-06-14
**关联**:承接 `2026-06-13-desktop-agent-foundation.md`(foundation 已完成),把 mock Agent 升级为真 GLM Agent。

## 目标

让 Agent 真用起来:用户在桌面工作台输入任务,前端通过 Tauri command 触发 Rust 端的 Agent 循环,Rust 调用 GLM API,通过 Function Calling 使用 `read_file` 和 `write_file` 两个工具完成"先读后写"的真实编辑任务,事件流实时推给前端,文件变更和 diff 在右栏回显。

## 非目标(明确不做)

- **不做搜索工具**(grep/ripgrep)。只有 `read_file` + `write_file`。
- **不做命令执行**。`command_risk.rs` 这版不接命令执行器。
- **不做结构索引**。
- **不做 Keychain**。API Key 走本地配置文件,明文 + chmod 600,Keychain 留后续计划。
- **不做"打开工作区"UI**。workspace_root 前端硬编码到 `/tmp/sophoni`。
- **不做设置页输入逻辑**。`SettingsPanel` 只读展示 `configured` 状态,不提供写入。
- **不做 GLM 重试 / 流式 / thinking 模式**。Provider 错误直接停止,非流式 chat/completions。
- **不做敏感路径清单**。write_file 只做工作区边界拦截。
- **不做多任务并发**。同一时间一个 Agent 任务,共享一个 cancel flag。

## 核心决策(对话中确认)

| # | 决策 | 选择 |
|---|------|------|
| 1 | 这版范围 | 读写双工具闭环(`read_file` + `write_file`),不做搜索 |
| 2 | Agent 运行位置 | **Rust 后端**。Agent 循环、Provider 调用都在 Rust,API Key 不经过 JS |
| 3 | 循环刹车 | 最大 **12 轮**,单轮超时 **30s**,整体超时 **120s**;超限不抛错,返回已完成步骤 |
| 4 | write_file 安全 | **工作区内静默执行**,越界(符号链接逃逸/`..`)由 `ensure_inside_root` 拦截,工具返回 `is_error` 喂回模型,agent 循环无感知 |
| 5 | 事件流粒度 | **结构化事件**(`thought`/`tool_call`/`tool_result`/`summary`/`error`),通过 Tauri `emit` 实时推 + 返回值兜底同步 |
| 6 | 取消 | 纳入。`cancel_agent_task` 命令 + `AtomicBool` flag,共享于 Tauri `State`,每轮检查 |
| 7 | API Key 存储 | **本地配置文件** `~/.config/sophoni/config.toml`(明文 + chmod 600),不经过 JS |
| 8 | 配置层范围 | **只做 Layer 1**:Rust 启动读取,用户手编辑文件,设置页只读展示。无 `save_config` 命令 |
| 9 | 测试策略 | L1(工具单测)+ L2(Agent 循环单测,注入 FakeProvider),不做 L3 真实 API 集成测试 |

## 架构

### 三层类型分离(关键)

```
┌───────────────────────────────────────────────┐
│ Layer 1: 模型无关领域类型(domain.rs)            │
│   Agent / tools / 前端都用这层                  │
│   加新 Provider 不动这层                        │
└───────────────────────────────────────────────┘
                    ↑ 实现
┌───────────────────────────────────────────────┐
│ Layer 2: AgentProvider trait(provider.rs)      │
│   契约:吃领域类型,吐领域类型                    │
└───────────────────────────────────────────────┘
          ↑ 实现              ↑ 实现
┌──────────────────┐  ┌──────────────────────┐
│ Layer 3a:        │  │ Layer 3b:            │
│ GlmProvider      │  │ 未来:OpenAI/Claude/… │
│ (GLM DTO 内部)   │  │ (各自专属 DTO)       │
└──────────────────┘  └──────────────────────┘
```

**原则**:GLM 专属字段(`finish_reason`、`prompt_tokens`、`role: "tool"` 字符串等)**永远不出现在 `domain.rs`**,只活在 `GlmProvider` 内部的 `glm_dto` 模块里,翻译成领域类型后才交给 Agent 循环。

### 模块结构

新增/改动的文件:

```
src-tauri/src/
├── lib.rs                      [改] 注册新命令,初始化 State
├── core/
│   ├── mod.rs                  [改] 导出新模块
│   ├── domain.rs               [改] 加 ConversationTurn / ToolCall / ProviderResponse / AgentConfig / ToolSchema 等
│   ├── errors.rs               [改] 加 Provider / Config 相关错误变体
│   ├── agent.rs                [改★核心] mock 循环 → 真 Agent 循环
│   ├── provider.rs             [新] AgentProvider trait + GlmProvider + FakeProvider
│   ├── config.rs               [新] 读 ~/.config/sophoni/config.toml
│   ├── tools.rs                [新] read_file / write_file 实现 + ToolDispatcher
│   ├── workspace.rs            [基本不动] 被 tools.rs 复用
│   ├── command_risk.rs         [不动] 本版不接命令执行
│   ├── storage.rs              [基本不动]
│   ├── diff.rs                 [不动]
│   └── tests.rs                [改] 加 L2 循环测试 + FakeProvider 脚本

src/
├── App.svelte                  [改] runDemo 从 mockApi 切到真 api,监听事件流
├── lib/api.ts                  [改] 加 runAgentTask / cancelAgentTask / getConfigStatus / onAgentEvent
├── lib/types.ts                [改] 加 ConfigStatus 类型
├── lib/components/
│   ├── Conversation.svelte     [改] 加取消按钮
│   └── SettingsPanel.svelte    [改] 激活只读状态展示
└── lib/mockApi.ts              [保留] 浏览器开发模式兜底
```

模块依赖方向(无环):

```
lib.rs → agent → provider → domain
              → tools → workspace → domain
              → config
```

### 领域类型(domain.rs 新增)

```rust
// 对话历史的一个条目,模型无关
pub enum ConversationTurn {
    User { content: String },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        result: ToolResult,
    },
}

pub struct SystemPrompt(pub String);

pub enum ToolName { ReadFile, WriteFile }

pub enum ToolArgs {
    Read { path: String },
    Write { path: String, content: String },
}

pub struct ToolCall {
    pub id: String,
    pub name: ToolName,
    pub arguments: ToolArgs,
}

pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
    pub file_change: Option<FileChange>,   // write_file 时 Some,read_file 时 None
}

// Provider 给 Agent 的响应
pub enum ProviderResponse {
    ToolCalls(Vec<ToolCall>),   // 模型要调工具,循环继续
    FinalAnswer(String),         // 最终答案,循环结束
}

pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,   // JSON Schema
}

pub struct AgentConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Serialize)]
pub struct ConfigStatus {
    pub configured: bool,
    pub model: String,
}
```

**既有类型复用**(不改):
- `AgentEvent { kind, title, body }` — 新增 kind 取值 `tool_call` / `tool_result` / `error`,原 `thought` / `summary` 保留。前端 `Conversation.svelte` 渲染逻辑复用,不按 kind 分样式(kind 只影响 `<span>` 里的文字,不影响布局)。
- `FileChange { path, diff, kind }` — write_file 产出。
- `AgentTaskResult { summary, events, file_changes }` — Tauri 命令返回值。

### tool_schemas() 具体定义

```rust
fn tool_schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "read_file",
            description: "读取工作区内指定文件的文本内容。路径相对于工作区根目录。",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" }
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "write_file",
            description: "向工作区内指定文件写入文本内容(覆盖)。路径相对于工作区根目录。",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" },
                    "content": { "type": "string", "description": "要写入的完整文件内容" }
                },
                "required": ["path", "content"]
            }),
        },
    ]
}
```

### 错误模型(errors.rs 扩展)

```rust
pub enum AppError {
    // ... 既有变体保留
    Provider(String),           // GLM 调用失败(HTTP 非 2xx、解析失败、网络)
    Config(String),             // 配置文件解析失败
    ConfigNotConfigured,        // 配置文件不存在或 Key 为空
    Tool(String),               // 工具层系统级错误(workspace 未初始化等)
}
```

**工具的业务错误(文件不存在、越界、写入失败)不抛 Err,返回 `ToolResult { is_error: true }`** 让模型自纠。只有系统级错误才抛 `Err`。

### Agent 循环(agent.rs)

伪代码(完整逻辑):

```rust
const SYSTEM_PROMPT: &str = "你是桌面工作区 Agent。只能操作工作区内文件。必须通过提供的工具(read_file/write_file)操作文件,不要在回复里直接给文件内容。完成任务后给出简短总结。";

pub async fn run_agent_task(
    provider: &mut dyn AgentProvider,
    tools: &ToolDispatcher,
    app_handle: &AppHandle,
    cancel: &AtomicBool,
    workspace_root: PathBuf,
    user_task: String,
) -> Result<AgentTaskResult, AppError> {
    let mut turns: Vec<ConversationTurn> = vec![ConversationTurn::User { content: user_task }];
    let mut events: Vec<AgentEvent> = vec![];
    let mut file_changes: Vec<FileChange> = vec![];
    let max_rounds = 12;
    let per_round_timeout = Duration::from_secs(30);
    let overall_deadline = Instant::now() + Duration::from_secs(120);
    let system = SystemPrompt(SYSTEM_PROMPT.to_string());

    for round in 0..max_rounds {
        // 刹车 1:取消
        if cancel.load(Relaxed) {
            push_event(&mut events, &app_handle, error_event("用户取消了任务"));
            break;
        }
        // 刹车 2:总超时
        if Instant::now() >= overall_deadline {
            push_event(&mut events, &app_handle, error_event("达到整体超时(120s)"));
            break;
        }

        // 调 Provider(带单轮超时)
        let response = tokio::time::timeout(
            per_round_timeout,
            provider.complete(&system, &turns, &tool_schemas()),
        ).await;

        let calls = match response {
            Ok(Ok(ProviderResponse::FinalAnswer(text))) => {
                push_event(&mut events, &app_handle, summary_event(&text));
                break;
            }
            Ok(Ok(ProviderResponse::ToolCalls(calls))) => calls,
            Ok(Err(e)) => {
                push_event(&mut events, &app_handle, error_event(&format!("Provider 错误: {e}")));
                break;
            }
            Err(_elapsed) => {
                push_event(&mut events, &app_handle, error_event("单轮超时(30s)"));
                break;
            }
        };

        // Assistant 的 tool_calls 记进对话历史
        turns.push(ConversationTurn::Assistant { content: None, tool_calls: calls.clone() });

        // 执行每个工具调用
        for call in calls {
            push_event(&mut events, &app_handle, tool_call_event(&call));
            let result = tools.dispatch(&call).await;
            if let Ok(r) = &result {
                if let Some(change) = &r.file_change {
                    file_changes.push(change.clone());
                }
            }
            let result = result.unwrap_or_else(|e| tool_error(&call.id, &e.to_string()));
            push_event(&mut events, &app_handle, tool_result_event(&call, &result));
            turns.push(ConversationTurn::Tool {
                tool_call_id: call.id.clone(),
                result,
            });
        }
    }

    let summary = events.iter().rev()
        .find(|e| e.kind == "summary")
        .map(|e| e.body.clone())
        .unwrap_or_else(|| "任务未正常完成,以上是已执行的步骤。".into());

    Ok(AgentTaskResult { summary, events, file_changes })
}

fn push_event(events: &mut Vec<AgentEvent>, app: &AppHandle, event: AgentEvent) {
    let _ = app.emit("agent-event", &event);
    events.push(event);
}
```

**关键设计点**:

1. **事件双推**:每次 push 都 emit 给前端,任务结束返回值再兜底同步(防事件丢失/乱序)。
2. **三重刹车**:取消、总超时(120s)、单轮超时(30s),都是 break 不是 return Err,保留已完成步骤。
3. **越界拦截在工具层**,agent 循环只看 `ToolResult.is_error`。
4. **system prompt 硬编码**,不可配(MVP 阶段便于迭代)。
5. **`tool_schemas()` 是纯静态函数**,返回 `read_file` / `write_file` 的 JSON Schema 描述。

### Provider(provider.rs)

```rust
#[async_trait]
pub trait AgentProvider: Send {
    async fn complete(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[ToolSchema],
    ) -> Result<ProviderResponse, AppError>;
}

pub struct GlmProvider {
    config: AgentConfig,
    http: reqwest::Client,
}

// GLM 专属 DTO,模块内私有 mod,字段名跟 GLM 官方文档对齐
mod glm_dto {
    struct GlmRequest<'a> { model, messages, tools, tool_choice }
    struct GlmMessage<'a> { role, content, tool_calls?, tool_call_id? }
    struct GlmToolCall { id, type: "function", function: GlmFunction }
    struct GlmFunction { name, arguments: String }   // JSON 字符串
    struct GlmResponse { choices, usage, ... }
    // ...
}
```

`complete()` 流程:
1. 领域类型 → GLM DTO(翻译层,纯函数)
2. `reqwest POST {base_url}/chat/completions`,bearer_auth(api_key)
3. 非 2xx → `AppError::Provider("HTTP {status}: {body}")`
4. 解析 `GlmResponse`
5. GLM DTO → 领域类型(反向翻译)

**翻译函数是模块内私有纯函数,每个都有单测**。

**不重试**:Provider 错误直接返回,agent 循环推 error 事件后停止。

**非流式**:请求不带 `stream: true`,thinking 模式留后续计划。

**FakeProvider**(`#[cfg(test)]`):按预设脚本返回固定 `ProviderResponse`,用于 L2 循环测试。

### 工具层(tools.rs)

```rust
pub struct ToolDispatcher {
    workspace_root: PathBuf,
}

impl ToolDispatcher {
    pub async fn dispatch(&self, call: &ToolCall) -> Result<ToolResult, AppError> {
        match (&call.name, &call.arguments) {
            (ReadFile, ToolArgs::Read { path }) => self.read_file(&call.id, path).await,
            (WriteFile, ToolArgs::Write { path, content }) => self.write_file(&call.id, path, content).await,
            _ => Err(AppError::Tool("工具名与参数不匹配".into())),
        }
    }
    // read_file: ensure_inside_root → tokio::fs::read_to_string
    // write_file: ensure_inside_root → workspace::write_text_with_snapshot(复用 foundation)
}
```

**关键设计点**:

1. **工具业务错误返回 `ToolResult { is_error: true }`**,不抛 Err。模型能"看到"错误并自纠(读不存在文件 → 换路径)。
2. **`ToolResult.file_change: Option<FileChange>`**:write_file 时 Some,read_file 时 None。agent 循环从这里拿 FileChange 累积。
3. **越界拦截复用 `workspace::ensure_inside_root`**,foundation 已有。
4. **ToolDispatcher 无状态**(只有 workspace_root),`&self` 共享,无需锁。

### 配置层(config.rs)

```toml
# ~/.config/sophoni/config.toml
api_key = ""           # 必填
model = "glm-4.6"      # 可选,默认 glm-4.6
base_url = "https://open.bigmodel.cn/api/paas/v4"  # 可选
```

- `AgentConfig::load()`:读文件 → 解析 toml → 校验 api_key 非空 → 返回 `AgentConfig`。文件不存在或 key 空 → `AppError::ConfigNotConfigured`。
- `AgentConfig::status()`:返回 `ConfigStatus { configured, model }`,**不含 Key 本体**(前端永远拿不到 Key)。
- 读取时主动 `chmod 600`(用 `std::os::unix::fs::PermissionsExt`),防止用户创建时权限过宽。
- 文件不存在**不自动创建**(因为没有 `save_config` 命令),README 指引用户手建。

### Tauri 命令(lib.rs)

```rust
struct AppState {
    cancel: Arc<AtomicBool>,
}

#[tauri::command]
async fn run_agent_task(
    state: State<'_, AppState>,
    app: AppHandle,
    workspace_root: String,
    prompt: String,
) -> Result<AgentTaskResult, String>

#[tauri::command]
fn cancel_agent_task(state: State<'_, AppState>)

#[tauri::command]
fn get_config_status() -> ConfigStatus   // AgentConfig::status()
```

**关键点**:
- `run_agent_task` 每次开始前 `cancel.store(false)`,首次调用时 `AgentConfig::load()`(失败返回错误事件)。
- 既有命令保留:`get_app_status`、`classify_command_risk`、`run_mock_task`。
- **错误返回 `String`**,不返回 `AppError`(Tauri 命令边界统一转 String)。
- **`workspace_root` 由前端硬编码传 `/tmp/sophoni`**,真实"打开工作区"是后续计划。

### 前端改动

**`api.ts` 新增**:
```typescript
export async function runAgentTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult>
export async function cancelAgentTask(): Promise<void>
export async function getConfigStatus(): Promise<ConfigStatus>
export function onAgentEvent(cb: (e: AgentEvent) => void): Promise<UnlistenFn>
```

**`App.svelte` 改动**:
- `runDemo` 改为调 `runAgentTask`,开始时 `onAgentEvent` 订阅事件流 push 进 `events`,结束兜底同步返回值。
- `finally` 里 unlisten + `running = false`。

**`Conversation.svelte` 改动**:加「取消」按钮,`disabled={!running}`,调 `cancelAgentTask`。

**`SettingsPanel.svelte` 改动**:挂载时 `getConfigStatus`,只读展示 `configured` 状态和 `model`。输入框不激活。

**`mockApi.ts` 保留**:浏览器开发模式(`pnpm dev` 无 Tauri)兜底用。

**工作区准备**:`/tmp/sophoni/` 放 README.md / package.json 几个测试文件,Agent 在这个沙盒里跑读写闭环,不碰真实代码。

## 测试策略

### L1 工具层单测(tools.rs)

- `read_file` 读到工作区内文件内容
- `read_file` 越界路径返回 `is_error: true`
- `read_file` 不存在的文件返回 `is_error: true`
- `write_file` 写入成功,返回 `file_change: Some` 且 diff 正确
- `write_file` 越界路径返回 `is_error: true`(ensure_inside_root 拦截)

### L2 Agent 循环单测(agent.rs,用 FakeProvider)

- **正常多轮**:FakeProvider 脚本 [Read tool_call, Write tool_call, FinalAnswer] → 验证 events 顺序、file_changes 长度、summary 正确
- **达到最大轮次**:FakeProvider 永远返回 tool_call(脚本 20 条)→ 验证第 12 轮后停止,error 事件包含"最大轮次"
- **取消**:跑到第 3 轮时 `cancel.store(true)` → 验证停止,error 事件包含"取消"
- **单轮超时**:FakeProvider::complete 内部 `tokio::time::sleep(35s)` → 验证 30s 超时触发(测试用更小的超时值注入,避免真等)
- **Provider 错误**:FakeProvider::complete 返回 Err → 验证停止,error 事件包含错误信息
- **越界工具调用**:FakeProvider 返回 read `/etc/passwd` → 验证 tool_result 是 is_error,模型自纠后正常完成

### Provider 翻译函数单测(provider.rs 内部)

- `to_glm_message(ConversationTurn::User)` → role="user"
- `to_glm_message(ConversationTurn::Tool)` → role="tool",tool_call_id 正确
- `translate_response(GlmResponse 带 tool_calls)` → `ProviderResponse::ToolCalls`
- `translate_response(GlmResponse 无 tool_calls)` → `ProviderResponse::FinalAnswer`
- `parse_tool_call("read_file", {path: "x"})` → 正确的 `ToolCall`

### 不做 L3

不写真实 GLM API 集成测试(慢、烧钱、CI 不稳)。手动验收:用户填好 Key,在工作台输入"读 README.md 然后帮我加一行注释",看 Agent 真调 GLM、真改文件、右栏出 diff。

## 成功标准(验收清单)

做完后能完成以下场景:

1. **配置就绪**:`~/.config/sophoni/config.toml` 填好 api_key,设置页显示"已配置,model: glm-4.6"。
2. **正常任务**:输入"读 /tmp/sophoni/README.md,在末尾加一行'Modified by agent'",Agent 调 GLM → read_file → write_file → final summary,中栏事件流实时出现,右栏 diff 显示新增行。
3. **取消**:任务跑到一半点「取消」,Agent 在当前轮结束后停止,error 事件显示"已取消"。
4. **未配置**:删掉 config.toml,跑任务,前端显示"未配置 GLM API Key,请参考 README"。
5. **越界拦截**:L2 测试用 FakeProvider 直接返回越界 tool_call(`write_file` 到 `/etc/passwd`),验证 tool_result 是 `is_error: true`,真实文件系统未被触碰。手动验收层面不强求真实 GLM 触发越界(模型行为不可控)。
6. **测试**:`cargo test`(含 L1 + L2)、`pnpm test`、`pnpm check`、`pnpm build` 全绿。

## 后续计划(明确不做)

- **Keychain**:把 config.toml 的明文 Key 迁移到 macOS Keychain,激活 `SettingsPanel` 输入框。
- **搜索工具**:加 `grep`/`ripgrep`,让 Agent 能"找所有用了 X 的地方"。
- **命令执行器**:接 `command_risk.rs`,让 Agent 能跑 `cargo check` 等。
- **结构索引**:符号级代码理解。
- **打开工作区 UI**:替换前端硬编码的 `/tmp/sophoni`,让用户选真实工作区。
- **GLM 流式 / thinking 模式**:提升响应体验。
- **Provider 重试**:对 429/500 做退避重试。
- **多 Agent 任务并发**:每个任务独立 cancel flag。
