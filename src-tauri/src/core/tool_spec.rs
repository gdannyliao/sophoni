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
use tracing::{info, warn};
use uuid::Uuid;
use walkdir::WalkDir;

use super::acceptance::{list_acceptance_runs, read_acceptance_report, read_runtime_log};
use super::command_risk::{classify_command_with_level, shell_words, CommandAction, RiskLevel};
use super::domain::{
    AgentToolArgs, AgentToolCall, AgentToolName, AgentToolResult, AgentToolSchema, ChangeKind,
    FileChange, MultiEdit,
};
use super::errors::{AppError, AppResult};
use super::tools::ConfirmHandler;
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

// ── write_file ──

pub struct WriteFileTool {
    fs: WorkspaceFs,
}

impl ToolSpec for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::Write { path, content } => {
                serde_json::json!({ "path": path, "content": content })
            }
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let path = req_str(args, "path", "write_file")?;
        let content = req_str(args, "content", "write_file")?;
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::WriteFile,
            arguments: AgentToolArgs::Write { path, content },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::Write { path, content } => (
                format!("write_file: {path}"),
                format!(
                    "path: {path}\ncontent ({} 行):\n{}",
                    content.lines().count().max(1),
                    content
                ),
            ),
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let fs = self.fs.clone();
        let call_id = call.id.clone();
        let (path, content) = match &call.arguments {
            AgentToolArgs::Write { path, content } => (path.clone(), content.clone()),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let full = fs.root().join(&path);
            let write = match fs.write_text_with_snapshot(&full, &content) {
                Ok(w) => w,
                Err(e) => return Ok(tool_error(&call_id, &format!("写入失败: {e}"))),
            };
            let existed = !write.previous_text.is_empty();
            let change = FileChange {
                id: Uuid::new_v4(),
                task_run_id: Uuid::new_v4(),
                path: path.clone(),
                kind: if existed {
                    ChangeKind::Modified
                } else {
                    ChangeKind::Created
                },
                diff: write.diff,
                created_at: Utc::now(),
            };
            Ok(AgentToolResult {
                tool_call_id: call_id,
                content: format!("已写入 {} ({} 行)", path, content.lines().count().max(1)),
                is_error: false,
                file_change: Some(change),
            })
        })
    }
}

// ── list_files ──

pub struct ListFilesTool {
    fs: WorkspaceFs,
}

impl ToolSpec for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::ListFiles { path, recursive } => {
                serde_json::json!({ "path": path, "recursive": recursive })
            }
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let path = opt_str(args, "path");
        let recursive = opt_bool(args, "recursive");
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::ListFiles,
            arguments: AgentToolArgs::ListFiles { path, recursive },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::ListFiles { path, recursive } => {
                let p = path.as_deref().unwrap_or(".");
                (
                    format!("list_files: {p} (recursive={recursive})"),
                    format!("path: {p}\nrecursive: {recursive}"),
                )
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let call_id = call.id.clone();
        let (path, recursive) = match &call.arguments {
            AgentToolArgs::ListFiles { path, recursive } => (path.clone(), *recursive),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let target = match path.as_deref() {
                Some(p) => match resolve_within_root(&root, p) {
                    Ok(t) => t,
                    Err(e) => return Ok(tool_error(&call_id, &e)),
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
                let kind = if entry.file_type().is_dir() {
                    "dir"
                } else {
                    "file"
                };
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
                tool_call_id: call_id,
                content,
                is_error: false,
                file_change: None,
            })
        })
    }
}

// ── grep ──

pub struct GrepTool {
    fs: WorkspaceFs,
}

impl ToolSpec for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn schema(&self) -> AgentToolSchema {
        AgentToolSchema {
            name: "grep",
            description:
                "在工作区内搜索匹配正则表达式的文件内容。返回 path:line:content 格式的结果。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "正则表达式" },
                    "path": { "type": "string", "description": "限定搜索的目录或文件，默认整个工作区" },
                    "include": { "type": "string", "description": "文件名 glob 过滤，如 *.ts" }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::Grep {
                pattern,
                path,
                include,
            } => serde_json::json!({ "pattern": pattern, "path": path, "include": include }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let pattern = req_str(args, "pattern", "grep")?;
        let path = opt_str(args, "path");
        let include = opt_str(args, "include");
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::Grep,
            arguments: AgentToolArgs::Grep {
                pattern,
                path,
                include,
            },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::Grep {
                pattern,
                path,
                include,
            } => {
                let p = path.as_deref().unwrap_or(".");
                let inc = include.as_deref().unwrap_or("(无)");
                (
                    format!("grep: /{pattern}/ in {p}"),
                    format!("pattern: {pattern}\npath: {p}\ninclude: {inc}"),
                )
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let call_id = call.id.clone();
        let (pattern, path, include) = match &call.arguments {
            AgentToolArgs::Grep {
                pattern,
                path,
                include,
            } => (pattern.clone(), path.clone(), include.clone()),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let re = match regex::Regex::new(&pattern) {
                Ok(re) => re,
                Err(e) => return Ok(tool_error(&call_id, &format!("正则编译失败: {e}"))),
            };
            let search_root = match path.as_deref() {
                Some(p) => match resolve_within_root(&root, p) {
                    Ok(t) => t,
                    Err(e) => return Ok(tool_error(&call_id, &e)),
                },
                None => root.clone(),
            };
            let include_glob = include.as_deref().and_then(|g| glob::Pattern::new(g).ok());
            let mut matches = Vec::new();
            let walker = WalkDir::new(&search_root)
                .follow_links(false)
                .max_depth(MAX_DEPTH)
                .into_iter()
                .filter_entry(|e| !is_ignored_dir(e));
            for entry in walker {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let too_big = entry
                    .metadata()
                    .map(|m| m.len() > MAX_FILE_BYTES)
                    .unwrap_or(true);
                if too_big {
                    continue;
                }
                if let Some(ref g) = include_glob {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if !g.matches(&fname) {
                        continue;
                    }
                }
                let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                let content = match std::fs::read_to_string(entry.path()) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                for (lineno, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        matches.push(format!("{}:{}: {}", rel.display(), lineno + 1, line));
                        if matches.len() >= GREP_MAX {
                            break;
                        }
                    }
                }
                if matches.len() >= GREP_MAX {
                    break;
                }
            }
            let truncated = matches.len() >= GREP_MAX;
            let mut output = matches.join("\n");
            if output.is_empty() {
                output = "（无匹配）".to_string();
            }
            if truncated {
                output.push_str(&format!(
                    "\n（结果已截断，只显示前 {GREP_MAX} 条匹配。请缩小搜索范围或用更精确的模式）"
                ));
            }
            Ok(AgentToolResult {
                tool_call_id: call_id,
                content: output,
                is_error: false,
                file_change: None,
            })
        })
    }
}

