# sophoni 领域语言

## 工具系统（Tool System）

sophoni 的 agent 通过"工具"操作工作区和外部世界。当前有 13 个工具。

### 核心概念

- **AgentToolName / AgentToolArgs** —— 工具的参数类型事实源（domain.rs 中的 enum）。保留为 enum 形式，因为 `ConversationTurn` 的 DB 序列化（turns_json）依赖其 serde 派生。
- **ToolSpec**（规划中）—— 一个工具的全部行为事实源：schema（给 LLM）、serialize/parse（wire 格式）、dispatch（执行）、describe（事件渲染）、available_in_chat_only（可见性）。每个工具一个 `impl ToolSpec`，持有自己需要的依赖（fs / http_client 等），收拢此前散落在 domain/provider/tools/agent 四文件的 6 处 match。
- **ToolRegistry**（规划中）—— 全部 ToolSpec 的集合（`Vec<Box<dyn ToolSpec>>`）。每次请求构造（因为持有 fs 等会话级依赖）。提供按 name 查找和遍历。消灭 6 个 13 分支大 match。

### 架构决策

1. **保留 AgentToolName/AgentToolArgs enum**：参数类型继续用 enum，消灭它会牵动 storage 层的 turns_json 序列化。enum 是参数事实源，ToolSpec 是行为事实源。
2. **ToolSpec 用 trait + impl**：每工具一个 struct impl ToolSpec trait。不用 fn 指针（async dispatch 别扭）。
3. **不分 ToolSpec 和 ToolContext**：ToolSpec 直接持有自己需要的依赖（fs/http_client 等）。不引入 ToolContext 间接层——registry 本就是每次请求重建（因为 fs 是会话级的），"全局复用"的好处不存在，分离只增加概念负担。

## 重构进度（refactor/toolspec 分支）

**状态：WIP，未完成，未合并 main。**

### 已完成
- `CONTEXT.md` 架构决策（上面 3 条）
- `src-tauri/src/core/tool_spec.rs`：ToolSpec trait + 辅助函数（req_str/opt_str/opt_bool/opt_usize/tool_error/resolve_within_root/find_actual_string/truncate_output/cap_text_bytes 等）+ ReadFileTool 完整 impl

### 待完成（按顺序）
1. **追加 12 个工具 impl 到 tool_spec.rs**：WriteFileTool / ListFilesTool / GrepTool / EditFileTool / MultiEditFileTool / DeleteFileTool / RunCommandTool / ReadAcceptanceReportTool / ReadRuntimeLogTool / ListAcceptanceRunsTool / WebSearchTool / WebFetchTool。每个工具的 6 个面（schema/serialize/parse/describe/dispatch/chat_only）从现有 agent.rs/provider.rs/tools.rs 机械搬移。**3 个集成点**：(a) `command_description` 从 agent.rs 改 pub(crate)；(b) tracing `info!`/`warn!` 需 import；(c) WebSearchTool.search_config 用 `Option<SearchConfig>` 保留未配置错误路径。
2. **build_tool_registry + find_tool**：构造 13 个实例返回 `Vec<Box<dyn ToolSpec>>`；按 name 查找。
3. **provider.rs**：删 tool_call_to_openai / parse_tool_call 的大 match，改用 registry。complete_streaming 接收 registry 参数。
4. **agent.rs**：tool_schemas 改 registry 遍历 + chat_only 过滤；tool_call_event 改 spec.describe；run_agent_task 接收 registry。command_description 改 pub(crate)。
5. **tools.rs**：删 ToolDispatcher + 13 方法（~760 行）。保留 WorkspaceMode + ConfirmHandler。
6. **lib.rs**：build_tool_registry 替代 ToolDispatcher::new。
7. **mod.rs**：注册 pub mod tool_spec。
8. **迁移测试** + pnpm accept + 合并。
