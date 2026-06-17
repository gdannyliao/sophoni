use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    WaitingForRiskDecision,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    FileSearch,
    FileRead,
    FileWrite,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub last_opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub events_json: String,
    pub turns_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: String,
    pub updated_at: DateTime<Utc>,
}


#[derive(Debug, Clone)]
pub struct ConversationMemory {
    pub category: Option<String>,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRun {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub status: TaskStatus,
    pub summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: Uuid,
    pub task_run_id: Uuid,
    pub kind: ToolKind,
    pub name: String,
    pub input_json: String,
    pub output_summary: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub id: Uuid,
    pub task_run_id: Uuid,
    pub path: String,
    pub kind: ChangeKind,
    pub diff: String,
    pub created_at: DateTime<Utc>,
}

/// 发给前端的事件单元。通过 Tauri 事件 `agent-event` 推送。
///
/// `kind` 取值：
/// - `tool_call`：模型发起了工具调用（`title` 形如 `read_file: README.md`，`body` 为参数摘要）
/// - `tool_result`：工具执行结果（`tool_call_id` 关联到对应 `tool_call`）
/// - `summary`：最终摘要（任务结束时整条发出）
/// - `thought`：思考提示
/// - `error`：错误
/// - `token`：**流式增量文本**。`title` 标识流段（`"summary"` = 最终摘要流；
///   `"reasoning"` = 工具调用前的推理文本流），`body` 为本次增量片段（非完整文本）。
///   前端需按 `title` 累积拼接后渲染。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

// ── Agent runtime types (model-agnostic) ──
// Prefixed with `Agent` to distinguish from persistence types above.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ConversationTurn {
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<AgentToolCall>,
    },
    Tool {
        tool_call_id: String,
        result: AgentToolResult,
    },
}

#[derive(Debug, Clone)]
pub struct SystemPrompt(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolName {
    ReadFile,
    WriteFile,
    ListFiles,
    Grep,
    EditFile,
    ReadAcceptanceReport,
    ReadRuntimeLog,
    ListAcceptanceRuns,
    RunCommand,
    WebSearch,
    WebFetch,
    MultiEditFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentToolArgs {
    Read {
        path: String,
    },
    Write {
        path: String,
        content: String,
    },
    ListFiles {
        path: Option<String>,
        recursive: bool,
    },
    Grep {
        pattern: String,
        path: Option<String>,
        include: Option<String>,
    },
    EditFile {
        path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    },
    ReadAcceptanceReport {
        run_id: Option<String>,
    },
    ReadRuntimeLog {
        run_id: Option<String>,
        file_name: String,
        max_lines: usize,
    },
    ListAcceptanceRuns {
        limit: usize,
    },
    RunCommand { command: String },
    WebSearch { query: String, max_results: usize },
    WebFetch { url: String, max_chars: usize },
    MultiEditFile { path: String, edits: Vec<MultiEdit> },
}

/// multi_edit_file 的单处替换项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiEdit {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub id: String,
    pub name: AgentToolName,
    pub arguments: AgentToolArgs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
    pub file_change: Option<FileChange>,
}

#[derive(Debug, Clone)]
pub enum ProviderResponse {
    ToolCalls(Vec<AgentToolCall>),
    FinalAnswer(String),
}

#[derive(Debug, Clone)]
pub struct AgentToolSchema {
    pub name: &'static str,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub risk_level: super::command_risk::RiskLevel,
    pub workspace_path: Option<String>,
    pub search_config: Option<super::web::SearchConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub configured: bool,
    pub provider: String,
    pub model: String,
}

#[cfg(test)]
mod tests {
    use super::{TaskStatus, ToolKind};

    #[test]
    fn task_status_is_serialized_as_snake_case() {
        let value = serde_json::to_value(TaskStatus::WaitingForRiskDecision).unwrap();
        assert_eq!(value, "waiting_for_risk_decision");
    }

    #[test]
    fn tool_kind_is_serialized_as_snake_case() {
        let value = serde_json::to_value(ToolKind::FileWrite).unwrap();
        assert_eq!(value, "file_write");
    }
}
