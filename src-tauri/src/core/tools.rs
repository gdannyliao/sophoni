use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;
use walkdir::WalkDir;

use super::acceptance::{list_acceptance_runs, read_acceptance_report, read_runtime_log};
use super::command_risk::{classify_command_with_level, shell_words, CommandAction, RiskLevel};
use super::domain::{
    AgentToolArgs, AgentToolCall, AgentToolName, AgentToolResult, ChangeKind, FileChange,
};
use super::errors::{AppError, AppResult};
use super::workspace::{lexical_normalize, WorkspaceFs};

const LIST_FILES_MAX: usize = 200;
const GREP_MAX: usize = 100;
const ACCEPTANCE_RUNS_LIMIT_MIN: usize = 1;
const ACCEPTANCE_RUNS_LIMIT_MAX: usize = 20;
const ACCEPTANCE_REPORT_MAX_BYTES: usize = 64 * 1024;
const ACCEPTANCE_LOG_MAX_BYTES: usize = 32 * 1024;
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

#[async_trait]
pub trait ConfirmHandler: Send + Sync {
    async fn confirm(&self, command: &str, reason: &str) -> bool;
}

pub struct ToolDispatcher {
    fs: WorkspaceFs,
    risk_level: RiskLevel,
    confirm_handler: Option<Arc<dyn ConfirmHandler>>,
}

impl ToolDispatcher {
    pub fn new(root: PathBuf) -> Self {
        Self {
            fs: WorkspaceFs::new(root),
            risk_level: RiskLevel::Standard,
            confirm_handler: None,
        }
    }

    pub fn with_risk_level(mut self, level: RiskLevel) -> Self {
        self.risk_level = level;
        self
    }

    pub fn with_confirm_handler(mut self, handler: Arc<dyn ConfirmHandler>) -> Self {
        self.confirm_handler = Some(handler);
        self
    }

