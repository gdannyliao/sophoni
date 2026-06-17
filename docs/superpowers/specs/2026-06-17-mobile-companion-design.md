# 手机版伴侣应用

## 背景与动机

桌面版 Sophoni 已能完成完整的 Agent 任务（文件读写、命令执行、验收观测）。但用户离开电脑后无法回顾或继续这些会话——会话数据锁在桌面本机 SQLite 里。

本次新增手机版伴侣应用：手机通过局域网连接桌面，**读取已有会话并续聊**。手机是瘦客户端，桌面是执行器——provider 调用、工具执行、工作区访问全部在桌面端完成，手机复用桌面全部能力。

## 核心决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 数据同步 | 局域网直连 | 零云依赖、改动集中、MVP 最快 |
| 手机能力 | 续聊（桌面代执行） | 手机瘦客户端，桌面复用全部能力（provider/工具/工作区） |
| 传输协议 | HTTP + SSE | 复用现有流式架构，服务端单向推 token 够用 |
| 设备发现 | 桌面显示二维码，手机扫码 | 一次扫码完成地址发现+认证，最顺滑 |
| 设备认证 | 6 位配对码随二维码传递 → 长期 token | 配对码嵌入二维码，扫码即认证；防局域网他人误接 |
| 手机栈 | Tauri Mobile（复用 Svelte） | 代码复用率最高，与现有架构一致 |
| 工具可用性 | 全部工具可用 | 最大化桌面能力，工具由桌面执行 |
| 目录选择 | 手机选桌面工作区 | 手机列出桌面工作区，选中切换激活；文件操作仍在桌面执行 |
| MVP 范围 | 完整功能对齐 | 会话浏览+续聊+工作区切换+变更审查+设置 |

## 架构总览

```
┌─────────────────┐         局域网 (HTTP + SSE)        ┌─────────────────────┐
│   手机端 (瘦)    │  ←──────────────────────────────→  │   桌面端 (执行器)    │
│                 │                                     │                     │
│  Svelte UI      │   扫二维码(IP+端口+配对码) → token   │  Tauri App          │
│  (Tauri Mobile) │   GET /conversations                │  ├─ HTTP 服务层(新)  │
│                 │   GET /conversations/:id            │  ├─ 二维码生成(新)   │
│  HTTP Client    │   POST /chat (SSE 流式 token)       │  ├─ 配对码鉴权(新)   │
│  + token 存储   │   GET /workspaces / PUT active      │  ├─ 现有 agent 循环 │
│  + 摄像头扫码   │   GET /files/:path (审查)           │  ├─ 现有工具调度    │
│                 │                                     │  ├─ SQLite 会话存储  │
│                 │                                     │  └─ 工作区文件系统   │
└─────────────────┘                                     └─────────────────────┘
```

**核心洞察**：桌面端**不重写业务逻辑**，只是在现有 Tauri command 实现上再开一个 HTTP 入口。同一套 agent 循环、工具调度、存储——IPC 走 Tauri command（桌面 UI），HTTP 走局域网 API（手机），两条入口复用同一个内部实现。

---

## 分层设计

本设计分为三个可独立推进的层次，依赖顺序为 第一层 → 第二层 → 第三层。每一层可独立设计、独立验收。

---

## 第一层：桌面端服务层（地基）

工作量最大、最关键的一层。把现有 Tauri command 能力通过 HTTP+SSE 暴露到局域网，含二维码发现与配对码鉴权。

### 1.1 HTTP 服务器（内嵌）

- 用 `axum`（Tokio 生态、轻量、原生支持 SSE 响应）。
- 在 Tauri `setup()` 钩子里用单独的 Tokio task 启动，监听 `0.0.0.0:<随机端口>`。
- 端口随机避免冲突，地址（IP+端口）编码进二维码告知手机。
- 新增依赖：`axum`、`tower`（中间件）、`qrcode`（二维码生成）。

### 1.2 二维码发现

桌面端生成二维码，手机扫码一次性获取连接地址 + 配对码。

- 启动时确定本机局域网 IP（遍历网络接口，取第一个非 loopback 的 IPv4）。
- 二维码内容（自定义 scheme，便于手机端解析）：
  ```
  sophoni://pair?ip=192.168.1.5&port=43210&code=482910
  ```
- 二维码渲染到桌面端的「手机连接」面板（设置页或专属弹窗），用 `qrcode` crate 生成 SVG/PNG，前端 `<img>` 展示。
- 配对码（`code`）随二维码一起轮换：每 60 秒或配对成功后重新生成，二维码随之刷新。
- 同时在二维码旁以明文显示 IP+端口+配对码，便于手机摄像头不可用时手输（降级路径）。

### 1.3 配对码鉴权

