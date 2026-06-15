use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
}

const SYSTEM_PROMPT: &str = "你是桌面工作区 Agent。只能操作工作区内文件。

可用工具：
- list_files：列出目录内容，了解工作区结构。不确定文件在哪时，先用它探索。
- grep：按正则搜索文件内容。找某个函数/变量/字符串用在哪时用它。
- read_file：读取指定文件内容。
- write_file：写入整个文件（新建或大改时用）。
- edit_file：精确替换文件中的一段文本（小改时用，比 write_file 省 token）。
- list_acceptance_runs：列出最近验收运行 ID。
- read_acceptance_report：读取验收报告 report.json，可不传 run_id 读取最新一次。
- read_runtime_log：读取验收运行日志的尾部内容，可不传 run_id 读取最新一次。

工作方式：
1. 不确定路径时，先 list_files 或 grep 探索。
2. 改文件前，先用 read_file 看当前内容。
3. 小改动优先用 edit_file（给出要替换的原文和新文本），大改动或新建文件用 write_file。
4. edit_file 的 old_string 必须与文件内容精确匹配（含缩进和空格）。
5. 当用户要求替换「所有」或「全部」时，用 edit_file 的 replace_all=true，一次替换所有匹配，不要分多次单独替换。
6. 验收时优先用 read_acceptance_report 看 report.json，重点检查 ok 和 failureSummary；失败或信息不足时再用 read_runtime_log 查看相关日志。
7. 不要在回复里直接给文件内容，通过工具操作。
8. 完成任务后给出简短总结。";

const MAX_ROUNDS: usize = 12;
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
) -> AppResult<AgentTaskResult> {
    let system = SystemPrompt(SYSTEM_PROMPT.to_string());
    let mut turns: Vec<ConversationTurn> = vec![ConversationTurn::User { content: user_task }];
    let mut events: Vec<AgentEvent> = vec![];
    let mut file_changes: Vec<FileChange> = vec![];
    let schemas = tool_schemas();
    let deadline = Instant::now() + OVERALL_TIMEOUT;

    for _round in 0..MAX_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            push(&mut events, sink, error_event("用户取消了任务"));
            break;
        }
        if Instant::now() >= deadline {
            push(&mut events, sink, error_event("达到整体超时(120s)"));
            break;
        }

        let response = tokio::time::timeout(
            PER_ROUND_TIMEOUT,
            provider.complete(&system, &turns, &schemas),
        )
        .await;

        let calls = match response {
            Ok(Ok(ProviderResponse::FinalAnswer(text))) => {
                push(&mut events, sink, summary_event(&text));
                break;
            }
            Ok(Ok(ProviderResponse::ToolCalls(calls))) => calls,
            Ok(Err(e)) => {
                push(
                    &mut events,
                    sink,
                    error_event(&format!("Provider 错误: {e}")),
                );
                break;
            }
            Err(_elapsed) => {
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

    Ok(AgentTaskResult {
        summary,
        events,
        file_changes,
    })
}

fn push(events: &mut Vec<AgentEvent>, sink: &dyn EventSink, event: AgentEvent) {
    sink.emit(&event);
    events.push(event);
}

fn tool_schemas() -> Vec<AgentToolSchema> {
    vec![
        AgentToolSchema {
            name: "read_file",
            description: "读取工作区内指定文件的文本内容。路径相对于工作区根目录。",
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
            description: "向工作区内指定文件写入文本内容(覆盖)。路径相对于工作区根目录。",
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
            description: "列出工作区内指定目录的文件和子目录。默认只列直接子项（不递归）。",
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
            description: "在工作区内搜索匹配正则表达式的文件内容。返回 path:line:content 格式的结果。",
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
            description: "对已有文件做精确文本替换(search-replace)。先 read_file 看准内容,再给出 old_string(必须与文件内容精确匹配,含缩进)和 new_string。old_string 必须在文件中唯一,除非 replace_all=true。",
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
            name: "read_acceptance_report",
            description: "读取验收运行的 report.json。默认读取最新一次验收运行；用于判断 ok 和 failureSummary。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "验收运行 ID；省略时读取最新一次" }
                }
            }),
        },
        AgentToolSchema {
            name: "read_runtime_log",
            description: "读取验收运行日志尾部内容。默认读取最新一次验收运行；max_lines 默认由模型调用方可省略，建议先读少量行。",
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
            description: "列出最近验收运行 ID。limit 会限制到 1 到 20。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 20, "description": "返回条数，默认 5" }
                }
            }),
        },
    ]
}

fn error_event(body: &str) -> AgentEvent {
    AgentEvent {
        kind: "error".into(),
        title: "错误".into(),
        body: body.into(),
    }
}

fn summary_event(body: &str) -> AgentEvent {
    AgentEvent {
        kind: "summary".into(),
        title: "任务完成".into(),
        body: body.into(),
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
        AgentToolArgs::RunCommand { .. } => ("run_command", "(待实现)".to_string(), String::new()),
    };
    AgentEvent {
        kind: "tool_call".into(),
        title: format!("{label}: {detail}"),
        body,
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
            },
            AgentEvent {
                kind: "tool".into(),
                title: "写入 README.md".into(),
                body: "已写入 README.md 并生成 diff。".into(),
            },
            AgentEvent {
                kind: "summary".into(),
                title: "任务完成".into(),
                body: "mock Agent 已生成可展示的文件变更。".into(),
            },
        ],
        file_changes: vec![change],
    })
}
