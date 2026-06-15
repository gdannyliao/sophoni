use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;
use walkdir::WalkDir;

use super::acceptance::{list_acceptance_runs, read_acceptance_report, read_runtime_log};
use super::command_risk::{classify_command, shell_words, CommandRisk};
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

pub struct ToolDispatcher {
    fs: WorkspaceFs,
}

impl ToolDispatcher {
    pub fn new(root: PathBuf) -> Self {
        Self {
            fs: WorkspaceFs::new(root),
        }
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
        let risk = classify_command(command, "");
        if risk == CommandRisk::High {
            return Ok(tool_error(
                call_id,
                &format!(
                    "命令被拒绝(高风险): {command}\n只允许安全的只读命令(cargo test/check/build/clippy、git status/diff/log、ls、rg、tsc、pnpm test/build/check 等)。"
                ),
            ));
        }

        let argv = match shell_words(command) {
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
