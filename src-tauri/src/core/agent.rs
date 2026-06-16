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

fn system_prompt(level: super::command_risk::RiskLevel) -> String {
    use super::command_risk::RiskLevel;
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
) -> AppResult<AgentTaskResult> {
    let system = SystemPrompt(system_prompt(tools.risk_level()));
    let mut turns: Vec<ConversationTurn> = vec![ConversationTurn::User { content: user_task }];
    let mut events: Vec<AgentEvent> = vec![];
    let mut file_changes: Vec<FileChange> = vec![];
    let schemas = tool_schemas(tools.risk_level());
    let deadline = Instant::now() + OVERALL_TIMEOUT;

    // emit conversation_created 让前端立即更新 Sidebar
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

fn tool_schemas(level: super::command_risk::RiskLevel) -> Vec<AgentToolSchema> {
    vec![
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
    ]
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
    })
}

#[cfg(test)]
mod tests {
    use super::super::domain::{AgentToolSchema, ChangeKind, ProviderResponse, SystemPrompt};
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
        )
        .await
        .unwrap();

        let emitted = sink.snapshot();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(emitted.iter().any(|e| e.kind == "summary"));
        assert_eq!(result.file_changes.len(), 1);
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
