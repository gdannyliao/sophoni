# 命令风险等级与人工确认

## 背景与动机

当前命令执行器的安全模型只有两态：Low（白名单内，直接执行）和 High（直接拒绝）。用户无法调整安全等级——Agent 永远不能执行 `npm install` 等有用的"中等风险"命令，也没有"放行但确认"的中间地带。

本次新增两个互补的能力：

1. **风险等级可选**——用户选择安全等级，`classify_command` 按等级调整判定严格程度。
2. **高风险人工确认**——需要确认的命令不直接拒绝，而是暂停 Agent 循环，弹窗等用户确认。

## 风险等级模型

### 三档等级

新增 `RiskLevel` 枚举（command_risk.rs）：

| 等级 | 名称 | 行为 |
|------|------|------|
| `Standard` | 标准 | 当前白名单行为：白名单内 Allow，其余 Deny |
| `Relaxed` | 宽松 | 更多命令自动放行（install 等），高危命令 RequireConfirm |
| `Unrestricted` | 完全访问 | 对 rm/mv/cp 做路径边界检查，其余放行，极少数致命命令硬拦截 |

### CommandAction 枚举

`classify_command` 的返回值从 `CommandRisk` 改为 `CommandAction`：

```rust
pub enum CommandAction {
    Allow,           // 直接执行
    Deny(String),    // 拒绝（含原因）
    RequireConfirm,  // 需要用户确认
}
```

### 判定矩阵

| 命令 | Standard | Relaxed | Unrestricted |
|------|----------|---------|--------------|
| `cargo test` / 白名单命令 | Allow | Allow | Allow |
| `npm install` / `pnpm install` | Deny | Allow | Allow |
| `git reset --hard` | Deny | RequireConfirm | Allow |
| `rm src/file`（工作区内） | Deny | RequireConfirm | Allow |
| `rm /etc/passwd`（工作区外） | Deny | RequireConfirm | RequireConfirm |
| `rm -rf /`（致命） | Deny | RequireConfirm | Deny（硬拦截） |
| `curl ... \| sh` | Deny | RequireConfirm | RequireConfirm |
| `dd if=/dev/...` | Deny | Deny | Deny（硬拦截） |
| `mkfs` | Deny | Deny | Deny（硬拦截） |
| `sudo ...` | Deny | RequireConfirm | RequireConfirm |

### 各等级判定逻辑

**Standard**（= 当前行为）：
- shell 结构风险（`;`、`&&`、`|` 等）→ Deny
- 白名单前缀匹配 → Allow
- 其余 → Deny

**Relaxed**：
- shell 结构风险 → Deny（注入攻击不可放松）
- 致命模式（`rm -rf /`、`dd`、`mkfs`）→ Deny
- `sudo` → RequireConfirm
- 扩展白名单（install 类命令）→ Allow
- 原白名单 → Allow
- 高危标记（`rm`、`mv`、`curl | sh` 等）→ RequireConfirm
- 其余 → RequireConfirm（保守：不在白名单的都问一下）

**Unrestricted**（完全访问）：
- shell 结构风险 → Deny
- 致命模式 → Deny
- 对 `rm`/`mv`/`cp` 做路径边界检查：
  - 路径可识别且在工作区内 → Allow
  - 路径可识别但在工作区外 → RequireConfirm
  - 路径无法确定（env 变量、`~` 等）→ RequireConfirm
- `sudo` → RequireConfirm
- 其余 → Allow

### 路径边界检查

仅对 `rm`、`mv`、`cp` 三个命令提取路径参数（`shell_words` 的非 flag 参数），用 `lexical_normalize`（已有于 workspace.rs）判断是否在 workspace_root 子树内。

**边界规则**：
- 绝对路径：直接判断是否在 workspace_root 下
- 相对路径：拼接 workspace_root 后 normalize，判断是否还在子树内
- `..` 跳出：normalize 后不在子树内 → 工作区外
- `~` / `$VAR`：无法确定 → RequireConfirm（保守）

**不做路径解析的命令**：`git clean`、`npm install`、`find -delete`、`dd`、`mkfs`、`chmod`、`chown` 等——路径参数位置不固定或不可靠提取，走模式匹配硬拦截或 RequireConfirm。

## 确认回调机制

### 数据流

```
Agent 循环 dispatch(run_command)
  → run_command 调 classify_command → RequireConfirm
  → ToolDispatcher 调 confirm_handler.confirm(command, reason)
  → confirm_handler 通过 IPC emit("command-confirm", { command, reason, request_id })
  → 前端弹 ConfirmDialog，用户点允许/拒绝
  → 前端 invoke("resolve_command_confirm", { request_id, allowed })
  → confirm_handler 收到结果返回 bool
  → true → run_command 继续执行；false → 返回拒绝结果
  → Agent 循环继续
```

### ConfirmHandler trait

```rust
#[async_trait]
pub trait ConfirmHandler: Send + Sync {
    async fn confirm(&self, command: &str, reason: &str) -> bool;
}
```

- `ToolDispatcher` 持有 `Option<Arc<dyn ConfirmHandler>>`
- 没有 handler 时（测试环境），`RequireConfirm` 当作 `Deny` 处理
- 生产环境的 handler 实现：用 `tokio::sync::oneshot` channel 连接 IPC 层
  - `confirm()` 生成唯一 request_id，emit 事件到前端，await receiver
  - IPC `resolve_command_confirm` 命令通过 request_id 找到对应 sender，发送结果
  - 超时 120s 自动拒绝

