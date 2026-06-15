use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;
use walkdir::WalkDir;

use super::domain::{
    AgentToolArgs, AgentToolCall, AgentToolName, AgentToolResult, ChangeKind, FileChange,
};
use super::errors::{AppError, AppResult};
use super::workspace::{lexical_normalize, WorkspaceFs};

const LIST_FILES_MAX: usize = 200;
const GREP_MAX: usize = 100;
const MAX_FILE_BYTES: u64 = 1_000_000;
const MAX_DEPTH: usize = 10;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".svelte-kit",
    "__pycache__",
];

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
            (AgentToolName::ListFiles, AgentToolArgs::ListFiles { path, recursive }) => {
                self.list_files(&call.id, path.as_deref(), *recursive).await
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

    async fn list_files(
        &self,
        call_id: &str,
        path: Option<&str>,
        recursive: bool,
    ) -> AppResult<AgentToolResult> {
        let root = self.fs.root().to_path_buf();
        let target = match path {
            Some(p) => match resolve_within_root(&root, p) {
                Ok(t) => t,
                Err(e) => return Ok(tool_error(call_id, &e)),
            },
            None => root.clone(),
        };

        let mut entries = Vec::new();
        let walker = WalkDir::new(&target)
            .follow_links(false)
            .max_depth(if recursive { MAX_DEPTH } else { 1 })
            .into_iter()
            .filter_entry(|e| !is_ignored_dir(e));

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.path() == target {
                continue;
            }
            let kind = if entry.file_type().is_dir() { "dir" } else { "file" };
            let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            entries.push(format!("{kind}  {}", rel.display()));
            if entries.len() >= LIST_FILES_MAX {
                break;
            }
        }

        let truncated = entries.len() >= LIST_FILES_MAX;
        let mut content = entries.join("\n");
        if content.is_empty() {
            content = "（空目录）".to_string();
        }
        if truncated {
            content.push_str(&format!("\n（结果已截断，只显示前 {LIST_FILES_MAX} 项）"));
        }

        Ok(AgentToolResult {
            tool_call_id: call_id.to_string(),
            content,
            is_error: false,
            file_change: None,
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

fn resolve_within_root(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let joined = root.join(relative);
    let normalized = lexical_normalize(&joined);
    if normalized.starts_with(root) {
        Ok(normalized)
    } else {
        Err(format!("路径越界: {relative}"))
    }
}

fn is_ignored_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .map(|n| IGNORED_DIRS.contains(&n))
            .unwrap_or(false)
}
