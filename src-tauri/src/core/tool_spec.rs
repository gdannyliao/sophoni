//! 工具定义的单一事实源。每个工具一个 `impl ToolSpec`，收拢此前散落在
//! domain/provider/tools/agent 四文件的 6 处 match。
//!
//! 加新工具 = 加一个 struct + impl ToolSpec + 在 build_tool_registry 注册。
//! 不再需要同步改 6 处大 match。

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;
use walkdir::WalkDir;

use super::acceptance::{list_acceptance_runs, read_acceptance_report, read_runtime_log};
use super::command_risk::{classify_command_with_level, shell_words, CommandAction, RiskLevel};
use super::domain::{
    AgentToolArgs, AgentToolCall, AgentToolName, AgentToolResult, AgentToolSchema, ChangeKind,
    FileChange, MultiEdit,
};
use super::errors::{AppError, AppResult};
use super::tools::{ConfirmHandler, WorkspaceMode};
use super::web::{self, SearchConfig};
use super::workspace::{lexical_normalize, WorkspaceFs};

/// dispatch 的返回类型（手动 Box::pin，因为 trait object 不能用 async-trait）。
pub type ToolFuture = Pin<Box<dyn std::future::Future<Output = AppResult<AgentToolResult>> + Send>>;

/// 一个工具的全部行为事实源。
pub trait ToolSpec: Send + Sync {
    /// 工具名（wire 格式字符串，如 "read_file"）
    fn name(&self) -> &'static str;
    /// 给 LLM 的 schema 描述
    fn schema(&self) -> AgentToolSchema;
    /// 参数 → wire JSON（序列化，发往 LLM 的 tool_call.arguments）
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value;
    /// wire JSON → AgentToolCall（解析，从 LLM 响应构造）
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall>;
    /// 参数 → (title, body) 用于 tool_call_event 渲染
    fn describe(&self, args: &AgentToolArgs) -> (String, String);
    /// 执行工具
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture;
    /// 是否在 ChatOnly 模式下可用（默认 false，网络工具 override 为 true）
    fn available_in_chat_only(&self) -> bool {
        false
    }
}

// ── 辅助函数（从 provider.rs 搬来，供各工具 parse 复用）──

fn req_str(args: &serde_json::Value, key: &str, tool: &str) -> AppResult<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| AppError::Provider(format!("{tool} missing {key}")))
}

fn opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn opt_bool(args: &serde_json::Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn opt_usize(args: &serde_json::Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(default)
}

// ── 通用辅助（从 tools.rs 搬来）──

pub(crate) fn tool_error(call_id: &str, message: &str) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: message.to_string(),
        is_error: true,
        file_change: None,
    }
}

pub(crate) fn resolve_within_root(root: &std::path::Path, relative: &str) -> Result<PathBuf, String> {
    let joined = root.join(relative);
    let normalized = lexical_normalize(&joined);
    if normalized.starts_with(root) {
        Ok(normalized)
    } else {
        Err(format!("路径越界: {relative}"))
    }
}

const LIST_FILES_MAX: usize = 200;
const GREP_MAX: usize = 100;
const ACCEPTANCE_RUNS_LIMIT_MIN: usize = 1;
const ACCEPTANCE_RUNS_LIMIT_MAX: usize = 20;
const ACCEPTANCE_REPORT_MAX_BYTES: usize = 64 * 1024;
const ACCEPTANCE_LOG_MAX_BYTES: usize = 32 * 1024;
const MAX_FILE_BYTES: u64 = 1_000_000;
const MAX_DEPTH: usize = 10;
const READ_RUNTIME_LOG_DEFAULT_MAX_LINES: usize = 80;
const READ_RUNTIME_LOG_MAX_LINES: u64 = 200;
const LIST_ACCEPTANCE_RUNS_DEFAULT_LIMIT: usize = 5;
const LIST_ACCEPTANCE_RUNS_MAX_LIMIT: u64 = 20;

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

fn is_ignored_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .map(|n| IGNORED_DIRS.contains(&n))
            .unwrap_or(false)
}

fn find_actual_string(content: &str, old_string: &str) -> Option<String> {
    if content.contains(old_string) {
        return Some(old_string.to_string());
    }
    let content_chars: Vec<char> = content.chars().collect();
    let old_chars: Vec<char> = old_string.chars().collect();
    let norm_content: String = content_chars.iter().map(|c| normalize_one_quote(*c)).collect();
    let norm_old: String = old_chars.iter().map(|c| normalize_one_quote(*c)).collect();
    let idx = norm_content.find(&norm_old)?;
    let char_start = norm_content[..idx].chars().count();
    let char_len = old_chars.len();
    let actual: String = content_chars[char_start..char_start + char_len]
        .iter()
        .collect();
    Some(actual)
}

fn normalize_one_quote(c: char) -> char {
    match c {
        '\u{201C}' | '\u{201D}' => '"',
        '\u{2018}' | '\u{2019}' => '\'',
        other => other,
    }
}

pub(crate) fn truncate_output(s: &str, max_lines: usize, max_chars: usize) -> String {
    let truncated: String = s.chars().take(max_chars).collect();
    let lines: Vec<&str> = truncated.lines().take(max_lines).collect();
    let result = lines.join("\n");
    let total_lines = s.lines().count();
    let total_chars = s.chars().count();
    if total_lines > max_lines || total_chars > max_chars {
        format!(
            "{result}\n（输出已截断，显示前 {}/{} 行。如需完整输出，请在终端手动运行。）",
            lines.len(),
            total_lines
        )
    } else {
        result
    }
}

fn cap_text_bytes(content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }
    let mut end = max_bytes;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    let mut capped = content[..end].to_string();
    capped.push_str(&format!("\n（内容已截断，只显示前 {max_bytes} 字节）"));
    capped
}

// ════════════════════════════════════════════════════════════════
// 文件类工具（7 个）：read / write / list / grep / edit / multi_edit / delete
// ════════════════════════════════════════════════════════════════

// ── read_file ──

pub struct ReadFileTool {
    fs: WorkspaceFs,
}

impl ToolSpec for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::Read { path } => serde_json::json!({ "path": path }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let path = req_str(args, "path", "read_file")?;
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::ReadFile,
            arguments: AgentToolArgs::Read { path },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::Read { path } => {
                (format!("read_file: {path}"), format!("path: {path}"))
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let fs = self.fs.clone();
        let call_id = call.id.clone();
        let path = match &call.arguments {
            AgentToolArgs::Read { path } => path.clone(),
            _ => return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) }),
        };
        Box::pin(async move {
            let full = fs.root().join(&path);
            match fs.read_text(&full) {
                Ok(content) => Ok(AgentToolResult {
                    tool_call_id: call_id,
                    content,
                    is_error: false,
                    file_change: None,
                }),
                Err(e) => Ok(tool_error(&call_id, &format!("读取失败: {e}"))),
            }
        })
    }
}