// ── edit_file ──

pub struct EditFileTool {
    fs: WorkspaceFs,
}

impl ToolSpec for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
            } => serde_json::json!({
                "path": path,
                "old_string": old_string,
                "new_string": new_string,
                "replace_all": replace_all
            }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let path = req_str(args, "path", "edit_file")?;
        let old_string = req_str(args, "old_string", "edit_file")?;
        let new_string = req_str(args, "new_string", "edit_file")?;
        let replace_all = opt_bool(args, "replace_all");
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::EditFile,
            arguments: AgentToolArgs::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
            },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
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
                    format!("edit_file: {} (replace_all={})", path, replace_all),
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
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let fs = self.fs.clone();
        let call_id = call.id.clone();
        let (path, old_string, new_string, replace_all) = match &call.arguments {
            AgentToolArgs::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
            } => (
                path.clone(),
                old_string.clone(),
                new_string.clone(),
                *replace_all,
            ),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            if old_string == new_string {
                return Ok(tool_error(&call_id, "old_string 和 new_string 相同,无需替换"));
            }
            let full = fs.root().join(&path);
            let content = match fs.read_text(&full) {
                Ok(c) => c,
                Err(e) => return Ok(tool_error(&call_id, &format!("读取失败: {e}"))),
            };
            let actual_old = match find_actual_string(&content, &old_string) {
                Some(s) => s,
                None => {
                    return Ok(tool_error(
                        &call_id,
                        "未找到匹配的文本。请先 read_file 确认当前内容。",
                    ))
                }
            };
            let match_count = content.matches(actual_old.as_str()).count();
            if match_count > 1 && !replace_all {
                return Ok(tool_error(
                    &call_id,
                    &format!(
                        "找到 {match_count} 处匹配,请提供更多上下文使 old_string 唯一,或设 replace_all=true"
                    ),
                ));
            }
            let updated = if replace_all {
                content.replace(&actual_old, &new_string)
            } else {
                content.replacen(&actual_old, &new_string, 1)
            };
            let write = match fs.write_text_with_snapshot(&full, &updated) {
                Ok(w) => w,
                Err(e) => return Ok(tool_error(&call_id, &format!("写入失败: {e}"))),
            };
            let change = FileChange {
                id: Uuid::new_v4(),
                task_run_id: Uuid::new_v4(),
                path: path.clone(),
                kind: ChangeKind::Modified,
                diff: write.diff,
                created_at: Utc::now(),
            };
            let summary = if replace_all {
                format!("已替换 {path} 中全部 {match_count} 处匹配")
            } else {
                format!("已替换 {path} 中的 1 处")
            };
            Ok(AgentToolResult {
                tool_call_id: call_id,
                content: summary,
                is_error: false,
                file_change: Some(change),
            })
        })
    }
}

// ── multi_edit_file ──

pub struct MultiEditFileTool {
    fs: WorkspaceFs,
}

impl ToolSpec for MultiEditFileTool {
    fn name(&self) -> &'static str {
        "multi_edit_file"
    }
    fn schema(&self) -> AgentToolSchema {
        AgentToolSchema {
            name: "multi_edit_file",
            description: "对同一文件按顺序应用多处精确替换。每处给出 old_string(精确匹配,含缩进)和 new_string。按顺序应用，任一处失败则整体不写入。适合一次改动多处的场景，比多次 edit_file 省轮次。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": { "type": "string", "description": "要替换的文本(精确匹配)" },
                                "new_string": { "type": "string", "description": "替换成的文本" },
                                "replace_all": { "type": "boolean", "description": "该处 old_string 出现多次时是否全部替换，默认 false" }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::MultiEditFile { path, edits } => {
                serde_json::json!({ "path": path, "edits": edits })
            }
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let tool = "multi_edit_file";
        let path = req_str(args, "path", tool)?;
        let edits_val = args
            .get("edits")
            .ok_or_else(|| AppError::Provider(format!("{tool} missing edits")))?;
        let edits: Vec<MultiEdit> = serde_json::from_value(edits_val.clone())
            .map_err(|e| AppError::Provider(format!("{tool} invalid edits: {e}")))?;
        if edits.is_empty() {
            return Err(AppError::Provider(format!("{tool} edits 不能为空")));
        }
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::MultiEditFile,
            arguments: AgentToolArgs::MultiEditFile { path, edits },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::MultiEditFile { path, edits } => (
                format!("multi_edit_file: {path}"),
                format!("path: {path}\nedits: {} 处", edits.len()),
            ),
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let fs = self.fs.clone();
        let call_id = call.id.clone();
        let (path, edits) = match &call.arguments {
            AgentToolArgs::MultiEditFile { path, edits } => (path.clone(), edits.clone()),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            if edits.is_empty() {
                return Ok(tool_error(&call_id, "edits 不能为空"));
            }
            let full = fs.root().join(&path);
            let mut content = match fs.read_text(&full) {
                Ok(c) => c,
                Err(e) => return Ok(tool_error(&call_id, &format!("读取失败: {e}"))),
            };
            for (i, edit) in edits.iter().enumerate() {
                if edit.old_string == edit.new_string {
                    return Ok(tool_error(
                        &call_id,
                        &format!("第 {} 个 edit: old_string 和 new_string 相同", i + 1),
                    ));
                }
                let actual_old = match find_actual_string(&content, &edit.old_string) {
                    Some(s) => s,
                    None => {
                        return Ok(tool_error(
                            &call_id,
                            &format!("第 {} 个 edit: 未找到匹配的文本", i + 1),
                        ));
                    }
                };
                let match_count = content.matches(actual_old.as_str()).count();
                if match_count > 1 && !edit.replace_all {
                    return Ok(tool_error(
                        &call_id,
                        &format!(
                            "第 {} 个 edit: 找到 {match_count} 处匹配,请提供更多上下文使 old_string 唯一,或设 replace_all=true",
                            i + 1
                        ),
                    ));
                }
                content = if edit.replace_all {
                    content.replace(&actual_old, &edit.new_string)
                } else {
                    content.replacen(&actual_old, &edit.new_string, 1)
                };
            }
            let write = match fs.write_text_with_snapshot(&full, &content) {
                Ok(w) => w,
                Err(e) => return Ok(tool_error(&call_id, &format!("写入失败: {e}"))),
            };
            let change = FileChange {
                id: Uuid::new_v4(),
                task_run_id: Uuid::new_v4(),
                path: path.clone(),
                kind: ChangeKind::Modified,
                diff: write.diff,
                created_at: Utc::now(),
            };
            Ok(AgentToolResult {
                tool_call_id: call_id,
                content: format!("已替换 {path} 中 {} 处", edits.len()),
                is_error: false,
                file_change: Some(change),
            })
        })
    }
}

