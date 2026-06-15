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
            (AgentToolName::Grep, AgentToolArgs::Grep { pattern, path, include }) => {
                self.grep(&call.id, pattern, path.as_deref(), include.as_deref()).await
            }
            (AgentToolName::EditFile, AgentToolArgs::EditFile { path, old_string, new_string, replace_all }) => {
                self.edit_file(&call.id, path, old_string, new_string, *replace_all).await
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
            return Ok(tool_error(call_id, "old_string 和 new_string 相同,无需替换"));
        }

        let full = self.fs.root().join(path);
        let content = match self.fs.read_text(&full) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error(call_id, &format!("读取失败: {e}"))),
        };

        let actual_old = match find_actual_string(&content, old_string) {
            Some(s) => s,
            None => {
                return Ok(tool_error(call_id, "未找到匹配的文本。请先 read_file 确认当前内容。"))
            }
        };

        let match_count = content.matches(actual_old.as_str()).count();
        if match_count > 1 && !replace_all {
            return Ok(tool_error(call_id, &format!(
                "找到 {match_count} 处匹配,请提供更多上下文使 old_string 唯一,或设 replace_all=true"
            )));
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

    let norm_content: String = content_chars.iter().map(|c| normalize_one_quote(*c)).collect();
    let norm_old: String = old_chars.iter().map(|c| normalize_one_quote(*c)).collect();

    let idx = norm_content.find(&norm_old)?;

    // norm_content.find 返回字节索引,转成字符索引。
    let char_start = norm_content[..idx].chars().count();
    let char_len = old_chars.len();

    // 从原 content 的字符序列里取对应片段(保留原引号)。
    let actual: String = content_chars[char_start..char_start + char_len].iter().collect();
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
