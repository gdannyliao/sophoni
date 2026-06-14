use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use uuid::Uuid;

use super::domain::{
    AgentEvent, AgentToolArgs, AgentToolCall, AgentToolResult, AgentToolSchema,
    ChangeKind, ConversationTurn, FileChange, ProviderResponse, SystemPrompt,
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

const SYSTEM_PROMPT: &str = "你是桌面工作区 Agent。只能操作工作区内文件。必须通过提供的工具(read_file/write_file)操作文件,不要在回复里直接给文件内容。完成任务后给出简短总结。";

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
                push(&mut events, sink, error_event(&format!("Provider 错误: {e}")));
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
    if !events.iter().any(|e| e.kind == "summary")
        && !events.iter().any(|e| e.kind == "error")
    {
        push(&mut events, sink, error_event("达到最大轮次(12),已停止"));
    }

    let summary = events
        .iter()
        .rev()
        .find(|e| e.kind == "summary")
        .map(|e| e.body.clone())
        .unwrap_or_else(|| "任务未正常完成,以上是已执行的步骤。".into());

    Ok(AgentTaskResult { summary, events, file_changes })
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
    ]
}

fn error_event(body: &str) -> AgentEvent {
    AgentEvent { kind: "error".into(), title: "错误".into(), body: body.into() }
}

fn summary_event(body: &str) -> AgentEvent {
    AgentEvent { kind: "summary".into(), title: "任务完成".into(), body: body.into() }
}

fn tool_call_event(call: &AgentToolCall) -> AgentEvent {
    let (label, detail) = match &call.arguments {
        AgentToolArgs::Read { path } => ("read_file", path.clone()),
        AgentToolArgs::Write { path, .. } => ("write_file", path.clone()),
    };
    AgentEvent {
        kind: "tool_call".into(),
        title: format!("{label}: {detail}"),
        body: format!("调用工具 {label}"),
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
            AgentEvent { kind: "thought".into(), title: "理解任务".into(), body: prompt.to_string() },
            AgentEvent { kind: "tool".into(), title: "写入 README.md".into(), body: "已写入 README.md 并生成 diff。".into() },
            AgentEvent { kind: "summary".into(), title: "任务完成".into(), body: "mock Agent 已生成可展示的文件变更。".into() },
        ],
        file_changes: vec![change],
    })
}