// ── delete_file ──

pub struct DeleteFileTool {
    fs: WorkspaceFs,
}

impl ToolSpec for DeleteFileTool {
    fn name(&self) -> &'static str {
        "delete_file"
    }
    fn schema(&self) -> AgentToolSchema {
        AgentToolSchema {
            name: "delete_file",
            description: "删除工作区内指定文件。删除目录请用 run_command（rmdir）。删除不可撤销。".into(),
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
            AgentToolArgs::DeleteFile { path } => serde_json::json!({ "path": path }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let path = req_str(args, "path", "delete_file")?;
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::DeleteFile,
            arguments: AgentToolArgs::DeleteFile { path },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::DeleteFile { path } => {
                (format!("delete_file: {path}"), format!("path: {path}"))
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let fs = self.fs.clone();
        let call_id = call.id.clone();
        let path = match &call.arguments {
            AgentToolArgs::DeleteFile { path } => path.clone(),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let full = match resolve_within_root(&root, &path) {
                Ok(f) => f,
                Err(e) => return Ok(tool_error(&call_id, &e)),
            };
            let old_content = match fs.read_text(&full) {
                Ok(c) => c,
                Err(e) => return Ok(tool_error(&call_id, &format!("读取失败: {e}"))),
            };
            if let Err(e) = std::fs::remove_file(&full) {
                return Ok(tool_error(&call_id, &format!("删除失败: {e}")));
            }
            let change = FileChange {
                id: Uuid::new_v4(),
                task_run_id: Uuid::new_v4(),
                path: path.clone(),
                kind: ChangeKind::Deleted,
                diff: super::diff::unified_diff(&old_content, ""),
                created_at: Utc::now(),
            };
            Ok(AgentToolResult {
                tool_call_id: call_id,
                content: format!("已删除 {path}"),
                is_error: false,
                file_change: Some(change),
            })
        })
    }
}

// ════════════════════════════════════════════════════════════════
// 命令工具（1 个）：run_command
// ════════════════════════════════════════════════════════════════

pub struct RunCommandTool {
    fs: WorkspaceFs,
    risk_level: RiskLevel,
    confirm_handler: Option<Arc<dyn ConfirmHandler>>,
}

impl RunCommandTool {
    pub fn new(
        fs: WorkspaceFs,
        risk_level: RiskLevel,
        confirm_handler: Option<Arc<dyn ConfirmHandler>>,
    ) -> Self {
        Self {
            fs,
            risk_level,
            confirm_handler,
        }
    }
}

impl ToolSpec for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }
    fn schema(&self) -> AgentToolSchema {
        AgentToolSchema {
            name: "run_command",
            description: super::agent::command_description(self.risk_level),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的命令（如 cargo test）" }
                },
                "required": ["command"]
            }),
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::RunCommand { command } => serde_json::json!({ "command": command }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let command = req_str(args, "command", "run_command")?;
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::RunCommand,
            arguments: AgentToolArgs::RunCommand { command },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::RunCommand { command } => {
                (format!("run_command: {command}"), format!("command: {command}"))
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let risk_level = self.risk_level;
        let confirm_handler = self.confirm_handler.clone();
        let call_id = call.id.clone();
        let command = match &call.arguments {
            AgentToolArgs::RunCommand { command } => command.clone(),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let action = classify_command_with_level(
                &command,
                root.to_str().unwrap_or(""),
                risk_level,
            );
            let command_to_run = match action {
                CommandAction::Allow => command.clone(),
                CommandAction::Deny(reason) => {
                    return Ok(tool_error(
                        &call_id,
                        &format!("命令被拒绝({reason}): {command}"),
                    ));
                }
                CommandAction::RequireConfirm => match &confirm_handler {
                    None => {
                        return Ok(tool_error(
                            &call_id,
                            &format!("命令需要确认但无确认处理器: {command}"),
                        ));
                    }
                    Some(handler) => {
                        let allowed = handler.confirm(&command, "高风险命令").await;
                        if !allowed {
                            return Ok(tool_error(&call_id, &format!("命令被用户拒绝: {command}")));
                        }
                        command.clone()
                    }
                },
            };
            let argv = match shell_words(&command_to_run) {
                v if v.is_empty() => return Ok(tool_error(&call_id, "空命令")),
                v => v,
            };
            let output = tokio::time::timeout(
                Duration::from_secs(30),
                tokio::process::Command::new(&argv[0])
                    .args(&argv[1..])
                    .current_dir(&root)
                    .output(),
            )
            .await;
            match output {
                Ok(Ok(out)) => {
                    let stdout = truncate_output(&String::from_utf8_lossy(&out.stdout), 100, 4000);
                    let stderr = truncate_output(&String::from_utf8_lossy(&out.stderr), 50, 2000);
                    let exit_code = out.status.code().unwrap_or(-1);
                    info!(%command, exit_code, "run_command");
                    let content = format!(
                        "exit code: {exit_code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                    );
                    let is_error = exit_code != 0;
                    Ok(AgentToolResult {
                        tool_call_id: call_id,
                        content,
                        is_error,
                        file_change: None,
                    })
                }
                Ok(Err(e)) => Ok(tool_error(&call_id, &format!("执行失败: {e}"))),
                Err(_) => Ok(tool_error(&call_id, "命令超时(30s),已被终止")),
            }
        })
    }
}