- 桌面启动时生成 6 位随机配对码 + 一个长期 token（随机 32 字节十六进制串）。
- 配对码嵌入二维码；token 存内存（重启失效，需重新扫码）。
- **配对流程**：
  1. 桌面端「手机连接」面板显示二维码（含 IP+端口+配对码）。
  2. 手机扫码，解析出地址和配对码。
  3. 手机 `POST /pair { code }` 到扫到的地址 → 校验通过返回 `{ token }`。
  4. 手机存 `{ ip, port, token }`，后续请求带 `Authorization: Bearer <token>`。
- 中间件校验 token：无效或缺失返回 401。配对码校验失败返回 403。
- 配对码一次有效（配对成功后失效，防止二维码被截图重放），token 长期有效（直到桌面重启）。

### 1.4 API 路由

所有路由（除 `/pair`）都走 token 中间件。每条路由复用现有 Tauri command 的内部实现，不重写业务逻辑。

| 路由 | 方法 | 复用现有能力 | 说明 |
|---|---|---|---|
| `/pair` | POST | 新增 | `{ code } → { token }`，唯一不需鉴权的路由 |
| `/conversations` | GET | `list_conversations` | 当前工作区的会话列表 |
| `/conversations/:id` | GET | `get_conversation` | 会话详情（events_json + turns_json） |
| `/chat` | POST (SSE) | `run_agent_task` | 发消息续聊，SSE 流推送 token/thought/round_timing/tool_call/tool_result/summary |
| `/chat/:taskId/cancel` | POST | `cancel_agent_task` | 取消正在执行的任务 |
| `/workspaces` | GET | `list_workspaces` | 桌面所有工作区列表 |
| `/workspaces/active` | GET | `get_workspace_path` | 当前激活工作区 |
| `/workspaces/active` | PUT | `set_workspace_path` | 切换激活工作区 |
| `/files/:path` | GET | 新增（读工作区文件） | ReviewView 读取文件内容 |
| `/config/risk-level` | GET | `get_risk_level` | 当前风险等级 |
| `/config/risk-level` | PUT | `set_risk_level` | 设置风险等级 |

### 1.5 `/chat` 的 SSE 实现（关键复用点）

现有的 `EventSink` trait 是抽象层——IPC 走 `AppEventSink`（emit Tauri 事件），HTTP 走 `SseEventSink`（写 SSE 响应流）。新增一个 sink 实现：

```rust
struct SseEventSink {
    sender: mpsc::Sender<AgentEvent>,  // channel 通往 SSE 响应
}

impl EventSink for SseEventSink {
    fn emit(&self, event: &AgentEvent) {
        // 阻塞式发送（channel 容量足够大，如 256）。绝不丢弃——token 事件丢了
        // 会导致手机端文本不完整。agent 循环本就是串行的，背压时自然限速。
        let _ = self.sender.blocking_send(event.clone());
    }
}
```

> 注意：`blocking_send` 需在同步上下文调用（`EventSink::emit` 是同步方法）。由于 channel 容量大且 agent 循环串行，实际不会长时间阻塞。若担心阻塞 agent 循环，可在 spawn 的 blocking task 里 send。

`/chat` handler：
1. 收到 `{ prompt, conversationId }`。
2. 构造 `SseEventSink` + channel。
3. spawn agent 循环（复用 `run_agent_task`）。
4. 把 channel 接收端转成 SSE 流返回（`axum::response::sse::Sse`）。
5. 每个事件序列化为一帧 `data: {json}\n\n`。

**同一套 agent 循环**，桌面 IPC 和手机 HTTP 各自接自己的 sink，业务逻辑零重复。

### 1.6 工具执行能力

全部工具可用——手机发的 `/chat` 触发的 agent 循环，工具调用（read_file/write_file/run_command 等）照常在桌面工作区执行。结果通过 SSE 的 `tool_call`/`tool_result` 事件返回手机。

### 1.7 错误处理

| HTTP 状态 | 场景 |
|---|---|
| 401 | token 无效/缺失 |
| 403 | 配对码错误 |
| 404 | 未知会话/路由/文件 |
| 409 | 桌面正在执行其他任务（agent 单任务串行） |
| 500 | 内部错误（agent 循环 panic 等） |
| SSE 断流 | 手机端检测到连接中断，提示「连接中断，请重试」 |

### 1.8 不做（YAGNI）

- 多设备并发会话（单任务串行，复用现有 cancel 锁）。
- HTTPS（局域网内部，配对码 + token 足够；后续可加自签证书）。
- token 持久化（重启失效，重新配对成本极低）。
- 服务端推送通知（手机在前台才连）。

### 1.9 第一层验收标准

- 桌面启动后，二维码面板可显示含 IP+端口+配对码的二维码。
- 用 curl/Postman 模拟手机：配对 → 列会话 → 取详情 → 发消息收 SSE 流 → 取消。
- 全部 12 条路由可独立测试，无需手机端。

---

## 第二层：手机端 client

手机与桌面服务层之间的桥，封装 HTTP 通信与配对状态。

