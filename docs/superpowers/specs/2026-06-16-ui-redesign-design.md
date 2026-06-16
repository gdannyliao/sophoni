# UI 重设计

## 背景与动机

当前 UI 存在两层问题：

- **视觉层（丑）**：210 行手写 CSS，色值硬编码（`#1f6feb` 等），没有设计系统；间距不统一，组件风格扁平缺乏层次。
- **结构层（乱）**：固定三栏布局信息稀疏，大量占位文字；Agent 事件流只做 `{kind}{title}{body}` 裸文本堆叠；命令执行器的 stdout/stderr/exit code 完全没有专门渲染；设置按钮悬浮在右下角位置诡异。

此外，`App.svelte` 第 29 行 `events = result.events` 用最终返回值覆盖了实时推送的事件，丢弃了已有的回合级流式能力——用户看到的是"一片空白然后突然蹦出完整结果"。

本次重设计同时解决视觉和结构两层问题。

## 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 布局 | 双栏 + 嵌入式卡片 | 侧栏可折叠，对话流为主，信息按需展开 |
| 变更展示 | 对话流轻量通知 + 独立审查视图 | 对话流不展开 diff 保持干净，审查时专注 |
| 审查视图结构 | 双栏（文件列表 \| diff/原文件切换） | 左侧导航右侧内容，经典高效布局 |
| 设置入口 | 侧边栏左下角 | 不遮挡内容，与其他开发工具习惯一致 |
| 配色基调 | 暗色主题（GitHub Dark 风格） | 长时间使用不刺眼，diff 高亮对比好 |
| 样式方案 | CSS 变量 + 设计 token | 零依赖，完全控制，保证一致性 |
| 流式范围 | 回合级事件流（打通已有基础） | token 级流式作为后续专项 |

## 视图架构

三个视图，通过 `App.svelte` 的 `view` 状态切换（非路由）：

| 视图 | 状态值 | 触发 | 布局 |
|------|--------|------|------|
| 主对话 | `main` | 默认 | 左栏会话列表 + 右栏对话流 |
| 变更审查 | `review` | 点「查看修改」 | 左栏文件列表 + 右栏 diff/原文件切换 |
| 设置 | 模态弹窗 | 点左下角设置按钮 | 覆盖层弹窗 |

主对话视图的左栏可折叠：展开 220px，折叠 48px 窄条（只显示图标），给对话流更多空间。

## 组件清单

### 重写（4 个）

**`App.svelte`** — 视图状态管理（`view: "main" | "review"`，设置用独立布尔）+ 布局容器。**修复实时事件流**：去掉 `events = result.events` 覆盖，只信任 `onAgentEvent` 实时推送的事件；`result` 仅用于 `fileChanges` 和 `summary`。

**`Sidebar.svelte`** — 会话列表（当前静态）+ 底部模型信息 + 设置按钮。可折叠（展开 220px / 折叠 48px 窄条）。

**`Conversation.svelte`** — 顶部工具栏（任务标题 + 工作区路径 + 「查看修改」按钮含变更数量徽标）+ 对话流（按事件 kind 分发到子组件）+ 底部输入框。

**`SettingsPanel.svelte`** — 暗色模态弹窗，沿用现有 Provider 信息/模型状态逻辑，只换视觉。

### 新建（6 个）

**`MessageBubble.svelte`** — 用户消息气泡（靠右，强调色背景，圆角不对称）。

**`CommandCard.svelte`** — 命令输出卡片。折叠态：图标（✗ 红 / ✓ 绿）+ 命令名 + exit code + 展开箭头。展开态：stdout / stderr 分区显示，截断提示保留（"输出已截断"）。

**`ChangeNotice.svelte`** — 文件变更通知（轻量）。显示图标 + 文件路径 + 状态标签（已修改/新增），不展开 diff，引导文字"在「查看修改」中查看 diff →"。

**`ThoughtLine.svelte`** — Agent 思考行（浅色次要文字，`💭 {title}`）。

**`ReviewView.svelte`** — 变更审查双栏视图。左栏：文件列表（图标 + 路径 + 状态标签 改/新），点击选中。右栏：选中文件的 Diff / 原文件切换 tab + 内容区。底部：复制 diff、确认变更操作。

**`DiffViewer.svelte`** — diff 渲染组件。删除行红底红字（`--danger`），新增行绿底绿字（`--success`），等宽字体，行号可选。

### 删除（1 个）

**`ContextPanel.svelte`** — 三栏右面板取消。文件变更职责移到 ChangeNotice + ReviewView，工具日志职责移到对话流卡片。