// ════════════════════════════════════════════════════════════════
// 验收类工具（3 个）：read_acceptance_report / read_runtime_log / list_acceptance_runs
// ════════════════════════════════════════════════════════════════

// ── read_acceptance_report ──

pub struct ReadAcceptanceReportTool {
    fs: WorkspaceFs,
}

impl ToolSpec for ReadAcceptanceReportTool {
    fn name(&self) -> &'static str {
        "read_acceptance_report"
    }
    fn schema(&self) -> AgentToolSchema {
        AgentToolSchema {
            name: "read_acceptance_report",
            description:
                "读取验收运行的 report.json。默认读取最新一次验收运行；用于判断 ok 和 failureSummary。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "验收运行 ID；省略时读取最新一次" }
                }
            }),
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::ReadAcceptanceReport { run_id } => serde_json::json!({ "run_id": run_id }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let run_id = opt_str(args, "run_id");
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::ReadAcceptanceReport,
            arguments: AgentToolArgs::ReadAcceptanceReport { run_id },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::ReadAcceptanceReport { run_id } => {
                let id = run_id.as_deref().unwrap_or("latest");
                (
                    format!("read_acceptance_report: {id}"),
                    format!("run_id: {id}"),
                )
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let call_id = call.id.clone();
        let run_id = match &call.arguments {
            AgentToolArgs::ReadAcceptanceReport { run_id } => run_id.clone(),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            match read_acceptance_report(&root, run_id.as_deref()) {
                Ok(content) => Ok(AgentToolResult {
                    tool_call_id: call_id,
                    content: cap_text_bytes(content, ACCEPTANCE_REPORT_MAX_BYTES),
                    is_error: false,
                    file_change: None,
                }),
                Err(e) => Ok(tool_error(&call_id, &format!("读取验收报告失败: {e}"))),
            }
        })
    }
}

// ── read_runtime_log ──

pub struct ReadRuntimeLogTool {
    fs: WorkspaceFs,
}

impl ToolSpec for ReadRuntimeLogTool {
    fn name(&self) -> &'static str {
        "read_runtime_log"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::ReadRuntimeLog {
                run_id,
                file_name,
                max_lines,
            } => serde_json::json!({
                "run_id": run_id,
                "file_name": file_name,
                "max_lines": max_lines
            }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let tool = "read_runtime_log";
        let run_id = opt_str(args, "run_id");
        let file_name = req_str(args, "file_name", tool)?;
        let max_lines = args
            .get("max_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(1, READ_RUNTIME_LOG_MAX_LINES) as usize)
            .unwrap_or(READ_RUNTIME_LOG_DEFAULT_MAX_LINES);
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::ReadRuntimeLog,
            arguments: AgentToolArgs::ReadRuntimeLog {
                run_id,
                file_name,
                max_lines,
            },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::ReadRuntimeLog {
                run_id,
                file_name,
                max_lines,
            } => {
                let id = run_id.as_deref().unwrap_or("latest");
                (
                    format!("read_runtime_log: {file_name} ({id}, max_lines={max_lines})"),
                    format!("run_id: {id}\nfile_name: {file_name}\nmax_lines: {max_lines}"),
                )
            }
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let call_id = call.id.clone();
        let (run_id, file_name, max_lines) = match &call.arguments {
            AgentToolArgs::ReadRuntimeLog {
                run_id,
                file_name,
                max_lines,
            } => (run_id.clone(), file_name.clone(), *max_lines),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            match read_runtime_log(&root, run_id.as_deref(), &file_name, max_lines) {
                Ok(content) => Ok(AgentToolResult {
                    tool_call_id: call_id,
                    content: cap_text_bytes(content, ACCEPTANCE_LOG_MAX_BYTES),
                    is_error: false,
                    file_change: None,
                }),
                Err(e) => Ok(tool_error(&call_id, &format!("读取运行日志失败: {e}"))),
            }
        })
    }
}

// ── list_acceptance_runs ──

pub struct ListAcceptanceRunsTool {
    fs: WorkspaceFs,
}

impl ToolSpec for ListAcceptanceRunsTool {
    fn name(&self) -> &'static str {
        "list_acceptance_runs"
    }
    fn schema(&self) -> AgentToolSchema {
        AgentToolSchema {
            name: "list_acceptance_runs",
            description: "列出最近验收运行 ID。limit 会限制到 1 到 20。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 20, "description": "返回条数，默认 5" }
                }
            }),
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::ListAcceptanceRuns { limit } => serde_json::json!({ "limit": limit }),
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(1, LIST_ACCEPTANCE_RUNS_MAX_LIMIT) as usize)
            .unwrap_or(LIST_ACCEPTANCE_RUNS_DEFAULT_LIMIT);
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::ListAcceptanceRuns,
            arguments: AgentToolArgs::ListAcceptanceRuns { limit },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::ListAcceptanceRuns { limit } => (
                format!("list_acceptance_runs: limit={limit}"),
                format!("limit: {limit}"),
            ),
            _ => unreachable!(),
        }
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let root = self.fs.root().to_path_buf();
        let call_id = call.id.clone();
        let limit = match &call.arguments {
            AgentToolArgs::ListAcceptanceRuns { limit } => *limit,
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let limit = limit.clamp(ACCEPTANCE_RUNS_LIMIT_MIN, ACCEPTANCE_RUNS_LIMIT_MAX);
            match list_acceptance_runs(&root, limit) {
                Ok(runs) => Ok(AgentToolResult {
                    tool_call_id: call_id,
                    content: if runs.is_empty() {
                        "（无验收运行记录）".to_string()
                    } else {
                        runs.join("\n")
                    },
                    is_error: false,
                    file_change: None,
                }),
                Err(e) => Ok(tool_error(&call_id, &format!("列出验收运行失败: {e}"))),
            }
        })
    }
}

