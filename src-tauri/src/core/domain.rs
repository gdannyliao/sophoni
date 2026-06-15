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
    pub created_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub kind: String,
    pub title: String,
    pub body: String,
}

// ── Agent runtime types (model-agnostic) ──
// Prefixed with `Agent` to distinguish from persistence types above.

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
pub struct AgentToolCall {
    pub id: String,
    pub name: AgentToolName,
    pub arguments: AgentToolArgs,
}

#[derive(Debug, Clone)]
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
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub configured: bool,
    pub provider: String,
    pub model: String,
}
