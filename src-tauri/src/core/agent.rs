use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use chrono::Utc;
use uuid::Uuid;

use super::domain::{
    AgentEvent, AgentToolArgs, AgentToolCall, AgentToolResult, AgentToolSchema, ChangeKind,
    ConversationTurn, FileChange, ProviderResponse, SystemPrompt,
};
use super::errors::AppResult;
use super::provider::AgentProvider;
use super::tools::ToolDispatcher;
use super::workspace::WorkspaceFs;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskResult {
    pub summary: String,
    pub events: Vec<AgentEvent>,
    pub file_changes: Vec<FileChange>,
    /// 本轮累积的完整 turns（含历史 history_turns + 本轮新增），用于持久化到 turns_json。
    pub turns: Vec<ConversationTurn>,
}

const MAX_ROUNDS: usize = 12;

fn command_description(level: super::command_risk::RiskLevel) -> String {
    use super::command_risk::RiskLevel;
    match level {
        RiskLevel::Standard => {
            "在工作区执行安全命令（测试、编译检查、lint 等）。只允许只读命令：cargo test/check/build/clippy、git status/diff/log、ls、rg、tsc、pnpm test/build/check。命令直接执行，不支持管道、重定向或 shell 特殊字符。".to_string()
        }
        RiskLevel::Relaxed => {
            "在工作区执行命令。允许测试、构建、安装依赖（npm/pnpm/cargo install）、git 操作等。高危命令（rm、mv、sudo 等）会请求用户确认后再执行。不支持管道、重定向或 shell 特殊字符。".to_string()
        }
        RiskLevel::Unrestricted => {
            "在工作区执行命令。允许所有命令，包括文件删除（rm）、移动（mv）、复制（cp）、安装依赖等。工作区内路径的 rm/mv/cp 直接执行，工作区外路径会请求用户确认。不支持管道、重定向或 shell 特殊字符。".to_string()
        }
    }
}

fn system_prompt(
    level: super::command_risk::RiskLevel,
    mode: super::tools::WorkspaceMode,
    existing_categories: &[String],
) -> String {
    use super::command_risk::RiskLevel;

    if mode == super::tools::WorkspaceMode::ChatOnly {
        return "你是桌面工作区 Agent。当前为纯对话模式（未选择工作区），文件操作和命令执行不可用。你可以回答问题、生成代码片段、解释概念。遇到不确定的问题时，可以用 web_search 搜索网络、用 web_fetch 读取网页详情。如果用户需要文件操作，提示选择工作区。".to_string();
    }
    let category_rule = if existing_categories.is_empty() {
        "
10. 任务完成后的总结，第一行必须是分类标签，格式 [category: 标签名]。标签用 2-4 个字概括任务类型（如'编译修复'、'依赖管理'、'文档更新'）。从第二行开始写总结。".to_string()
    } else {
        format!("
10. 任务完成后的总结，第一行必须是分类标签，格式 [category: 标签名]。已有类别：{}。优先复用已有类别，只在新任务类型不属于任何已有类别时才创建新类别。标签用 2-4 个字。从第二行开始写总结。", existing_categories.join("、"))
    };

    let (run_cmd_line, file_ops_hint) = match level {
        RiskLevel::Standard => (
            "- run_command：执行安全命令（cargo test、cargo check、git status 等），验证代码改动。",
            "",
        ),
        RiskLevel::Relaxed => (
            "- run_command：执行命令（测试、构建、安装依赖、git 操作等）。高危命令会请求用户确认。",
            "\n10. 用户要求删除/移动/复制文件时，用 run_command 执行 rm/mv/cp 命令。高危操作会弹窗请求用户确认，确认后自动执行。",
        ),
        RiskLevel::Unrestricted => (
            "- run_command：执行命令（测试、构建、文件操作、安装依赖等，工作区内不限）。",
            "\n10. 用户要求删除/移动/复制文件时，用 run_command 执行 rm/mv/cp 命令。工作区内路径直接执行，工作区外路径会请求用户确认。",
        ),
    };
    format!("你是桌面工作区 Agent。只能操作工作区内文件。

可用工具：
- list_files：列出目录内容，了解工作区结构。不确定文件在哪时，先用它探索。
- grep：按正则搜索文件内容。找某个函数/变量/字符串用在哪时用它。
- read_file：读取指定文件内容。
- write_file：写入整个文件（新建或大改时用）。
- edit_file：精确替换文件中的一段文本（小改时用，比 write_file 省 token）。
{run_cmd_line}
- list_acceptance_runs：列出最近验收运行 ID。
- read_acceptance_report：读取验收报告 report.json，可不传 run_id 读取最新一次。
- read_runtime_log：读取验收运行日志的尾部内容，可不传 run_id 读取最新一次。
- web_search：搜索网络。遇到未知报错、陌生 API、不确定的用法时，先搜索而不是猜。
- web_fetch：读取网页内容。web_search 找到线索后，用它读取详情。

工作方式：
1. 不确定路径时，先 list_files 或 grep 探索。
2. 改文件前，先用 read_file 看当前内容。
3. 小改动优先用 edit_file（给出要替换的原文和新文本），大改动或新建文件用 write_file。
4. edit_file 的 old_string 必须与文件内容精确匹配（含缩进和空格）。
5. 当用户要求替换「所有」或「全部」时，用 edit_file 的 replace_all=true，一次替换所有匹配，不要分多次单独替换。
6. 改完代码后，用 run_command 跑 cargo check 或 cargo test 验证改动是否正确。如果命令失败，读 stderr 定位问题并修正。
7. 验收时优先用 read_acceptance_report 看 report.json，重点检查 ok 和 failureSummary；失败或信息不足时再用 read_runtime_log 查看相关日志。
8. 不要在回复里直接给文件内容，通过工具操作。
9. 完成任务后给出简短总结。{file_ops_hint}")
}

/// 去掉模型输出中的 <think>...</think> 思维链标签（MiniMax-M3 等模型会输出）。
pub fn strip_think_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_think = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<think>") {
            in_think = true;
            let after = trimmed.strip_prefix("<think>").unwrap_or("");
            if after.contains("</think>") {
                let clean = after.split("</think>").nth(1).unwrap_or("");
                if !clean.trim().is_empty() {
                    result.push_str(clean.trim());
                    result.push('\n');
                }
                in_think = false;
            }
            continue;
        }
        if in_think {
            if trimmed.contains("</think>") {
                in_think = false;
                let after = trimmed.split("</think>").nth(1).unwrap_or("");
                if !after.trim().is_empty() {
                    result.push_str(after.trim());
                    result.push('\n');
                }
            }
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result.trim_end_matches('\n').to_string()
}

pub fn parse_category(text: &str) -> (Option<String>, String) {
    let first_line = text.lines().next().unwrap_or("");
    if let Some(rest) = first_line.trim().strip_prefix("[category:") {
        let category = rest.trim_end_matches(']').trim().to_string();
        if !category.is_empty() {
            let clean = text.lines().skip(1).collect::<Vec<_>>().join("\n");
            return (Some(category), clean.trim().to_string());
        }
    }
    (None, text.to_string())
}

pub fn build_memory_context(memories: &[super::domain::ConversationMemory]) -> String {
    use std::collections::BTreeMap;
    if memories.is_empty() {
        return String::new();
    }
    let mut by_category: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for m in memories {
        let cat = m.category.as_deref().unwrap_or("其他");
        by_category.entry(cat.to_string()).or_default().push(&m.summary);
    }
    let mut text = String::from("## 历史任务记忆\n\n");
    for (category, summaries) in &by_category {
        text.push_str(&format!("### {category}\n"));
        for s in summaries {
            text.push_str(&format!("- {s}\n"));
        }
        text.push('\n');
    }
    if let Some(last) = memories.last() {
        text.push_str(&format!("### 最近任务\n- {}\n", last.summary));
    }
    text
}

const PER_ROUND_TIMEOUT: Duration = Duration::from_secs(30);
const OVERALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Abstraction over "where events go". In production this emits via Tauri's
/// AppHandle; in tests it collects into a buffer. Keeps the loop testable
/// without a Tauri runtime. `Sync` is required so `&dyn EventSink` is `Send`
/// and can cross await points in the async loop.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &AgentEvent);
}