// ════════════════════════════════════════════════════════════════
// 网络工具（2 个）：web_search / web_fetch —— ChatOnly 下仍可用
// ════════════════════════════════════════════════════════════════

// ── web_search ──

pub struct WebSearchTool {
    search_config: Option<SearchConfig>,
    http_client: reqwest::Client,
}

impl ToolSpec for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::WebSearch { query, max_results } => {
                serde_json::json!({ "query": query, "max_results": max_results })
            }
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let query = req_str(args, "query", "web_search")?;
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::WebSearch,
            arguments: AgentToolArgs::WebSearch { query, max_results },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::WebSearch { query, max_results } => (
                format!("web_search: {query}"),
                format!("query: {query}\nmax_results: {max_results}"),
            ),
            _ => unreachable!(),
        }
    }
    fn available_in_chat_only(&self) -> bool {
        true
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let config = self.search_config.clone();
        let http = self.http_client.clone();
        let call_id = call.id.clone();
        let (query, max_results) = match &call.arguments {
            AgentToolArgs::WebSearch { query, max_results } => (query.clone(), *max_results),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            let config = match &config {
                Some(c) => c,
                None => {
                    return Ok(tool_error(
                        &call_id,
                        "未配置搜索 API，请在设置里配置 Tavily 或 Google CSE key",
                    ));
                }
            };
            let backend = match web::select_backend(config) {
                Some(b) => b,
                None => {
                    return Ok(tool_error(
                        &call_id,
                        "搜索后端配置不完整（Tavily 需要 tavily_key；Google 需要 google_key + google_cx）",
                    ));
                }
            };
            match backend.search(&http, &query, max_results).await {
                Ok(results) => Ok(AgentToolResult {
                    tool_call_id: call_id,
                    content: web::format_results(&results),
                    is_error: false,
                    file_change: None,
                }),
                Err(e) => Ok(tool_error(&call_id, &format!("搜索失败: {e}"))),
            }
        })
    }
}

// ── web_fetch ──

pub struct WebFetchTool {
    http_client: reqwest::Client,
}

impl ToolSpec for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }
    fn schema(&self) -> AgentToolSchema {
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
        }
    }
    fn serialize_args(&self, args: &AgentToolArgs) -> serde_json::Value {
        match args {
            AgentToolArgs::WebFetch { url, max_chars } => {
                serde_json::json!({ "url": url, "max_chars": max_chars })
            }
            _ => unreachable!(),
        }
    }
    fn parse(&self, id: &str, args: &serde_json::Value) -> AppResult<AgentToolCall> {
        let url = req_str(args, "url", "web_fetch")?;
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(8000) as usize;
        Ok(AgentToolCall {
            id: id.to_string(),
            name: AgentToolName::WebFetch,
            arguments: AgentToolArgs::WebFetch { url, max_chars },
        })
    }
    fn describe(&self, args: &AgentToolArgs) -> (String, String) {
        match args {
            AgentToolArgs::WebFetch { url, max_chars } => {
                (format!("web_fetch: {url}"), format!("url: {url}\nmax_chars: {max_chars}"))
            }
            _ => unreachable!(),
        }
    }
    fn available_in_chat_only(&self) -> bool {
        true
    }
    fn dispatch(&self, call: &AgentToolCall) -> ToolFuture {
        let http = self.http_client.clone();
        let call_id = call.id.clone();
        let (url, max_chars) = match &call.arguments {
            AgentToolArgs::WebFetch { url, max_chars } => (url.clone(), *max_chars),
            _ => {
                return Box::pin(async move { Ok(tool_error(&call_id, "参数不匹配")) });
            }
        };
        Box::pin(async move {
            match web::web_fetch(&http, &url, max_chars).await {
                Ok(content) => Ok(AgentToolResult {
                    tool_call_id: call_id,
                    content,
                    is_error: false,
                    file_change: None,
                }),
                Err(e) => Ok(tool_error(&call_id, &e.to_string())),
            }
        })
    }
}

// ════════════════════════════════════════════════════════════════
// Registry：全部工具的集合。每次请求构造（持有 fs 等会话级依赖）。
// ════════════════════════════════════════════════════════════════

pub type ToolRegistry = Vec<Box<dyn ToolSpec>>;

/// 按 name 查找工具。未注册返回 None。
pub fn find_tool<'a>(registry: &'a ToolRegistry, name: &str) -> Option<&'a dyn ToolSpec> {
    registry.iter().find(|t| t.name() == name).map(|b| &**b)
}

/// 构造全部 13 个工具的 registry。每次请求构造（fs / confirm_handler 是会话级依赖）。
#[allow(clippy::too_many_arguments)]
pub fn build_tool_registry(
    fs: WorkspaceFs,
    risk_level: RiskLevel,
    confirm_handler: Option<Arc<dyn ConfirmHandler>>,
    search_config: Option<SearchConfig>,
    http_client: reqwest::Client,
) -> ToolRegistry {
    vec![
        Box::new(ReadFileTool { fs: fs.clone() }),
        Box::new(WriteFileTool { fs: fs.clone() }),
        Box::new(ListFilesTool { fs: fs.clone() }),
        Box::new(GrepTool { fs: fs.clone() }),
        Box::new(EditFileTool { fs: fs.clone() }),
        Box::new(MultiEditFileTool { fs: fs.clone() }),
        Box::new(DeleteFileTool { fs: fs.clone() }),
        Box::new(RunCommandTool::new(fs.clone(), risk_level, confirm_handler.clone())),
        Box::new(ReadAcceptanceReportTool { fs: fs.clone() }),
        Box::new(ReadRuntimeLogTool { fs: fs.clone() }),
        Box::new(ListAcceptanceRunsTool { fs }),
        Box::new(WebSearchTool {
            search_config,
            http_client: http_client.clone(),
        }),
        Box::new(WebFetchTool { http_client }),
    ]
}