### 2.1 配对与连接管理

- **首次配对**：手机打开扫码页（`tauri-plugin-barcode-scanner` 调原生摄像头）→ 扫桌面二维码 → 解析 `sophoni://pair?ip=...&port=...&code=...` → POST `/pair { code }` 到该地址 → 拿 token → 存 `{ ip, port, token }` 到本地存储。
- **后续连接**：自动用存储的 `{ ip, port, token }`，启动时 `GET /workspaces/active` 探活。
- **失效处理**：401 时清空存储，回到扫码配对页。

### 2.2 HTTP Client 封装

- `src/lib/mobile-api.ts`：与桌面端 `api.ts` 平行的实现，底层是 HTTP fetch 而非 Tauri invoke。
- 自动带 `Authorization: Bearer <token>`。
- SSE：`POST /chat` 用 `fetch` + `ReadableStream` 解析（移动端 EventSource 对 POST 支持差）。逐行读 `data: {...}`，反序列化为 `AgentEvent`，回调上层（与桌面端 `onAgentEvent` 语义一致）。

### 2.3 API 模块函数

与桌面端 `api.ts` 暴露相同语义的函数，让第三层 UI 组件几乎无感知：

| 函数 | 桌面端实现 | 手机端实现 |
|---|---|---|
| `runAgentTask(prompt, convId)` | Tauri invoke + listen | `POST /chat` SSE 解析 |
| `listConversations()` | Tauri invoke | `GET /conversations` |
| `getConversation(id)` | Tauri invoke | `GET /conversations/:id` |
| `cancelAgentTask()` | Tauri invoke | `POST /chat/:taskId/cancel` |
| `getWorkspacePath()` | Tauri invoke | `GET /workspaces/active` |
| `listWorkspaces()` | 新增 | `GET /workspaces` |
| `getRiskLevel()` / `setRiskLevel()` | Tauri invoke | `GET/PUT /config/risk-level` |

### 2.4 不做

- 离线缓存（必须连桌面）。
- 断线自动重连（提示用户手动重试即可，MVP 阶段）。

---

## 第三层：手机端 UI

复用现有 Svelte 组件，加平台分流与响应式适配。

### 3.1 平台分流

- `App.svelte` 初始化判断平台（`import.meta.env.TAURI_PLATFORM` 或运行时探测）。
- Mobile → `mobile-api.ts`；Desktop → `api.ts`。
- 配对页仅在 Mobile 首次启动显示。

### 3.2 组件复用与适配

现有组件直接复用，仅加响应式适配：

| 组件 | 桌面 | 手机适配 |
|---|---|---|
| `Sidebar` | 固定左栏 | 抽屉式，默认收起，点汉堡按钮滑出 |
| `Conversation` | 桌面布局 | 触屏增大点击区，移除 hover，输入框贴底 |
| `ReviewView` | 双栏（列表+diff 并排） | 单栏（列表点击进入 diff 详情，返回键回列表） |
| `WelcomeView` | 居中大卡 | 全屏，输入区贴底 |
| `SettingsPanel` | 弹窗 | 全屏页面 |
| `CommandCard` | 展开式 | 默认折叠 stdout/stderr，点击展开 |

### 3.3 移动端特有界面

- **配对页**（`PairingView.svelte`，新增）：摄像头扫码界面 + 连接状态提示（含「重新扫码」「手输地址降级」入口）。
- **工作区切换页**（复用现有工作区列表，抽屉菜单入口）。

### 3.4 不做

- 离线模式（必须连桌面）。
- 后台推送（仅前台流式）。
- 手机本地文件操作（全部桌面执行）。

---

## 成功标准

1. **设备发现**：手机扫桌面二维码即可完成地址发现 + 认证（一次扫码，无需手输 IP）。
2. **配对安全**：配对码错误无法获取 token；token 缺失返回 401。
3. **会话读取**：手机能列出并打开桌面已有会话，看到完整事件流（含流式 token、推理、轮次耗时徽章）。
4. **续聊**：手机发消息能触发桌面 agent 循环，token 实时 SSE 推送，工具调用照常执行并返回结果。
5. **工作区切换**：手机能列出桌面工作区并切换激活。
6. **变更审查**：手机能查看会话的文件变更 diff（diff 数据已含在 conversation 的 events_json 里，无需独立路由）。
7. **向后兼容**：桌面端原有功能（IPC 通道）完全不受影响。
8. **分层独立**：第一层可用 curl 独立验收；三层依赖顺序清晰。

## 实施顺序

1. **第一层**（桌面服务层）→ spec → plan → 实现 → curl 验收。
2. **第二层**（手机 client）→ spec → plan → 实现 → 单元测试。
3. **第三层**（手机 UI）→ spec → plan → 实现 → 真机验收。

每层各自走 spec → plan → 实现循环。本 spec 是总览，三层各自实现时可在本 spec 基础上细化或补充子 spec。