pub async fn run_agent_task(
    mut provider: Box<dyn AgentProvider>,
    tools: &ToolDispatcher,
    sink: &dyn EventSink,
    cancel: &AtomicBool,
    _system: SystemPrompt,
    user_task: String,
    _schemas: Vec<AgentToolSchema>,
    conversation_id: uuid::Uuid,
    memory_context: String,
    existing_categories: Vec<String>,
    history_turns: Vec<ConversationTurn>,
) -> AppResult<AgentTaskResult> {
    let task_start = Instant::now();
    info!(prompt = %user_task, "agent task started");
    let system = SystemPrompt(system_prompt(tools.risk_level(), tools.workspace_mode(), &existing_categories));
    // 续聊：先继承历史 turns，模型能看到同会话之前的完整对话。
    // 新会话时 history_turns 为空，等价于从零开始。
    let mut turns: Vec<ConversationTurn> = history_turns;
    if !memory_context.is_empty() {
        turns.push(ConversationTurn::Assistant {
            content: Some(memory_context.clone()),
            tool_calls: vec![],
        });
    }
    turns.push(ConversationTurn::User { content: user_task.clone() });
    let mut events: Vec<AgentEvent> = vec![];
    let mut file_changes: Vec<FileChange> = vec![];
    let schemas = tool_schemas(tools.risk_level(), tools.workspace_mode());
    let deadline = Instant::now() + OVERALL_TIMEOUT;

    // emit conversation_created 让前端立即更新 Sidebar（复用会话时前端靠 id 去重）
    push(
        &mut events,
        sink,
        AgentEvent {
            kind: "conversation_created".into(),
            title: conversation_id.to_string(),
            body: conversation_id.to_string(),
            tool_call_id: None,
        },
    );
    // emit 用户消息事件，前端渲染用户气泡（连续会话可见每条输入）
    push(
        &mut events,
        sink,
        AgentEvent {
            kind: "user".into(),
            title: "用户".into(),
            body: user_task,
            tool_call_id: None,
        },
    );

    for round in 0..MAX_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            warn!("agent: 用户取消");
            push(&mut events, sink, error_event("用户取消了任务"));
            break;
        }
        if Instant::now() >= deadline {
            warn!("agent: 达到整体超时(120s)");
            push(&mut events, sink, error_event("达到整体超时(120s)"));
            break;
        }

        // 流式回调：token 实时 emit 给前端，同时累积到 round_text 供本轮结束时判断
        // 是否是「推理过程」（ToolCalls 轮里模型的 reasoning 文本）。用 Arc<Mutex<String>>
        // 让闭包满足 Send+Sync（TokenSink 要求），锁开销可忽略（后端已 30ms 批量）。
        let round_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let round_text_for_cb = round_text.clone();
        let sink_ref: &dyn EventSink = sink;
        let on_token = move |delta: &str| {
            sink_ref.emit(&AgentEvent {
                kind: "token".into(),
                title: "assistant".into(),
                body: delta.to_string(),
                tool_call_id: None,
            });
            if let Ok(mut acc) = round_text_for_cb.lock() {
                acc.push_str(delta);
            }
        };

        let round_start = Instant::now();
        let response = tokio::time::timeout(
            PER_ROUND_TIMEOUT,
            provider.complete_streaming(&system, &turns, &schemas, &on_token),
        )
        .await;
        let round_elapsed_ms = round_start.elapsed().as_millis();
        let round_text_val = round_text.lock().map(|s| s.clone()).unwrap_or_default();
        // 后端日志：每轮耗时，便于定位延迟瓶颈。
        info!(round = round + 1, elapsed_ms = round_elapsed_ms, "agent round");

        let calls = match response {
            Ok(Ok(ProviderResponse::FinalAnswer(text))) => {
                let clean = strip_think_tags(&text);
                emit_round_timing(&mut events, sink, round + 1, round_elapsed_ms, "最终答案");
                push(&mut events, sink, summary_event(&clean));
                // 必须把最终回复记进 turns，否则续聊时历史会缺失 assistant 回复，
                // 导致下一轮 user 消息直接拼在上一轮 user 后面（模型看到连续两条 user 消息）。
                turns.push(ConversationTurn::Assistant {
                    content: Some(clean),
                    tool_calls: vec![],
                });
                break;
            }
            Ok(Ok(ProviderResponse::ToolCalls(calls))) => {
                let call_count = calls.len();
                // 工具调用轮：若有累积的推理文本，落定为 thought 事件（前端淡色展示）。
                if !round_text_val.is_empty() {
                    push(
                        &mut events,
                        sink,
                        AgentEvent {
                            kind: "thought".into(),
                            title: "推理".into(),
                            body: round_text_val,
                            tool_call_id: None,
                        },
                    );
                }
                emit_round_timing(&mut events, sink, round + 1, round_elapsed_ms, &format!("工具调用×{call_count}"));
                calls
            }
            Ok(Err(e)) => {
                emit_round_timing(&mut events, sink, round + 1, round_elapsed_ms, "Provider 错误");
                push(
                    &mut events,
                    sink,
                    error_event(&format!("Provider 错误: {e}")),
                );
                break;
            }
            Err(_elapsed) => {
                emit_round_timing(&mut events, sink, round + 1, PER_ROUND_TIMEOUT.as_millis(), "单轮超时");
                warn!(round = round + 1, "agent: 单轮超时(30s)");
                push(&mut events, sink, error_event("单轮超时(30s)"));
                break;
            }
        };

        turns.push(ConversationTurn::Assistant {
            content: None,
            tool_calls: calls.clone(),
        });

        for call in calls {
            push(&mut events, sink, tool_call_event(&call));
            let result = tools.dispatch(&call).await;
            let result = match result {
                Ok(r) => r,
                Err(e) => tool_error_result(&call.id, &e.to_string()),
            };
            if let Some(change) = &result.file_change {
                file_changes.push(change.clone());
            }
            push(&mut events, sink, tool_result_event(&call, &result));
            turns.push(ConversationTurn::Tool {
                tool_call_id: call.id.clone(),
                result,
            });
        }
    }

    // If we exited the loop without a FinalAnswer and without an explicit
    // error event, surface the max-rounds stop as an error so the user sees why.
    if !events.iter().any(|e| e.kind == "summary") && !events.iter().any(|e| e.kind == "error") {
        push(&mut events, sink, error_event("达到最大轮次(12),已停止"));
    }

    let summary = events
        .iter()
        .rev()
        .find(|e| e.kind == "summary")
        .map(|e| e.body.clone())
        .unwrap_or_else(|| "任务未正常完成,以上是已执行的步骤。".into());

    info!(total_ms = task_start.elapsed().as_millis(), "agent task done");

    Ok(AgentTaskResult {
        summary,
        events,
        file_changes,
        turns,
    })
}