/// 给所有工具生成 schema 列表。ChatOnly 模式下只含 `available_in_chat_only` 的工具。
pub fn tool_schemas(registry: &ToolRegistry, mode: super::tools::WorkspaceMode) -> Vec<AgentToolSchema> {
    registry
        .iter()
        .filter(|t| {
            mode == super::tools::WorkspaceMode::Full || t.available_in_chat_only()
        })
        .map(|t| t.schema())
        .collect()
}

/// 聚合 dispatch：按 name 查找工具执行。ChatOnly 模式下，非 chat_only 工具返回友好错误。
/// （网络工具不受 ChatOnly 拦截。）
pub async fn dispatch(
    registry: &ToolRegistry,
    mode: super::tools::WorkspaceMode,
    call: &AgentToolCall,
) -> AppResult<AgentToolResult> {
    let name = wire_name(&call.name);
    let spec = find_tool(registry, name).ok_or_else(|| {
        AppError::Tool(format!("unknown tool: {name}"))
    })?;
    // ChatOnly 拦截：非 chat_only 工具直接拒绝
    if mode == super::tools::WorkspaceMode::ChatOnly && !spec.available_in_chat_only() {
        warn!(tool = ?call.name, "dispatch blocked: ChatOnly mode");
        return Ok(AgentToolResult {
            tool_call_id: call.id.clone(),
            content: "未选择工作区，此操作不可用。请在左侧选择工作区。".to_string(),
            is_error: true,
            file_change: None,
        });
    }
    info!(tool = ?call.name, "dispatch");
    spec.dispatch(call).await
}

/// AgentToolName → wire 字符串
pub fn wire_name(name: &AgentToolName) -> &'static str {
    match name {
        AgentToolName::ReadFile => "read_file",
        AgentToolName::WriteFile => "write_file",
        AgentToolName::ListFiles => "list_files",
        AgentToolName::Grep => "grep",
        AgentToolName::EditFile => "edit_file",
        AgentToolName::MultiEditFile => "multi_edit_file",
        AgentToolName::DeleteFile => "delete_file",
        AgentToolName::RunCommand => "run_command",
        AgentToolName::ReadAcceptanceReport => "read_acceptance_report",
        AgentToolName::ReadRuntimeLog => "read_runtime_log",
        AgentToolName::ListAcceptanceRuns => "list_acceptance_runs",
        AgentToolName::WebSearch => "web_search",
        AgentToolName::WebFetch => "web_fetch",
        AgentToolName::CreateScheduledTask => "create_scheduled_task",
        AgentToolName::ListScheduledTasks => "list_scheduled_tasks",
        AgentToolName::DeleteScheduledTask => "delete_scheduled_task",
    }
}

#[cfg(test)]
mod tests {
    use super::super::command_risk::RiskLevel;
    use super::super::domain::{AgentToolArgs, AgentToolCall, AgentToolName, ChangeKind, MultiEdit};
    use super::super::tools::{ConfirmHandler, WorkspaceMode};
    use super::super::workspace::WorkspaceFs;
    use super::{build_tool_registry, dispatch, ToolRegistry};
    use std::sync::Arc;

    /// 构造标准测试 registry（Standard 风险、无 confirm、无 search）。
    fn registry(root: &std::path::Path) -> Arc<ToolRegistry> {
        let fs = WorkspaceFs::new(root.to_path_buf());
        let http = reqwest::Client::new();
        Arc::new(build_tool_registry(fs, RiskLevel::Standard, None, None, http))
    }