    pub fn risk_level(&self) -> RiskLevel {
        self.risk_level
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
            (
                AgentToolName::Grep,
                AgentToolArgs::Grep {
                    pattern,
                    path,
                    include,
                },
            ) => {
                self.grep(&call.id, pattern, path.as_deref(), include.as_deref())
                    .await
            }
            (
                AgentToolName::EditFile,
                AgentToolArgs::EditFile {
                    path,
                    old_string,
                    new_string,
                    replace_all,
                },
            ) => {
                self.edit_file(&call.id, path, old_string, new_string, *replace_all)
                    .await
            }
            (
                AgentToolName::ReadAcceptanceReport,
                AgentToolArgs::ReadAcceptanceReport { run_id },
            ) => {
                self.read_acceptance_report(&call.id, run_id.as_deref())
                    .await
            }
            (
                AgentToolName::ReadRuntimeLog,
                AgentToolArgs::ReadRuntimeLog {
                    run_id,
                    file_name,
                    max_lines,
                },
            ) => {
                self.read_runtime_log(&call.id, run_id.as_deref(), file_name, *max_lines)
                    .await
            }
            (AgentToolName::ListAcceptanceRuns, AgentToolArgs::ListAcceptanceRuns { limit }) => {
                self.list_acceptance_runs(&call.id, *limit).await
            }
            (AgentToolName::RunCommand, AgentToolArgs::RunCommand { command }) => {
                self.run_command(&call.id, command).await
            }
            _ => Err(AppError::Tool(
                "tool name and arguments do not match".into(),
            )),
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

    async fn write_file(
        &self,
        call_id: &str,
        path: &str,
        content: &str,
    ) -> AppResult<AgentToolResult> {
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
            kind: if existed {
                ChangeKind::Modified
            } else {
                ChangeKind::Created
            },
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
            tool_call_id: call_id.to_string(),
            content,
            is_error: false,
            file_change: None,
        })
    }

    async fn grep(
        &self,
        call_id: &str,
        pattern: &str,
        path: Option<&str>,
        include: Option<&str>,
    ) -> AppResult<AgentToolResult> {
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => return Ok(tool_error(call_id, &format!("正则编译失败: {e}"))),
        };

        let root = self.fs.root().to_path_buf();
        let search_root = match path {
            Some(p) => match resolve_within_root(&root, p) {
                Ok(t) => t,
                Err(e) => return Ok(tool_error(call_id, &e)),
            },
            None => root.clone(),
        };

        let include_glob = include.and_then(|g| glob::Pattern::new(g).ok());

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
            tool_call_id: call_id.to_string(),
            content: output,
            is_error: false,
            file_change: None,
        })
    }

    async fn edit_file(
        &self,
        call_id: &str,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> AppResult<AgentToolResult> {
        if old_string == new_string {
            return Ok(tool_error(
                call_id,
                "old_string 和 new_string 相同,无需替换",
            ));
        }

        let full = self.fs.root().join(path);
        let content = match self.fs.read_text(&full) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error(call_id, &format!("读取失败: {e}"))),
        };

        let actual_old = match find_actual_string(&content, old_string) {
            Some(s) => s,
            None => {
                return Ok(tool_error(
                    call_id,
                    "未找到匹配的文本。请先 read_file 确认当前内容。",
                ))
            }
        };

        let match_count = content.matches(actual_old.as_str()).count();
        if match_count > 1 && !replace_all {
            return Ok(tool_error(
                call_id,
                &format!(
                "找到 {match_count} 处匹配,请提供更多上下文使 old_string 唯一,或设 replace_all=true"
            ),
            ));
        }

        let updated = if replace_all {
            content.replace(&actual_old, new_string)
        } else {
            content.replacen(&actual_old, new_string, 1)
        };

        let write = match self.fs.write_text_with_snapshot(&full, &updated) {
            Ok(w) => w,
            Err(e) => return Ok(tool_error(call_id, &format!("写入失败: {e}"))),
        };

        let change = FileChange {
            id: Uuid::new_v4(),
            task_run_id: Uuid::new_v4(),
            path: path.to_string(),
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
            tool_call_id: call_id.to_string(),
            content: summary,
            is_error: false,
            file_change: Some(change),
        })
    }

    async fn read_acceptance_report(
        &self,
        call_id: &str,
        run_id: Option<&str>,
    ) -> AppResult<AgentToolResult> {
        match read_acceptance_report(self.fs.root(), run_id) {
            Ok(content) => Ok(AgentToolResult {
                tool_call_id: call_id.to_string(),
                content: cap_text_bytes(content, ACCEPTANCE_REPORT_MAX_BYTES),
                is_error: false,
                file_change: None,
            }),
            Err(e) => Ok(tool_error(call_id, &format!("读取验收报告失败: {e}"))),
        }
    }

    async fn read_runtime_log(
        &self,
        call_id: &str,
        run_id: Option<&str>,
        file_name: &str,
        max_lines: usize,
    ) -> AppResult<AgentToolResult> {
        match read_runtime_log(self.fs.root(), run_id, file_name, max_lines) {
            Ok(content) => Ok(AgentToolResult {
                tool_call_id: call_id.to_string(),
                content: cap_text_bytes(content, ACCEPTANCE_LOG_MAX_BYTES),
                is_error: false,
                file_change: None,
            }),
            Err(e) => Ok(tool_error(call_id, &format!("读取运行日志失败: {e}"))),
        }
    }

    async fn list_acceptance_runs(
        &self,
        call_id: &str,
        limit: usize,
    ) -> AppResult<AgentToolResult> {
        let limit = limit.clamp(ACCEPTANCE_RUNS_LIMIT_MIN, ACCEPTANCE_RUNS_LIMIT_MAX);
        match list_acceptance_runs(self.fs.root(), limit) {
            Ok(runs) => Ok(AgentToolResult {
                tool_call_id: call_id.to_string(),
                content: if runs.is_empty() {
                    "（无验收运行记录）".to_string()
                } else {
                    runs.join("\n")
                },
                is_error: false,
                file_change: None,
            }),
            Err(e) => Ok(tool_error(call_id, &format!("列出验收运行失败: {e}"))),
        }
    }

    async fn run_command(&self, call_id: &str, command: &str) -> AppResult<AgentToolResult> {
        let action = classify_command_with_level(
            command,
            self.fs.root().to_str().unwrap_or(""),
            self.risk_level,
        );

        let command_to_run = match action {
            CommandAction::Allow => command.to_string(),
            CommandAction::Deny(reason) => {
                return Ok(tool_error(
                    call_id,
                    &format!("命令被拒绝({reason}): {command}"),
                ));
            }
            CommandAction::RequireConfirm => match &self.confirm_handler {
                None => {
                    return Ok(tool_error(
                        call_id,
                        &format!("命令需要确认但无确认处理器: {command}"),
                    ));
                }
                Some(handler) => {
                    let allowed = handler.confirm(command, "高风险命令").await;
                    if !allowed {
                        return Ok(tool_error(
                            call_id,
                            &format!("命令被用户拒绝: {command}"),
                        ));
                    }
                    command.to_string()
                }
            },
        };

        let argv = match shell_words(&command_to_run) {
            v if v.is_empty() => return Ok(tool_error(call_id, "空命令")),
            v => v,
        };

        let root = self.fs.root().to_path_buf();
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
                let content = format!(
                    "exit code: {exit_code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                );
                let is_error = exit_code != 0;
                Ok(AgentToolResult {
                    tool_call_id: call_id.to_string(),
                    content,
                    is_error,
                    file_change: None,
                })
            }
            Ok(Err(e)) => Ok(tool_error(call_id, &format!("执行失败: {e}"))),
            Err(_) => Ok(tool_error(call_id, "命令超时(30s),已被终止")),
        }
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

