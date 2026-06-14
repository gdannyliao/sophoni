use std::path::PathBuf;

use chrono::Utc;
use uuid::Uuid;

use super::domain::{AgentEvent, ChangeKind, FileChange};
use super::errors::AppResult;
use super::workspace::WorkspaceFs;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskResult {
    pub summary: String,
    pub events: Vec<AgentEvent>,
    pub file_changes: Vec<FileChange>,
}

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
                kind: "thought".to_string(),
                title: "理解任务".to_string(),
                body: prompt.to_string(),
            },
            AgentEvent {
                kind: "tool".to_string(),
                title: "写入 README.md".to_string(),
                body: "已写入 README.md 并生成 diff。".to_string(),
            },
            AgentEvent {
                kind: "summary".to_string(),
                title: "任务完成".to_string(),
                body: "mock Agent 已生成可展示的文件变更。".to_string(),
            },
        ],
        file_changes: vec![change],
    })
}