fn push(events: &mut Vec<AgentEvent>, sink: &dyn EventSink, event: AgentEvent) {
    sink.emit(&event);
    events.push(event);
}

/// emit 一条轮次耗时事件（kind="round_timing"），前端渲染成徽章。同时本函数不写
/// 后端日志——日志在循环内用 eprintln! 输出，避免重复。
fn emit_round_timing(
    events: &mut Vec<AgentEvent>,
    sink: &dyn EventSink,
    round: usize,
    elapsed_ms: u128,
    result_label: &str,
) {
    push(
        events,
        sink,
        AgentEvent {
            kind: "round_timing".into(),
            title: format!("轮次{round}"),
            body: format!("{elapsed_ms}ms · {result_label}"),
            tool_call_id: None,
        },
    );
}

fn tool_schemas(level: super::command_risk::RiskLevel, mode: super::tools::WorkspaceMode) -> Vec<AgentToolSchema> {
    // 网络工具在所有模式都可用（含 ChatOnly）
    let web_schemas: Vec<AgentToolSchema> = vec![
        AgentToolSchema {
            name: "web_search",
            description: "搜索网络获取外部信息。遇到未知报错、陌生 API、版本兼容性问题时使用。返回标题、摘要和 URL 列表。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索关键词" },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": 10, "description": "返回条数，默认 5" }
                },
                "required": ["query"]
            }),
        },
        AgentToolSchema {
            name: "web_fetch",
            description: "抓取指定 URL 的网页内容并转为文本。用于读取 web_search 找到的页面详情（文档、Stack Overflow 答案、GitHub issue 等）。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要抓取的完整 URL（http/https）" },
                    "max_chars": { "type": "integer", "minimum": 500, "maximum": 50000, "description": "返回的最大字符数，默认 8000" }
                },
                "required": ["url"]
            }),
        },
    ];
    if mode == super::tools::WorkspaceMode::ChatOnly {
        return web_schemas;
    }
    let mut schemas = vec![
        AgentToolSchema {
            name: "read_file",
            description: "读取工作区内指定文件的文本内容。路径相对于工作区根目录。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" }
                },
                "required": ["path"]
            }),
        },
        AgentToolSchema {
            name: "write_file",
            description: "向工作区内指定文件写入文本内容(覆盖)。路径相对于工作区根目录。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" },
                    "content": { "type": "string", "description": "要写入的完整文件内容" }
                },
                "required": ["path", "content"]
            }),
        },
        AgentToolSchema {
            name: "list_files",
            description: "列出工作区内指定目录的文件和子目录。默认只列直接子项（不递归）。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的目录路径，默认为工作区根" },
                    "recursive": { "type": "boolean", "description": "是否递归列出子目录，默认 false" }
                }
            }),
        },
        AgentToolSchema {
            name: "grep",
            description: "在工作区内搜索匹配正则表达式的文件内容。返回 path:line:content 格式的结果。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "正则表达式" },
                    "path": { "type": "string", "description": "限定搜索的目录或文件，默认整个工作区" },
                    "include": { "type": "string", "description": "文件名 glob 过滤，如 *.ts" }
                },
                "required": ["pattern"]
            }),
        },
        AgentToolSchema {
            name: "edit_file",
            description: "对已有文件做精确文本替换(search-replace)。先 read_file 看准内容,再给出 old_string(必须与文件内容精确匹配,含缩进)和 new_string。old_string 必须在文件中唯一,除非 replace_all=true。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" },
                    "old_string": { "type": "string", "description": "要替换的文本(精确匹配)" },
                    "new_string": { "type": "string", "description": "替换成的文本(必须与 old_string 不同)" },
                    "replace_all": { "type": "boolean", "description": "当 old_string 在文件中出现多次且你想全部替换时设为 true(例如用户要求替换'所有'/'全部'时)。默认 false 时 old_string 必须在文件中唯一。" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        AgentToolSchema {
            name: "run_command",
            description: command_description(level),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的命令（如 cargo test）" }
                },
                "required": ["command"]
            }),
        },
        AgentToolSchema {
            name: "read_acceptance_report",
            description: "读取验收运行的 report.json。默认读取最新一次验收运行；用于判断 ok 和 failureSummary。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "验收运行 ID；省略时读取最新一次" }
                }
            }),
        },
        AgentToolSchema {
            name: "read_runtime_log",
            description: "读取验收运行日志尾部内容。默认读取最新一次验收运行；max_lines 默认由模型调用方可省略，建议先读少量行。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "验收运行 ID；省略时读取最新一次" },
                    "file_name": { "type": "string", "description": "日志文件名，例如 runtime.log" },
                    "max_lines": { "type": "integer", "minimum": 1, "maximum": 200, "description": "读取尾部行数，默认 80" }
                },
                "required": ["file_name"]
            }),
        },
        AgentToolSchema {
            name: "list_acceptance_runs",
            description: "列出最近验收运行 ID。limit 会限制到 1 到 20。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 20, "description": "返回条数，默认 5" }
                }
            }),
        },
    ];
    schemas.extend(web_schemas);
    schemas
}

