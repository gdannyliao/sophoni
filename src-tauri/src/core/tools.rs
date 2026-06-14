use std::path::PathBuf;

use chrono::Utc;
use uuid::Uuid;

use super::domain::{
    AgentToolArgs, AgentToolCall, AgentToolName, AgentToolResult, ChangeKind, FileChange,
};
use super::errors::{AppError, AppResult};
use super::workspace::WorkspaceFs;

pub struct ToolDispatcher {
    fs: WorkspaceFs,
}

impl ToolDispatcher {
    pub fn new(root: PathBuf) -> Self {
        Self { fs: WorkspaceFs::new(root) }
    }

    pub async fn dispatch(&self, call: &AgentToolCall) -> AppResult<AgentToolResult> {
        match (&call.name, &call.arguments) {
            (AgentToolName::ReadFile, AgentToolArgs::Read { path }) => {
                self.read_file(&call.id, path).await
            }
            (AgentToolName::WriteFile, AgentToolArgs::Write { path, content }) => {
                self.write_file(&call.id, path, content).await
            }
            _ => Err(AppError::Tool("tool name and arguments do not match".into())),
        }
    }

    async fn read_file(&self, call_id: &str, path: &str) -> AppResult<AgentToolResult> {
        let full = self.fs.root().join(path);
        match self.fs.read_text(&full) {
            Ok(content) => Ok(AgentToolResult {
                tool_call_id: call_id.to_string(),
                content,
                is_error: false,
                file_change: None,
            }),
            Err(e) => Ok(tool_error(call_id, &format!("读取失败: {e}"))),
        }
    }

    async fn write_file(&self, call_id: &str, path: &str, content: &str) -> AppResult<AgentToolResult> {
        let full = self.fs.root().join(path);
        let write = match self.fs.write_text_with_snapshot(&full, content) {
            Ok(w) => w,
            Err(e) => return Ok(tool_error(call_id, &format!("写入失败: {e}"))),
        };

        let existed = !write.previous_text.is_empty();
        let change = FileChange {
            id: Uuid::new_v4(),
            task_run_id: Uuid::new_v4(),
            path: path.to_string(),
            kind: if existed { ChangeKind::Modified } else { ChangeKind::Created },
            diff: write.diff,
            created_at: Utc::now(),
        };

        Ok(AgentToolResult {
            tool_call_id: call_id.to_string(),
            content: format!("已写入 {} ({} 行)", path, content.lines().count().max(1)),
            is_error: false,
            file_change: Some(change),
        })
    }
}

fn tool_error(call_id: &str, message: &str) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: message.to_string(),
        is_error: true,
        file_change: None,
    }
}