## 对话流渲染规则

事件按 `kind` 字段分发到不同组件：

| 事件 kind | 渲染组件 | 说明 |
|-----------|----------|------|
| `thought` | ThoughtLine | 浅色行，`💭 {title}` |
| `tool_call`（run_command） | CommandCard | 创建命令卡片，等待结果填充 |
| `tool_call`（edit_file / write_file） | ChangeNotice | 创建变更通知 |
| `tool_call`（其他工具） | ThoughtLine | 读文件等只读操作，低展示优先级 |
| `tool_result` | 合并到对应 tool_call 卡片 | 命令结果填入 CommandCard（stdout/stderr/exit code） |
| `summary` | 底部结果区 | Agent 最终总结 |
| `error` | 红色错误卡片 | 失败提示 |

**tool_call 与 tool_result 的配对**：两者通过 `tool_call_id` 关联。tool_call 创建卡片，tool_result 填充卡片内容。CommandCard 在收到 result 前显示"运行中..."状态。

## 设计 token 系统

替换当前 210 行硬编码 CSS。所有组件只用 token 变量，不硬编码色值。

```
/* 色彩 */
--bg-primary: #0d1117      /* 主背景 */
--bg-secondary: #161b22    /* 卡片/侧栏背景 */
--bg-tertiary: #21262d     /* 卡片标题栏/hover */
--border: #30363d          /* 边框 */
--text-primary: #c9d1d9    /* 主文字 */
--text-secondary: #8b949e  /* 次要文字 */
--accent: #58a6ff          /* 强调色（链接/选中/用户消息） */
--accent-bg: #1f6feb       /* 强调色背景（用户消息气泡/按钮） */
--success: #3fb950         /* 成功/exit 0/新增行 */
--danger: #f85149          /* 错误/exit 非0/删除行 */
--add-bg: #3fb95011        /* diff 新增行背景 */
--del-bg: #f8514911        /* diff 删除行背景 */

/* 间距（4px 基准） */
--space-1: 4px
--space-2: 8px
--space-3: 12px
--space-4: 16px
--space-6: 24px

/* 圆角 */
--radius-sm: 4px
--radius-md: 8px
--radius-lg: 12px

/* 字体 */
--font-sans: Inter, ui-sans-serif, system-ui, sans-serif
--font-mono: 'SF Mono', 'Cascadia Code', 'JetBrains Mono', monospace
```

token 定义在 `app.css` 的 `:root` 中，所有组件通过 `var(--token-name)` 引用。

## 回合级事件流修复

**当前问题**：

```typescript
// App.svelte 第 26-30 行
unlisten = await onAgentEvent((e) => { events = [...events, e]; });  // 实时推送
const result = await runAgentTask(WORKSPACE_ROOT, task);
events = result.events;  // ← 覆盖！丢弃了实时事件
```

**修复后**：

```typescript
unlisten = await onAgentEvent((e) => { events = [...events, e]; });
const result = await runAgentTask(WORKSPACE_ROOT, task);
// 只取 fileChanges 和 summary，events 已由实时推送维护
fileChanges = result.fileChanges;
summary = result.summary;
```

用户体验：Agent 执行过程中，思考 → 命令调用 → 命令结果 → 文件变更 逐步出现在对话流中，而非等待全部完成后一次性显示。

## 明确不做（YAGNI）

- **Token 级流式（SSE 增量）** — 需要改造 `AgentProvider` trait 签名 + SSE 解析 + 增量 JSON 拼接，作为后续专项 spec。
- **多工作区切换 UI** — 当前硬编码 `/tmp/sophoni`，工作区管理是独立 feature。
- **会话持久化/历史** — 当前会话列表是静态的，持久化需要后端存储支持。
- **高风险命令确认 UI** — 命令执行器规格中的后续计划，需要 IPC 确认往返。
- **亮色主题** — 只做暗色，亮色作为后续可选项。

## 成功标准

1. **视觉一致**：全站无硬编码色值，所有颜色/间距/圆角走 token 变量；暗色主题统一。
2. **对话流可读**：事件按 kind 分层渲染（思考/命令卡片/变更通知），不再是裸文本堆叠。
3. **命令输出专门渲染**：CommandCard 显示 exit code + 可展开 stdout/stderr + 截断提示。
4. **变更审查独立**：点「查看修改」进入双栏审查视图，支持 diff/原文件切换。
5. **实时进度**：修复事件流覆盖，Agent 执行过程中逐步显示进度。
6. **验收通过**：`pnpm accept` ok=true，现有前端测试通过（适配新结构）。