fn error_event(body: &str) -> AgentEvent {
    AgentEvent {
        kind: "error".into(),
        title: "错误".into(),
        body: body.into(),
        tool_call_id: None,
    }
}

fn summary_event(body: &str) -> AgentEvent {
    AgentEvent {
        kind: "summary".into(),
        title: "任务完成".into(),
        body: body.into(),
        tool_call_id: None,
    }
}

fn tool_call_event(call: &AgentToolCall) -> AgentEvent {
    let (label, detail, body) = match &call.arguments {
        AgentToolArgs::Read { path } => ("read_file", path.clone(), format!("path: {path}")),
        AgentToolArgs::Write { path, content } => (
            "write_file",
            path.clone(),
            format!(
                "path: {path}\ncontent ({} 行):\n{}",
                content.lines().count().max(1),
                content
            ),
        ),
        AgentToolArgs::ListFiles { path, recursive } => {
            let p = path.as_deref().unwrap_or(".");
            (
                "list_files",
                format!("{p} (recursive={recursive})"),
                format!("path: {p}\nrecursive: {recursive}"),
            )
        }
        AgentToolArgs::Grep {
            pattern,
            path,
            include,
        } => {
            let p = path.as_deref().unwrap_or(".");
            let inc = include.as_deref().unwrap_or("(无)");
            (
                "grep",
                format!("/{pattern}/ in {p}"),
                format!("pattern: {pattern}\npath: {p}\ninclude: {inc}"),
            )
        }
        AgentToolArgs::EditFile {
            path,
            old_string,
            new_string,
            replace_all,
        } => {
            let old_preview = old_string.lines().take(3).collect::<Vec<_>>().join("\n");
            let old_suffix = if old_string.lines().count() > 3 {
                "\n..."
            } else {
                ""
            };
            (
                "edit_file",
                format!("{} (replace_all={})", path, replace_all),
                format!(
                    "path: {}\nreplace_all: {}\nold_string:\n{}{}\nnew_string ({} 行):",
                    path,
                    replace_all,
                    old_preview,
                    old_suffix,
                    new_string.lines().count().max(1)
                ),
            )
        }
        AgentToolArgs::ReadAcceptanceReport { run_id } => {
            let id = run_id.as_deref().unwrap_or("latest");
            (
                "read_acceptance_report",
                id.to_string(),
                format!("run_id: {id}"),
            )
        }
        AgentToolArgs::ReadRuntimeLog {
            run_id,
            file_name,
            max_lines,
        } => {
            let id = run_id.as_deref().unwrap_or("latest");
            (
                "read_runtime_log",
                format!("{file_name} ({id}, max_lines={max_lines})"),
                format!("run_id: {id}\nfile_name: {file_name}\nmax_lines: {max_lines}"),
            )
        }
        AgentToolArgs::ListAcceptanceRuns { limit } => (
            "list_acceptance_runs",
            format!("limit={limit}"),
            format!("limit: {limit}"),
        ),
        AgentToolArgs::RunCommand { command } => {
            ("run_command", command.clone(), format!("command: {command}"))
        }
        AgentToolArgs::WebSearch { query, max_results } => {
            ("web_search", query.clone(), format!("query: {query}\nmax_results: {max_results}"))
        }
        AgentToolArgs::WebFetch { url, max_chars } => {
            ("web_fetch", url.clone(), format!("url: {url}\nmax_chars: {max_chars}"))
        }
    };
    AgentEvent {
        kind: "tool_call".into(),
        title: format!("{label}: {detail}"),
        body,
        tool_call_id: Some(call.id.clone()),
    }
}