/// 查找文件中匹配 old_string 的实际文本。
/// 先试精确匹配,失败则归一化引号后重试。
/// 返回文件里实际的那段文本(保留原引号风格)。
fn find_actual_string(content: &str, old_string: &str) -> Option<String> {
    // 1. 精确匹配
    if content.contains(old_string) {
        return Some(old_string.to_string());
    }

    // 2. 归一化引号后匹配
    // 曲引号(3 字节)和直引号(1 字节)字节长度不同,用字符序列对齐。
    let content_chars: Vec<char> = content.chars().collect();
    let old_chars: Vec<char> = old_string.chars().collect();

    let norm_content: String = content_chars
        .iter()
        .map(|c| normalize_one_quote(*c))
        .collect();
    let norm_old: String = old_chars.iter().map(|c| normalize_one_quote(*c)).collect();

    let idx = norm_content.find(&norm_old)?;

    // norm_content.find 返回字节索引,转成字符索引。
    let char_start = norm_content[..idx].chars().count();
    let char_len = old_chars.len();

    // 从原 content 的字符序列里取对应片段(保留原引号)。
    let actual: String = content_chars[char_start..char_start + char_len]
        .iter()
        .collect();
    Some(actual)
}

fn normalize_quotes(s: &str) -> String {
    s.chars().map(normalize_one_quote).collect()
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

#[cfg(test)]
mod tests {
    use super::super::domain::{AgentToolArgs, AgentToolCall, AgentToolName};
    use super::super::test_support::TempWorkspace;
    use super::{truncate_output, ToolDispatcher};
    use std::fs;

    fn read_call(path: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-1".to_string(),
            name: AgentToolName::ReadFile,
            arguments: AgentToolArgs::Read {
                path: path.to_string(),
            },
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

    fn read_acceptance_report_call(run_id: Option<&str>) -> AgentToolCall {
        AgentToolCall {
            id: "call-acceptance-report".to_string(),
            name: AgentToolName::ReadAcceptanceReport,
            arguments: AgentToolArgs::ReadAcceptanceReport {
                run_id: run_id.map(String::from),
            },
        }
    }

    fn read_runtime_log_call(
        run_id: Option<&str>,
        file_name: &str,
        max_lines: usize,
    ) -> AgentToolCall {
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

    fn cmd_call(command: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-cmd".to_string(),
            name: AgentToolName::RunCommand,
            arguments: AgentToolArgs::RunCommand {
                command: command.to_string(),
            },
        }
    }

    #[tokio::test]
    async fn tool_read_file_returns_content() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hello.txt"), "hi there\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&read_call("hello.txt")).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "hi there\n");
        assert!(result.file_change.is_none());
    }

    #[tokio::test]
    async fn tool_read_file_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&read_call("../outside.txt")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn tool_read_nonexistent_returns_error_result_not_panic() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&read_call("nope.txt")).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn tool_write_file_creates_and_returns_file_change() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&write_call("out.txt", "new content\n"))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("out.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "new content\n");
        let change = result
            .file_change
            .expect("write should produce file_change");
        assert_eq!(change.path, "out.txt");
        assert!(change.diff.contains("+new content"));
    }

    #[tokio::test]
    async fn tool_write_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&write_call("../escape.txt", "x"))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn tool_reads_latest_acceptance_report() {
        let workspace = TempWorkspace::new("tool-acceptance-report");
        workspace.write_run_file("2026-06-14T09-00-00Z", "report.json", r#"{"ok":false}"#);
        workspace.write_run_file(
            "2026-06-15T09-00-00Z",
            "report.json",
            r#"{"ok":true,"failureSummary":null}"#,
        );

        let tools = ToolDispatcher::new(workspace.path().to_path_buf());
        let result = tools
            .dispatch(&read_acceptance_report_call(None))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains(r#""ok":true"#));
        assert!(result.content.contains("failureSummary"));
        assert!(result.file_change.is_none());
    }

    #[tokio::test]
    async fn tool_truncates_oversized_acceptance_report() {
        let workspace = TempWorkspace::new("tool-acceptance-report-large");
        let large_report = format!(
            r#"{{"ok":true,"failureSummary":null,"body":"{}"}}"#,
            "x".repeat(70 * 1024)
        );
        workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", &large_report);

        let tools = ToolDispatcher::new(workspace.path().to_path_buf());
        let result = tools
            .dispatch(&read_acceptance_report_call(None))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.len() < large_report.len());
        assert!(result.content.starts_with(r#"{"ok":true"#));
        assert!(result.content.contains("内容已截断，只显示前 65536 字节"));
    }

    #[tokio::test]
    async fn tool_reads_runtime_log_with_max_lines() {
        let workspace = TempWorkspace::new("tool-runtime-log");
        workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", r#"{"ok":true}"#);
        workspace.write_run_file(
            "2026-06-15T09-00-00Z",
            "runtime.log",
            "line1\nline2\nline3\nline4\n",
        );

        let tools = ToolDispatcher::new(workspace.path().to_path_buf());
        let result = tools
            .dispatch(&read_runtime_log_call(
                Some("2026-06-15T09-00-00Z"),
                "runtime.log",
                2,
            ))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "line3\nline4\n");
    }

    #[tokio::test]
    async fn tool_truncates_oversized_runtime_log_line() {
        let workspace = TempWorkspace::new("tool-runtime-log-large");
        workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", r#"{"ok":true}"#);
        let large_log = format!("{}\n", "x".repeat(40 * 1024));
        workspace.write_run_file("2026-06-15T09-00-00Z", "runtime.log", &large_log);

        let tools = ToolDispatcher::new(workspace.path().to_path_buf());
        let result = tools
            .dispatch(&read_runtime_log_call(
                Some("2026-06-15T09-00-00Z"),
                "runtime.log",
                1,
            ))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.len() < large_log.len());
        assert!(result.content.contains("内容已截断，只显示前 32768 字节"));
    }

    #[tokio::test]
    async fn tool_lists_acceptance_runs_empty_returns_placeholder() {
        let workspace = TempWorkspace::new("tool-acceptance-empty");
        fs::create_dir_all(workspace.path()).unwrap();

        let tools = ToolDispatcher::new(workspace.path().to_path_buf());
        let result = tools.dispatch(&list_acceptance_runs_call(5)).await.unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content, "（无验收运行记录）");
    }

    #[tokio::test]
    async fn list_files_empty_dir_returns_placeholder() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, false)).await.unwrap();

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

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, false)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("b.txt"));
        assert!(result.content.contains("dir"));
        assert!(result.content.contains("subdir"));
    }

    #[tokio::test]
    async fn list_files_recursive_lists_nested() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("nested/deep")).unwrap();
        std::fs::write(root.join("nested/deep/file.txt"), "x").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, true)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.content.contains("file.txt"));
        assert!(result.content.contains("nested/deep/file.txt"));
    }

    #[tokio::test]
    async fn list_files_ignores_node_modules() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(root.join("real.txt"), "y").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, true)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.content.contains("node_modules"));
        assert!(result.content.contains("real.txt"));
    }

    #[tokio::test]
    async fn list_files_truncates_at_200() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..250 {
            std::fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, false)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.content.contains("截断"));
        let lines: Vec<&str> = result.content.lines().filter(|l| l.contains(".txt")).collect();
        assert_eq!(lines.len(), 200);
    }

    #[tokio::test]
    async fn list_files_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&list_call(Some("../outside"), false))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn grep_finds_matches() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.ts"), "const x = invoke(\"foo\");\n").unwrap();
        std::fs::write(root.join("b.ts"), "no match here\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("invoke", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("a.ts:1:"));
        assert!(result.content.contains("invoke"));
        assert!(!result.content.contains("b.ts"));
    }

    #[tokio::test]
    async fn grep_no_match_returns_placeholder() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("nonexistent", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("无匹配"));
    }

    #[tokio::test]
    async fn grep_regex_word_boundary() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "invoke\nxinvokey\ninvoked\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call(r"\binvoke\b", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        let match_lines: Vec<&str> = result.content.lines().filter(|l| l.contains(":")).collect();
        assert_eq!(match_lines.len(), 1);
        assert!(match_lines[0].contains(":1:"));
    }

    #[tokio::test]
    async fn grep_ignores_node_modules() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules/lib.js"), "var invoke = 1;\n").unwrap();
        std::fs::write(root.join("real.ts"), "let invoke = 2;\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("invoke", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.content.contains("node_modules"));
        assert!(result.content.contains("real.ts"));
    }

    #[tokio::test]
    async fn grep_skips_large_files() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let big = "invoke ".repeat(200_000);
        std::fs::write(root.join("big.txt"), &big).unwrap();
        std::fs::write(root.join("small.txt"), "invoke here\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("invoke", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.content.contains("big.txt"));
        assert!(result.content.contains("small.txt"));
    }

    #[tokio::test]
    async fn grep_truncates_at_100() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let content = (0..150).map(|_| "invoke").collect::<Vec<_>>().join("\n");
        std::fs::write(root.join("many.txt"), &content).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("invoke", None, None))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.content.contains("截断"));
        let match_lines: Vec<&str> = result.content.lines().filter(|l| l.contains(":")).collect();
        assert_eq!(match_lines.len(), 100);
    }

    #[tokio::test]
    async fn grep_include_glob_filter() {
        let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.ts"), "invoke\n").unwrap();
        std::fs::write(root.join("b.js"), "invoke\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("invoke", None, Some("*.ts")))
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

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&grep_call("x", Some("../outside"), None))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_files_does_not_follow_symlink_dirs() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("sophoni-sym-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!("sophoni-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        symlink(&outside, root.join("link")).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, true)).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        assert!(!result.content.contains("secret.txt"));
    }

    #[tokio::test]
    async fn list_files_respects_depth_limit() {
        let root = std::env::temp_dir().join(format!("sophoni-deep-{}", uuid::Uuid::new_v4()));
        let mut deep = root.clone();
        for i in 0..12 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("leaf.txt"), "x").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&list_call(None, true)).await.unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(!result.content.contains("leaf.txt"));
    }

    #[tokio::test]
    async fn edit_file_basic_replace() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello world\nfoo bar\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("a.txt", "world", "Rust", false))
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
    async fn edit_file_multiline_replace() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "line1\nline2\nline3\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call(
                "a.txt",
                "line1\nline2",
                "replaced1\nreplaced2",
                false,
            ))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "replaced1\nreplaced2\nline3\n");
    }

    #[tokio::test]
    async fn edit_file_not_found_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("a.txt", "nonexistent", "x", false))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("未找到"));
    }

    #[tokio::test]
    async fn edit_file_not_unique_without_replace_all() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "foo\nfoo\nfoo\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("a.txt", "foo", "bar", false))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("3 处"));
    }

    #[tokio::test]
    async fn edit_file_replace_all() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "foo\nfoo\nfoo\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("a.txt", "foo", "bar", true))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "bar\nbar\nbar\n");
        assert!(result.content.contains("3 处"));
    }

    #[tokio::test]
    async fn edit_file_old_equals_new_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "hello\n").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("a.txt", "hello", "hello", false))
            .await
            .unwrap();

        std::fs::remove_dir_all(&root).unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("相同"));
    }

    #[tokio::test]
    async fn edit_file_nonexistent_file_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("nope.txt", "old", "new", false))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn edit_file_outside_root_is_error() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("../outside", "old", "new", false))
            .await
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn edit_file_quote_normalization_curly_to_straight() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "let x = \"hello\";\n").unwrap();

        let curly_old = "let x = \u{201C}hello\u{201D};";
        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call("a.txt", curly_old, "let x = \"world\";", false))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "let x = \"world\";\n");
    }

    #[tokio::test]
    async fn edit_file_preserves_curly_quotes_in_file() {
        let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "let x = \u{201C}hello\u{201D};\n").unwrap();

        let straight_old = "let x = \"hello\";";
        let tools = ToolDispatcher::new(root.clone());
        let result = tools
            .dispatch(&edit_call(
                "a.txt",
                straight_old,
                "let x = \u{201C}world\u{201D};",
                false,
            ))
            .await
            .unwrap();

        let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "let x = \u{201C}world\u{201D};\n");
    }

    #[tokio::test]
    async fn run_command_ls_succeeds() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("visible.txt"), "x").unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&cmd_call("ls")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(!result.is_error);
        assert!(result.content.contains("visible.txt"));
    }

    #[tokio::test]
    async fn run_command_ls_nonexistent_is_error_with_exit_code() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&cmd_call("ls /nonexistent_dir_xyz")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(result.content.contains("exit code"));
    }

    #[tokio::test]
    async fn run_command_high_risk_rejected() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&cmd_call("rm -rf /")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
        assert!(result.content.contains("高风险"));
    }

    #[tokio::test]
    async fn run_command_shell_injection_rejected() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&cmd_call("cargo test && rm -rf /")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn run_command_empty_rejected() {
        let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        let tools = ToolDispatcher::new(root.clone());
        let result = tools.dispatch(&cmd_call("   ")).await.unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(result.is_error);
    }

    #[test]
    fn truncate_short_input_returned_asis_without_hint() {
        let out = truncate_output("line1\nline2\n", 100, 4000);
        assert_eq!(out, "line1\nline2");
        assert!(!out.contains("截断"));
    }

    #[test]
    fn truncate_when_too_many_lines_adds_hint_with_counts() {
        let input = "l1\nl2\nl3\nl4\nl5\n";
        let out = truncate_output(input, 2, 4000);
        assert!(out.starts_with("l1\nl2\n"), "应只保留前 2 行");
        assert!(out.contains("截断"), "截断提示必须出现，让模型知道输出不全");
        assert!(out.contains("手动运行"), "提示应引导模型知道完整输出需另行获取");
        assert!(
            out.contains("前 2/5 行"),
            "提示应显示实际/总行数，got: {out}"
        );
    }

    #[test]
    fn truncate_when_too_many_chars_adds_hint() {
        let long_line = "a".repeat(100);
        let out = truncate_output(&long_line, 100, 10);
        assert!(out.contains("截断"));
        assert!(out.contains("前 1/1 行"));
        let body = out.lines().next().unwrap();
        assert_eq!(body.chars().filter(|c| *c == 'a').count(), 10);
    }

    #[test]
    fn truncate_empty_input_returns_empty() {
        let out = truncate_output("", 100, 4000);
        assert_eq!(out, "");
        assert!(!out.contains("截断"));
    }

    #[test]
    fn truncate_exactly_at_limit_not_truncated() {
        let input = "a\nb\n";
        let out = truncate_output(input, 2, 4000);
        assert_eq!(out, "a\nb");
        assert!(!out.contains("截断"));
    }
}