    fn read_call(path: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-1".to_string(),
            name: AgentToolName::ReadFile,
            arguments: AgentToolArgs::Read { path: path.to_string() },
        }
    }

    fn write_call(path: &str, content: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-2".to_string(),
            name: AgentToolName::WriteFile,
            arguments: AgentToolArgs::Write {
                path: path.to_string(),
                content: content.to_string(),
            },
        }
    }

    fn list_call(path: Option<&str>, recursive: bool) -> AgentToolCall {
        AgentToolCall {
            id: "call-list".to_string(),
            name: AgentToolName::ListFiles,
            arguments: AgentToolArgs::ListFiles {
                path: path.map(String::from),
                recursive,
            },
        }
    }

    fn grep_call(pattern: &str, path: Option<&str>, include: Option<&str>) -> AgentToolCall {
        AgentToolCall {
            id: "call-grep".to_string(),
            name: AgentToolName::Grep,
            arguments: AgentToolArgs::Grep {
                pattern: pattern.to_string(),
                path: path.map(String::from),
                include: include.map(String::from),
            },
        }
    }

    fn edit_call(path: &str, old: &str, new: &str, replace_all: bool) -> AgentToolCall {
        AgentToolCall {
            id: "call-edit".to_string(),
            name: AgentToolName::EditFile,
            arguments: AgentToolArgs::EditFile {
                path: path.to_string(),
                old_string: old.to_string(),
                new_string: new.to_string(),
                replace_all,
            },
        }
    }

    fn multi_edit_call(path: &str, edits: Vec<MultiEdit>) -> AgentToolCall {
        AgentToolCall {
            id: "call-multi".to_string(),
            name: AgentToolName::MultiEditFile,
            arguments: AgentToolArgs::MultiEditFile {
                path: path.to_string(),
                edits,
            },
        }
    }

    fn edit(old: &str, new: &str) -> MultiEdit {
        MultiEdit {
            old_string: old.to_string(),
            new_string: new.to_string(),
            replace_all: false,
        }
    }

    fn delete_call(path: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-delete".to_string(),
            name: AgentToolName::DeleteFile,
            arguments: AgentToolArgs::DeleteFile { path: path.to_string() },
        }
    }

    fn cmd_call(command: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-cmd".to_string(),
            name: AgentToolName::RunCommand,
            arguments: AgentToolArgs::RunCommand { command: command.to_string() },
        }
    }

    fn web_search_call(query: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-ws".to_string(),
            name: AgentToolName::WebSearch,
            arguments: AgentToolArgs::WebSearch {
                query: query.to_string(),
                max_results: 5,
            },
        }
    }

    fn web_fetch_call(url: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-wf".to_string(),
            name: AgentToolName::WebFetch,
            arguments: AgentToolArgs::WebFetch {
                url: url.to_string(),
                max_chars: 8000,
            },
        }
    }

    fn read_acceptance_report_call(run_id: Option<&str>) -> AgentToolCall {
        AgentToolCall {
            id: "call-acceptance-report".to_string(),
            name: AgentToolName::ReadAcceptanceReport,
            arguments: AgentToolArgs::ReadAcceptanceReport { run_id: run_id.map(String::from) },
        }
    }

    fn read_runtime_log_call(run_id: Option<&str>, file_name: &str, max_lines: usize) -> AgentToolCall {
        AgentToolCall {
            id: "call-runtime-log".to_string(),
            name: AgentToolName::ReadRuntimeLog,
            arguments: AgentToolArgs::ReadRuntimeLog {
                run_id: run_id.map(String::from),
                file_name: file_name.to_string(),
                max_lines,
            },
        }
    }

    fn list_acceptance_runs_call(limit: usize) -> AgentToolCall {
        AgentToolCall {
            id: "call-list-acceptance".to_string(),
            name: AgentToolName::ListAcceptanceRuns,
            arguments: AgentToolArgs::ListAcceptanceRuns { limit },
        }
    }

    // ── read_file ──

    #[tokio::test]
    async fn tool_read_file_returns_content() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hello.txt"), "hi there\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &read_call("hello.txt")).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "hi there\n");
        assert!(result.file_change.is_none());
    }

    #[tokio::test]
    async fn tool_read_nonexistent_returns_error_result_not_panic() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &read_call("nope.txt")).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.is_error);
    }

    // ── write_file ──

    #[tokio::test]
    async fn tool_write_file_creates_and_returns_file_change() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &write_call("out.txt", "new content\n"))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("out.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "new content\n");
        let change = result.file_change.expect("write should produce file_change");
        assert_eq!(change.path, "out.txt");
        assert!(change.diff.contains("+new content"));
    }

    // ── list_files ──

    #[tokio::test]
    async fn list_files_empty_dir_returns_placeholder() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &list_call(None, false)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("空目录"));
    }

    #[tokio::test]
    async fn list_files_lists_files_and_dirs() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("subdir")).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &list_call(None, false)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("subdir"));
    }

    #[tokio::test]
    async fn list_files_ignores_node_modules() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(root.join("real.txt"), "y").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &list_call(None, true)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.content.contains("node_modules"));
        assert!(result.content.contains("real.txt"));
    }

    #[tokio::test]
    async fn list_files_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &list_call(Some("../outside"), false))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    // ── grep ──

    #[tokio::test]
    async fn grep_finds_matches() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.ts"), "const x = invoke(\"foo\");\n").unwrap();
        std::fs::write(root.join("b.ts"), "no match here\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &grep_call("invoke", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("a.ts:1:"));
        assert!(!result.content.contains("b.ts"));
    }

    #[tokio::test]
    async fn grep_include_glob_filter() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.ts"), "invoke\n").unwrap();
        std::fs::write(root.join("b.js"), "invoke\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &grep_call("invoke", None, Some("*.ts")))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.content.contains("a.ts"));
        assert!(!result.content.contains("b.js"));
    }

    #[tokio::test]
    async fn grep_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &grep_call("x", Some("../outside"), None))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    // ── edit_file ──

    #[tokio::test]
    async fn edit_file_basic_replace() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello world\nfoo bar\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &edit_call("a.txt", "world", "Rust", false))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "hello Rust\nfoo bar\n");
        let change = result.file_change.expect("should have file_change");
        assert!(change.diff.contains("-hello world"));
        assert!(change.diff.contains("+hello Rust"));
    }

    #[tokio::test]
    async fn edit_file_not_found_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &edit_call("a.txt", "nonexistent", "x", false))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("未找到"));
    }

    #[tokio::test]
    async fn edit_file_replace_all() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "foo\nfoo\nfoo\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &edit_call("a.txt", "foo", "bar", true))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "bar\nbar\nbar\n");
        assert!(result.content.contains("3 处"));
    }

    // ── multi_edit_file ──

    #[tokio::test]
    async fn multi_edit_file_applies_multiple_edits() {
        let root = std::env::temp_dir().join(format!("sophoni-me-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();

        let r = registry(&root);
        let result = dispatch(
            &r,
            WorkspaceMode::Full,
            &multi_edit_call("a.txt", vec![edit("alpha", "ALPHA"), edit("beta", "BETA"), edit("gamma", "GAMMA")]),
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "ALPHA\nBETA\nGAMMA\n");
        assert!(result.content.contains("3 处"));
    }

    #[tokio::test]
    async fn multi_edit_file_atomic_rollback() {
        let root = std::env::temp_dir().join(format!("sophoni-me-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();

        let r = registry(&root);
        let result = dispatch(
            &r,
            WorkspaceMode::Full,
            &multi_edit_call(
                "a.txt",
                vec![edit("alpha", "ALPHA"), edit("nonexistent", "X"), edit("gamma", "GAMMA")],
            ),
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("第 2 个 edit"));
        assert_eq!(written, "alpha\nbeta\ngamma\n");
    }

    // ── delete_file ──

    #[tokio::test]
    async fn delete_file_removes_file_and_returns_deleted_change() {
        let root = std::env::temp_dir().join(format!("sophoni-df-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("target.txt"), "content\n").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &delete_call("target.txt")).await.unwrap();

        assert!(!result.is_error);
        assert!(!root.join("target.txt").exists(), "文件应被删除");
        let change = result.file_change.expect("应有 file_change");
        assert_eq!(change.kind, ChangeKind::Deleted);
        assert_eq!(change.path, "target.txt");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn delete_file_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-df-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &delete_call("../outside.txt")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(result.content.contains("越界"));
    }

    // ── run_command ──

    #[tokio::test]
    async fn run_command_ls_succeeds() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("visible.txt"), "x").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("ls")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(!result.is_error);
        assert!(result.content.contains("visible.txt"));
    }

    #[tokio::test]
    async fn run_command_high_risk_rejected() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("rm -rf /")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(result.content.contains("高风险"));
    }

    #[tokio::test]
    async fn run_command_empty_rejected() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("   ")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    // ── acceptance 工具 ──

    #[tokio::test]
    async fn tool_reads_latest_acceptance_report() {
        let workspace = super::super::test_support::TempWorkspace::new("tool-acceptance-report");
        workspace.write_run_file("2026-06-14T09-00-00Z", "report.json", r#"{"ok":false}"#);
        workspace.write_run_file(
            "2026-06-15T09-00-00Z",
            "report.json",
            r#"{"ok":true,"failureSummary":null}"#,
        );

        let r = registry(workspace.path());
        let result = dispatch(&r, WorkspaceMode::Full, &read_acceptance_report_call(None))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains(r#""ok":true"#));
        assert!(result.content.contains("failureSummary"));
    }

    #[tokio::test]
    async fn tool_reads_runtime_log_with_max_lines() {
        let workspace = super::super::test_support::TempWorkspace::new("tool-runtime-log");
        workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", r#"{"ok":true}"#);
        workspace.write_run_file(
            "2026-06-15T09-00-00Z",
            "runtime.log",
            "line1\nline2\nline3\nline4\n",
        );

        let r = registry(workspace.path());
        let result = dispatch(
            &r,
            WorkspaceMode::Full,
            &read_runtime_log_call(Some("2026-06-15T09-00-00Z"), "runtime.log", 2),
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "line3\nline4\n");
    }

    #[tokio::test]
    async fn tool_lists_acceptance_runs_empty_returns_placeholder() {
        let workspace = super::super::test_support::TempWorkspace::new("tool-acceptance-empty");
        std::fs::create_dir_all(workspace.path()).unwrap();

        let r = registry(workspace.path());
        let result = dispatch(&r, WorkspaceMode::Full, &list_acceptance_runs_call(5)).await.unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "（无验收运行记录）");
    }

    // ── ChatOnly 模式 ──

    #[tokio::test]
    async fn chat_only_mode_blocks_file_tools() {
        let root = std::env::temp_dir().join(format!("sophoni-chat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("test.txt"), "hello").unwrap();

        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::ChatOnly, &read_call("test.txt")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error, "ChatOnly 模式应阻止文件读取");
        assert!(result.content.contains("未选择工作区"));
    }

    #[tokio::test]
    async fn web_search_not_blocked_by_chat_only() {
        let root = std::env::temp_dir().join(format!("sophoni-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::ChatOnly, &web_search_call("rust")).await.unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(!result.content.contains("未选择工作区"));
        assert!(result.content.contains("未配置"));
    }

    #[tokio::test]
    async fn web_fetch_not_blocked_by_chat_only() {
        let root = std::env::temp_dir().join(format!("sophoni-wf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let r = registry(&root);
        let result = dispatch(&r, WorkspaceMode::ChatOnly, &web_fetch_call("file:///etc/passwd"))
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(!result.content.contains("未选择工作区"));
    }

    // ── ConfirmHandler（Relaxed/Unrestricted 模式）──

    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

    struct MockConfirmHandler {
        allowed: bool,
        called: AtomicBool,
    }

    #[async_trait::async_trait]
    impl ConfirmHandler for MockConfirmHandler {
        async fn confirm(&self, _command: &str, _reason: &str) -> bool {
            self.called.store(true, AtomicOrdering::Relaxed);
            self.allowed
        }
    }

    fn mock_handler(allowed: bool) -> Arc<MockConfirmHandler> {
        Arc::new(MockConfirmHandler {
            allowed,
            called: AtomicBool::new(false),
        })
    }

    fn registry_with(root: &std::path::Path, level: RiskLevel, handler: Option<Arc<dyn ConfirmHandler>>) -> Arc<ToolRegistry> {
        let fs = WorkspaceFs::new(root.to_path_buf());
        let http = reqwest::Client::new();
        Arc::new(build_tool_registry(fs, level, handler, None, http))
    }

    #[tokio::test]
    async fn run_command_relaxed_rm_confirmed_executes() {
        let root = std::env::temp_dir().join(format!("sophoni-confirm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("target.txt"), "x").unwrap();

        let handler = mock_handler(true);
        let r = registry_with(&root, RiskLevel::Relaxed, Some(handler.clone()));
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("rm target.txt")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(!result.is_error, "确认通过后命令应执行成功");
    }

    #[tokio::test]
    async fn run_command_relaxed_rm_denied_returns_rejection() {
        let root = std::env::temp_dir().join(format!("sophoni-confirm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("target.txt"), "x").unwrap();

        let handler = mock_handler(false);
        let r = registry_with(&root, RiskLevel::Relaxed, Some(handler.clone()));
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("rm target.txt")).await.unwrap();

        assert!(result.is_error, "用户拒绝后应返回错误");
        assert!(result.content.contains("用户拒绝"));
        assert!(handler.called.load(AtomicOrdering::Relaxed), "handler 应被调用");
        assert!(root.join("target.txt").exists(), "拒绝后文件不应被删");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_command_standard_denies_rm_without_handler_call() {
        let root = std::env::temp_dir().join(format!("sophoni-confirm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let handler = mock_handler(true);
        let r = registry_with(&root, RiskLevel::Standard, Some(handler.clone()));
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("rm target.txt")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(result.content.contains("被拒绝"));
        assert!(!handler.called.load(AtomicOrdering::Relaxed), "Standard 模式不应调用 handler");
    }

    #[tokio::test]
    async fn run_command_unrestricted_rm_in_workspace_no_confirm() {
        let root = std::env::temp_dir().join(format!("sophoni-confirm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("target.txt"), "x").unwrap();

        let handler = mock_handler(true);
        let r = registry_with(&root, RiskLevel::Unrestricted, Some(handler.clone()));
        let result = dispatch(&r, WorkspaceMode::Full, &cmd_call("rm target.txt")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(!result.is_error, "工作区内 rm 应直接放行");
        assert!(!handler.called.load(AtomicOrdering::Relaxed), "工作区内 rm 不应触发确认");
    }
}
