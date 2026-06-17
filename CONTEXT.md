# sophoni 领域语言

## 工具系统（Tool System）

sophoni 的 agent 通过"工具"操作工作区和外部世界。当前有 13 个工具。

### 核心概念

- **AgentToolName / AgentToolArgs** —— 工具的参数类型事实源（domain.rs 中的 enum）。保留为 enum 形式，因为 `ConversationTurn` 的 DB 序列化（turns_json）依赖其 serde 派生。
- **ToolSpec** —— 一个工具的全部行为事实源：schema（给 LLM）、serialize_args/parse（wire 格式）、describe（事件渲染）、dispatch（执行）、available_in_chat_only（可见性）。每个工具一个 `impl ToolSpec`，持有自己需要的依赖（fs / http_client 等），收拢此前散落在 domain/provider/tools/agent 四文件的 6 处 match。位于 `src-tauri/src/core/tool_spec.rs`。
- **ToolRegistry** —— 全部 ToolSpec 的集合（`Vec<Box<dyn ToolSpec>>`，别名 `ToolRegistry`）。每次请求构造（因为持有 fs 等会话级依赖）。`build_tool_registry` 构造 13 个实例；`find_tool` 按 name 查找；`tool_schemas(registry, mode)` 生成 schema 列表（ChatOnly 过滤）；`dispatch(registry, mode, call)` 聚合执行（含 ChatOnly 拦截）。消灭 6 个 13 分支大 match。

### 架构决策

1. **保留 AgentToolName/AgentToolArgs enum**：参数类型继续用 enum，消灭它会牵动 storage 层的 turns_json 序列化。enum 是参数事实源，ToolSpec 是行为事实源。
2. **ToolSpec 用 trait + impl**：每工具一个 struct impl ToolSpec trait。不用 fn 指针（async dispatch 别扭）。
3. **不分 ToolSpec 和 ToolContext**：ToolSpec 直接持有自己需要的依赖（fs/http_client 等）。不引入 ToolContext 间接层——registry 本就是每次请求重建（因为 fs 是会话级的），"全局复用"的好处不存在，分离只增加概念负担。
4. **Provider 持有 `Arc<ToolRegistry>`**：wire 格式 ↔ AgentToolCall 的转换（serialize_args/parse）是 provider 的职责，provider 构造时接收 registry 并持有。`OpenAICompatibleProvider::new(config, registry)`。
5. **ChatOnly 拦截放在 registry 层**：`tool_spec::dispatch` 统一拦截，单工具不用各自判断。`available_in_chat_only()` 默认 false，网络工具 override 为 true。
6. **run_agent_task 接收 registry + risk_level + mode**：registry 是行为事实源；risk_level/mode 作为独立参数传给 `system_prompt`（动态 prompt 仍需它们），不藏在 registry 里。

## 重构进度（refactor/toolspec 分支）

**状态：已完成，待 pnpm accept 验收 + 合并 main。**

### 已完成
- `CONTEXT.md` 架构决策（上面 6 条）
- `src-tauri/src/core/tool_spec.rs`：ToolSpec trait + 辅助函数 + 13 个工具完整 impl（ReadFile/WriteFile/ListFiles/Grep/EditFile/MultiEditFile/DeleteFile/RunCommand/ReadAcceptanceReport/ReadRuntimeLog/ListAcceptanceRuns/WebSearch/WebFetch）+ ToolRegistry/build_tool_registry/find_tool/tool_schemas/dispatch/wire_name
- `src-tauri/src/core/mod.rs`：注册 `pub mod tool_spec`
- `src-tauri/src/core/provider.rs`：`OpenAICompatibleProvider` 持有 `Arc<ToolRegistry>`；`tool_call_to_openai`/`turn_to_openai_message`/`translate_response`/`parse_tool_call` 改用 registry（删 2 个 13 分支 match）
- `src-tauri/src/core/agent.rs`：`run_agent_task` 接收 `&ToolRegistry` + risk_level + mode；删本地 `tool_schemas` 大函数（改用 `tool_spec::tool_schemas`）；`tool_call_event` 改用 `spec.describe`；`command_description` 改 `pub(crate)`
- `src-tauri/src/core/tools.rs`：删 ToolDispatcher + 13 方法 + 文件级辅助函数（~760 行）。保留 WorkspaceMode + ConfirmHandler + truncate_output + 5 个 truncate 测试
- `src-tauri/src/lib.rs`：`build_tool_registry` 替代 `ToolDispatcher::new`；构造 `Arc<ToolRegistry>` 传给 provider 和 `run_agent_task`
- 测试迁移：provider.rs 14 处 translate_response/turn_to_openai_message 加 registry；agent.rs 7+ 处 run_agent_task 改 13 参数 + registry；tool_spec.rs 新增 30 个工具行为测试（read/write/list/grep/edit/multi_edit/delete/run_command/acceptance/web/chat_only/confirm）

### 验收
- `cargo test --manifest-path src-tauri/Cargo.toml`：175 passed, 0 failed, 5 ignored
- 待跑 `pnpm accept`