fn tool_result_event(call: &AgentToolCall, result: &AgentToolResult) -> AgentEvent {
    AgentEvent {
        kind: "tool_result".into(),
        title: format!("结果: {}", call.id),
        body: if result.is_error {
            format!("失败: {}", result.content)
        } else {
            result.content.clone()
        },
        tool_call_id: Some(call.id.clone()),
    }
}

fn tool_error_result(call_id: &str, message: &str) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: message.to_string(),
        is_error: true,
        file_change: None,
    }
}

// ── mock agent (kept for browser dev mode compatibility) ──

pub fn run_mock_agent_task(workspace_root: PathBuf, prompt: &str) -> AppResult<AgentTaskResult> {
    let fs = WorkspaceFs::new(workspace_root.clone());
    let target = workspace_root.join("README.md");
    let target_existed = target.exists();
    let next_text = format!("# Sophoni\n\nMock task completed for: {}\n", prompt);
    let write = fs.write_text_with_snapshot(&target, &next_text)?;

    let task_id = Uuid::new_v4();
    let change = FileChange {
        id: Uuid::new_v4(),
        task_run_id: task_id,
        path: "README.md".to_string(),
        kind: if target_existed {
            ChangeKind::Modified
        } else {
            ChangeKind::Created
        },
        diff: write.diff,
        created_at: Utc::now(),
    };

    Ok(AgentTaskResult {
        summary: "mock Agent 已完成一次文件写入任务。".to_string(),
        events: vec![
            AgentEvent {
                kind: "thought".into(),
                title: "理解任务".into(),
                body: prompt.to_string(),
                tool_call_id: None,
            },
            AgentEvent {
                kind: "tool".into(),
                title: "写入 README.md".into(),
                body: "已写入 README.md 并生成 diff。".into(),
                tool_call_id: None,
            },
            AgentEvent {
                kind: "summary".into(),
                title: "任务完成".into(),
                body: "mock Agent 已生成可展示的文件变更。".into(),
                tool_call_id: None,
            },
        ],
        file_changes: vec![change],
        turns: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::super::domain::{AgentToolSchema, ChangeKind, ConversationTurn, ProviderResponse, SystemPrompt};
    use super::super::provider::{fake_read_call, fake_write_call, FakeProvider};
    use super::super::tools::ToolDispatcher;
    use super::{run_agent_task, run_mock_agent_task, EventSink};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct CollectingSink {
        events: Mutex<Vec<super::AgentEvent>>,
    }

    impl CollectingSink {
        fn new() -> Self {
            Self {
                events: Mutex::new(vec![]),
            }
        }
        fn snapshot(&self) -> Vec<super::AgentEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl EventSink for CollectingSink {
        fn emit(&self, event: &super::AgentEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    fn empty_schemas() -> Vec<AgentToolSchema> {
        vec![]
    }

    #[test]
    fn verify_tool_schemas_include_web_tools() {
        use super::super::command_risk::RiskLevel;
        use super::super::tools::WorkspaceMode;
        let full = super::tool_schemas(RiskLevel::Standard, WorkspaceMode::Full);
        let names: Vec<&str> = full.iter().map(|s| s.name).collect();
        println!("Full mode tools: {names:?}");
        assert!(names.contains(&"web_search"), "web_search 应在 Full 模式可用");
        assert!(names.contains(&"web_fetch"), "web_fetch 应在 Full 模式可用");

        let chat = super::tool_schemas(RiskLevel::Standard, WorkspaceMode::ChatOnly);
        let chat_names: Vec<&str> = chat.iter().map(|s| s.name).collect();
        println!("ChatOnly mode tools: {chat_names:?}");
        assert!(chat_names.contains(&"web_search"), "web_search 应在 ChatOnly 模式可用");
        assert!(chat_names.contains(&"web_fetch"), "web_fetch 应在 ChatOnly 模式可用");
    }

    #[test]
    fn strip_think_tags_removes_think_block() {
        let input = "<think>用户在问文件列表</think>\n工作区只有一个文件 abc.txt";
        let result = super::strip_think_tags(input);
        assert!(!result.contains("<think>"));
        assert!(!result.contains("用户在问文件列表"));
        assert!(result.contains("工作区只有一个文件 abc.txt"));
    }

    #[test]
    fn strip_think_tags_preserves_text_without_think() {
        let input = "工作区只有一个文件 abc.txt";
        let result = super::strip_think_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn strip_think_tags_handles_inline_think() {
        let input = "<think>思考</think>实际回答";
        let result = super::strip_think_tags(input);
        assert_eq!(result, "实际回答");
    }

    #[test]
    fn parse_category_with_think_tags() {
        let input = "[category: 编译修复]\n修复了 lib.rs";
        let (cat, summary) = super::parse_category(input);
        assert_eq!(cat.as_deref(), Some("编译修复"));
        assert_eq!(summary, "修复了 lib.rs");
    }

#[test]
    fn mock_agent_returns_events_and_file_change() {
        let root =
            std::env::temp_dir().join(format!("sophoni-agent-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let expected = "# Sophoni\n\nMock task completed for: 更新 README\n";

        let result = run_mock_agent_task(root.clone(), "更新 README").unwrap();

        assert!(result.summary.contains("mock Agent"));
        assert!(result.events.iter().any(|event| event.kind == "tool"));
        assert_eq!(result.file_changes.len(), 1);
        let change = &result.file_changes[0];
        assert_eq!(change.path, "README.md");
        assert_eq!(change.kind, ChangeKind::Created);
        assert!(change.diff.contains("+# Sophoni"));
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            expected
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mock_agent_marks_existing_readme_as_modified() {
        let root =
            std::env::temp_dir().join(format!("sophoni-agent-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "old readme\n").unwrap();

        let result = run_mock_agent_task(root.clone(), "更新 README").unwrap();

        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.file_changes[0].path, "README.md");
        assert_eq!(result.file_changes[0].kind, ChangeKind::Modified);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn agent_loop_completes_read_then_write_then_summary() {
        let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "old\n").unwrap();

        let provider = FakeProvider::new(vec![
            ProviderResponse::ToolCalls(vec![fake_read_call("c1", "README.md")]),
            ProviderResponse::ToolCalls(vec![fake_write_call("c2", "README.md", "new\n")]),
            ProviderResponse::FinalAnswer("done".into()),
        ]);
        let tools = ToolDispatcher::new(root.clone());
        let sink = CollectingSink::new();
        let cancel = Arc::new(AtomicBool::new(false));

        let result = run_agent_task(
            Box::new(provider),
            &tools,
            &sink,
            &cancel,
            SystemPrompt("sys".into()),
            "update readme".into(),
            empty_schemas(),
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .unwrap();

        let emitted = sink.snapshot();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(emitted.iter().any(|e| e.kind == "summary"));
        assert_eq!(result.file_changes.len(), 1);
    }

    #[tokio::test]
    async fn final_answer_records_assistant_turn_for_continuation() {
        // 回归：FinalAnswer 分支必须把 assistant 回复 push 到 turns。
        // 否则续聊时历史缺失 assistant 回复，下一轮 user 会直接拼在上一轮 user 后面。
        let root = std::env::temp_dir().join(format!("sophoni-turn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let provider = FakeProvider::new(vec![ProviderResponse::FinalAnswer("done".into())]);
        let tools = ToolDispatcher::new(root.clone());
        let sink = CollectingSink::new();
        let cancel = Arc::new(AtomicBool::new(false));

        let result = run_agent_task(
            Box::new(provider),
            &tools,
            &sink,
            &cancel,
            SystemPrompt("sys".into()),
            "你好".into(),
            empty_schemas(),
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        // turns 应为 [User("你好"), Assistant("done")]，assistant 回复不能丢
        assert_eq!(result.turns.len(), 2);
        assert!(matches!(&result.turns[0], ConversationTurn::User { content } if content == "你好"));
        assert!(matches!(&result.turns[1], ConversationTurn::Assistant { content: Some(c), tool_calls } if c == "done" && tool_calls.is_empty()));
    }

    #[tokio::test]
    async fn continuation_does_not_concat_two_user_turns() {
        // 回归：模拟续聊 —— 传入 history_turns（含上一轮的 user + assistant 回复），
        // 再追加新一轮 user。验证 turns 结构为 user→assistant→user，不会出现连续两个 user。
        let root = std::env::temp_dir().join(format!("sophoni-cont-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let history = vec![
            ConversationTurn::User { content: "第一个问题".into() },
            ConversationTurn::Assistant { content: Some("第一个回答".into()), tool_calls: vec![] },
        ];
        let provider = FakeProvider::new(vec![ProviderResponse::FinalAnswer("第二个回答".into())]);
        let tools = ToolDispatcher::new(root.clone());
        let sink = CollectingSink::new();
        let cancel = Arc::new(AtomicBool::new(false));

        let result = run_agent_task(
            Box::new(provider),
            &tools,
            &sink,
            &cancel,
            SystemPrompt("sys".into()),
            "第二个问题".into(),
            empty_schemas(),
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            history,
        )
        .await
        .unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        // 期望：[User("第一个问题"), Assistant("第一个回答"), User("第二个问题"), Assistant("第二个回答")]
        assert_eq!(result.turns.len(), 4);
        assert!(matches!(&result.turns[0], ConversationTurn::User { content } if content == "第一个问题"));
        assert!(matches!(&result.turns[1], ConversationTurn::Assistant { content: Some(c), .. } if c == "第一个回答"));
        assert!(matches!(&result.turns[2], ConversationTurn::User { content } if content == "第二个问题"));
        assert!(matches!(&result.turns[3], ConversationTurn::Assistant { content: Some(c), .. } if c == "第二个回答"));
    }

    #[tokio::test]
    async fn agent_loop_stops_on_max_rounds() {
        let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f.txt"), "x\n").unwrap();

        let provider = FakeProvider::always(ProviderResponse::ToolCalls(vec![fake_read_call(
            "c", "f.txt",
        )]));
        let tools = ToolDispatcher::new(root.clone());
        let sink = CollectingSink::new();
        let cancel = Arc::new(AtomicBool::new(false));

        let _result = run_agent_task(
            Box::new(provider),
            &tools,
            &sink,
            &cancel,
            SystemPrompt("s".into()),
            "t".into(),
            empty_schemas(),
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .unwrap();

        let emitted = sink.snapshot();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(emitted
            .iter()
            .any(|e| e.kind == "error" && e.body.contains("最大轮次")));
    }

    #[tokio::test]
    async fn agent_loop_stops_on_cancel() {
        let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f.txt"), "x\n").unwrap();

        let provider = FakeProvider::always(ProviderResponse::ToolCalls(vec![fake_read_call(
            "c", "f.txt",
        )]));
        let tools = ToolDispatcher::new(root.clone());
        let sink = CollectingSink::new();
        let cancel = Arc::new(AtomicBool::new(false));
        cancel.store(true, Ordering::Relaxed);

        let _result = run_agent_task(
            Box::new(provider),
            &tools,
            &sink,
            &cancel,
            SystemPrompt("s".into()),
            "t".into(),
            empty_schemas(),
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .unwrap();

        let emitted = sink.snapshot();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(emitted
            .iter()
            .any(|e| e.kind == "error" && e.body.contains("取消")));
    }

    #[tokio::test]
    async fn agent_loop_stops_on_provider_error() {
        let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let provider = FakeProvider::always_error("boom");
        let tools = ToolDispatcher::new(root.clone());
        let sink = CollectingSink::new();
        let cancel = Arc::new(AtomicBool::new(false));

        let _result = run_agent_task(
            Box::new(provider),
            &tools,
            &sink,
            &cancel,
            SystemPrompt("s".into()),
            "t".into(),
            empty_schemas(),
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .unwrap();

        let emitted = sink.snapshot();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(emitted
            .iter()
            .any(|e| e.kind == "error" && e.body.contains("Provider")));
    }

    // ── run_command 端到端（真实 Provider，需联网 + API Key）──
    // 用 `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored run_command_live`
    // 运行。默认 ignore，避免污染常规测试与 CI。

    #[tokio::test]
    #[ignore]
    async fn run_command_live_invokes_tool_against_real_provider() {
        use super::super::domain::{AgentConfig, AgentEvent, SystemPrompt};
        use super::super::provider::OpenAICompatibleProvider;
        use super::super::tools::ToolDispatcher;
        use super::{run_agent_task, EventSink};

        let (config, provider_name) = AgentConfig::load().expect("AgentConfig 未配置");
        eprintln!("使用真实 Provider: {provider_name} / {}", config.model);

        let root = std::env::temp_dir().join(format!("sophoni-live-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "# live\n").unwrap();
        let _ = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output();

        let provider: Box<dyn super::super::provider::AgentProvider> =
            Box::new(OpenAICompatibleProvider::new(config));
        let tools = ToolDispatcher::new(root.clone());
        let cancel = std::sync::atomic::AtomicBool::new(false);

        struct Collector(std::sync::Mutex<Vec<AgentEvent>>);
        impl EventSink for Collector {
            fn emit(&self, event: &AgentEvent) {
                self.0.lock().unwrap().push(event.clone());
            }
        }
        let sink = Collector(std::sync::Mutex::new(Vec::new()));

        let task = "在工作区跑一次 git status，把命令输出原样告诉我。".to_string();
        let result = run_agent_task(
            provider,
            &tools,
            &sink,
            &cancel,
            SystemPrompt(String::new()),
            task,
            vec![],
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .expect("run_agent_task 出错");

        eprintln!("Agent summary: {}", result.summary);

        let events = sink.0.lock().unwrap();
        eprintln!("收到 {} 个事件", events.len());
        for ev in events.iter() {
            eprintln!("  [{}] {}", ev.kind, ev.title);
        }

        let invoked_run_command = events
            .iter()
            .any(|e| e.kind == "tool_call" && e.title.starts_with("run_command:"));
        let has_non_error_result = events
            .iter()
            .any(|e| e.kind == "tool_result" && !e.body.starts_with("失败"));

        let _ = std::fs::remove_dir_all(&root);

        assert!(invoked_run_command, "Agent 没有调用 run_command 工具");
        assert!(
            has_non_error_result,
            "run_command 没有产生成功结果（可能命令被拒或执行失败）"
        );
    }

    // ── 场景 2：失败自纠闭环（真实 Provider）──
    // 用 `cargo test -- --ignored run_command_self_heal` 运行。

    #[tokio::test]
    #[ignore]
    async fn run_command_self_heal_fixes_compile_error_against_real_provider() {
        use super::super::domain::{AgentConfig, AgentEvent, SystemPrompt};
        use super::super::provider::OpenAICompatibleProvider;
        use super::super::tools::ToolDispatcher;
        use super::{run_agent_task, EventSink};

        let (config, provider_name) = AgentConfig::load().expect("AgentConfig 未配置");
        eprintln!("使用真实 Provider: {provider_name} / {}", config.model);

        let root = std::env::temp_dir().join(format!("sophoni-heal-{}", uuid::Uuid::new_v4()));
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            &root.join("Cargo.toml"),
            "[package]\nname = \"heal_demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"src/lib.rs\"\n",
        )
        .unwrap();
        // 缺少分号 → cargo check 必然报 "expected `;`"。
        std::fs::write(
            src_dir.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    let x = a\n    let y = b\n    x + y\n}\n",
        )
        .unwrap();

        let provider: Box<dyn super::super::provider::AgentProvider> =
            Box::new(OpenAICompatibleProvider::new(config));
        let tools = ToolDispatcher::new(root.clone());
        let cancel = std::sync::atomic::AtomicBool::new(false);

        struct Collector(std::sync::Mutex<Vec<AgentEvent>>);
        impl EventSink for Collector {
            fn emit(&self, event: &AgentEvent) {
                self.0.lock().unwrap().push(event.clone());
            }
        }
        let sink = Collector(std::sync::Mutex::new(Vec::new()));

        let task = "这个 Rust 工程的 src/lib.rs 有编译错误。请用 cargo check 验证，\
                    根据报错修正代码，再跑一次 cargo check 确认修复成功。"
            .to_string();
        let result = run_agent_task(
            provider,
            &tools,
            &sink,
            &cancel,
            SystemPrompt(String::new()),
            task,
            vec![],
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .expect("run_agent_task 出错");
        eprintln!("Agent summary: {}", result.summary);

        let events = sink.0.lock().unwrap();
        eprintln!("收到 {} 个事件", events.len());

        let mut check_calls = 0usize;
        let mut failed_checks = 0usize;
        let mut successful_checks = 0usize;
        let mut edit_calls = 0usize;
        let mut saw_edit = false;

        for ev in events.iter() {
            eprintln!("  [{}] {}", ev.kind, ev.title);
            match ev.kind.as_str() {
                "tool_call" => {
                    if ev.title.starts_with("run_command:") && ev.title.contains("cargo check") {
                        check_calls += 1;
                    } else if ev.title.starts_with("edit_file:") {
                        edit_calls += 1;
                        saw_edit = true;
                    }
                }
                "tool_result" => {
                    let is_check_result = ev.body.contains("cargo check")
                        || ev.body.contains("error[")
                        || ev.body.contains("error:")
                        || ev.body.starts_with("exit code:");
                    if !is_check_result {
                        continue;
                    }
                    if ev.body.starts_with("失败:") {
                        failed_checks += 1;
                    } else if ev.body.contains("exit code: 0") {
                        successful_checks += 1;
                    }
                }
                _ => {}
            }
        }

        let succeeded_after_edit = successful_checks >= 1
            && saw_edit
            && successful_checks + failed_checks >= 2;
        eprintln!(
            "诊断: cargo check 调用 {check_calls} 次（成功 {successful_checks}，失败 {failed_checks}），edit_file {edit_calls} 次"
        );

        let _ = std::fs::remove_dir_all(&root);

        assert!(
            check_calls >= 1,
            "Agent 没有用 run_command 跑 cargo check"
        );
        assert!(edit_calls >= 1, "Agent 没有调用 edit_file 修复代码");
        assert!(
            successful_checks >= 1,
            "cargo check 从未成功，Agent 没能完成修复（可能是模型能力不足）"
        );
        assert!(
            succeeded_after_edit,
            "未形成'check失败→edit→check成功'闭环：check总次数 {}，成功 {}，edit {}",
            successful_checks + failed_checks,
            successful_checks,
            edit_calls
        );
    }

    // ── Unrestricted 等级 live 测试：真实 LLM 尝试用 rm 删文件 ──
    // 验证动态 prompt 生效：Unrestricted 模式下模型知道可以用 run_command 执行 rm，
    // 而不是说"我没有删除工具"。用 `cargo test -- --ignored run_command_unrestricted_rm` 运行。

    #[tokio::test]
    #[ignore]
    async fn run_command_unrestricted_rm_uses_command_not_refuse() {
        use super::super::command_risk::RiskLevel;
        use super::super::domain::{AgentConfig, AgentEvent, SystemPrompt};
        use super::super::provider::OpenAICompatibleProvider;
        use super::super::tools::ToolDispatcher;
        use super::{run_agent_task, EventSink};

        let (config, provider_name) = AgentConfig::load().expect("AgentConfig 未配置");
        eprintln!("使用真实 Provider: {provider_name} / {}", config.model);

        let root = std::env::temp_dir().join(format!("sophoni-rm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("junk.txt"), "delete me\n").unwrap();

        let provider: Box<dyn super::super::provider::AgentProvider> =
            Box::new(OpenAICompatibleProvider::new(config));
        let tools = ToolDispatcher::new(root.clone())
            .with_risk_level(RiskLevel::Unrestricted);
        let cancel = std::sync::atomic::AtomicBool::new(false);

        struct Collector(std::sync::Mutex<Vec<AgentEvent>>);
        impl EventSink for Collector {
            fn emit(&self, event: &AgentEvent) {
                self.0.lock().unwrap().push(event.clone());
            }
        }
        let sink = Collector(std::sync::Mutex::new(Vec::new()));

        let task = "删除工作区里的 junk.txt 文件。".to_string();
        let result = run_agent_task(
            provider,
            &tools,
            &sink,
            &cancel,
            SystemPrompt(String::new()),
            task,
            vec![],
            uuid::Uuid::new_v4(),
            String::new(),
            vec![],
            vec![],
        )
        .await
        .expect("run_agent_task 出错");
        eprintln!("Agent summary: {}", result.summary);

        let events = sink.0.lock().unwrap();
        eprintln!("收到 {} 个事件", events.len());
        for ev in events.iter() {
            eprintln!("  [{}] {}", ev.kind, ev.title);
        }

        // 核心断言：Agent 用 run_command 调了 rm（而非说"我没有删除工具"）
        let invoked_rm = events
            .iter()
            .any(|e| e.kind == "tool_call" && e.title.starts_with("run_command: rm"));
        assert!(
            invoked_rm,
            "Agent 没有通过 run_command 执行 rm 删除文件。summary: {}",
            result.summary
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