**Agent 循环不需要改动**——确认逻辑封装在 `run_command` 内部，`dispatch` 的 await 时间变长。

## config 持久化 + 设置面板切换

### config.toml

新增字段：

```toml
risk_level = "standard"  # standard | relaxed | unrestricted
```

- 加在 config.toml 顶层（和 active 同层）
- `AgentConfig::load()` 解析，缺失或无效值回退到 `Standard`
- 默认 `Standard`（向后兼容）

### 后端 IPC 命令

- `set_risk_level(level: String)` — 验证 level 合法，写入 config.toml，更新内存中的当前等级
- `get_risk_level() -> String` — 返回当前等级（设置面板初始化用）

### ToolDispatcher 获取等级

Agent 任务启动时，从 `AgentConfig` 读取 `risk_level`，传给 `ToolDispatcher::new(root, risk_level, confirm_handler)`。一次任务内等级固定（中途改 config 不影响正在运行的任务）。

### 设置面板

SettingsPanel 新增等级选择器：
- 三个选项（标准 / 宽松 / 完全访问），radio 或 segmented control
- 切换时调 `invoke("set_risk_level", { level })`
- 初始化时调 `invoke("get_risk_level")` 读当前值
- 切换后立即生效（下次任务使用新等级）

## 前端确认弹窗

### ConfirmDialog 组件

新建 `src/lib/components/ConfirmDialog.svelte`：
- 模态弹窗，样式与 SettingsPanel 一致（暗色卡片 + 遮罩层）
- 内容：⚠️ 图标 + 命令文本（等宽字体）+ 风险原因 + "允许执行" / "拒绝" 按钮
- 阻塞式：不点不消失，Agent 循环在 await 等待

### App.svelte 改动

- 新增 `pendingConfirm` 状态（`{ requestId, command, reason } | null`）
- 监听 `command-confirm` 事件
- `onResolve(allowed)` → `invoke("resolve_command_confirm", { requestId, allowed })` + 清除状态
- 渲染 ConfirmDialog（当 pendingConfirm 非空时）

### 边界情况

- 用户不响应 → 超时 120s 自动拒绝（后端 confirm_handler 超时返回 false）
- 任务取消时有 pending confirm → 自动拒绝，Agent 循环退出
- 前端未连接（测试环境）→ ToolDispatcher 无 handler，RequireConfirm 当 Deny

## 组件清单

### 后端修改

| 文件 | 动作 | 内容 |
|------|------|------|
| `command_risk.rs` | 修改 | 新增 `RiskLevel`、`CommandAction`；`classify_command` 签名改为接受 `RiskLevel` 返回 `CommandAction`；新增路径边界检查函数 |
| `domain.rs` | 修改 | `AgentConfig` 加 `risk_level: RiskLevel` 字段 |
| `config.rs` | 修改 | `load()` 解析 `risk_level`；新增 `set_risk_level()` 写 config |
| `tools.rs` | 修改 | `ToolDispatcher` 加 `risk_level` + `ConfirmHandler`；`run_command` 用 `CommandAction` |
| `lib.rs` | 修改 | 新增 IPC 命令 `set_risk_level`、`get_risk_level`、`resolve_command_confirm`；emit `command-confirm` 事件 |
| `agent.rs` | 修改 | `run_agent_task` 传入 risk_level + confirm_handler 给 ToolDispatcher |

### 前端修改

| 文件 | 动作 | 内容 |
|------|------|------|
| `src/lib/types.ts` | 修改 | 加 `RiskLevel`、`CommandConfirmRequest` 类型 |
| `src/lib/api.ts` | 修改 | 加 `setRiskLevel`、`getRiskLevel`、`resolveCommandConfirm`、`onCommandConfirm` |
| `src/lib/components/ConfirmDialog.svelte` | 新建 | 确认弹窗 |
| `src/lib/components/SettingsPanel.svelte` | 修改 | 加风险等级选择器 |
| `src/App.svelte` | 修改 | 监听 command-confirm + 渲染 ConfirmDialog |

## 成功标准

1. **三档等级生效**：同一命令在不同等级下得到不同判定（cargo test 始终 Allow；npm install 在 Relaxed/Unrestricted 放行；rm src/ 在 Unrestricted 放行）。
2. **确认弹窗闭环**：Relaxed/Unrestricted 模式下，高危命令触发前端弹窗 → 用户允许则执行 → 拒绝则 Agent 收到拒绝结果并继续。
3. **等级持久化**：设置面板切换等级 → 写入 config.toml → 重启后保持。
4. **路径边界检查**：Unrestricted 模式下，`rm src/file` 放行，`rm /etc/passwd` 需确认，`rm -rf /` 硬拒绝。
5. **向后兼容**：现有 config.toml 不含 risk_level → 默认 Standard → 行为与当前完全一致。
6. **测试覆盖**：command_risk 各等级判定测试 + 路径边界检查测试 + ConfirmHandler 测试。
7. **验收通过**：`pnpm accept` ok=true。

## 明确不做（YAGNI）

- **"本次任务不再询问"** — 复杂且危险（一次放行后续全放行）。
- **命令白名单自定义** — 用户自定义白名单是独立 feature。
- **路径解析覆盖所有命令** — 只对 rm/mv/cp 做，其余走模式匹配。
- **确认历史/审计日志** — 后续可选。
