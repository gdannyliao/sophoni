//! 工具调度相关的基础类型。
//!
//! 历史上的 `ToolDispatcher` 已被 `tool_spec::ToolRegistry` 取代：每个工具一个
//! `impl ToolSpec`，由 `build_tool_registry` 聚合。本模块只保留两个无依赖的基础类型：
//! - `WorkspaceMode`：Full / ChatOnly，决定哪些工具对模型可见、dispatch 时是否拦截。
//! - `ConfirmHandler`：run_command 高危命令的用户确认回调（Tauri 端实现弹窗）。

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspaceMode {
    #[default]
    Full,
    ChatOnly,
}

#[async_trait]
pub trait ConfirmHandler: Send + Sync {
    async fn confirm(&self, command: &str, reason: &str) -> bool;
}

/// 截断命令输出，避免长输出淹没模型上下文。
/// 超过行数或字符数上限时追加中文提示（含实际/总行数，引导模型另行获取完整输出）。
pub(crate) fn truncate_output(s: &str, max_lines: usize, max_chars: usize) -> String {
    let truncated: String = s.chars().take(max_chars).collect();
    let lines: Vec<&str> = truncated.lines().take(max_lines).collect();
    let result = lines.join("
");
    let total_lines = s.lines().count();
    let total_chars = s.chars().count();
    if total_lines > max_lines || total_chars > max_chars {
        format!(
            "{result}
（输出已截断，显示前 {}/{} 行。如需完整输出，请在终端手动运行。）",
            lines.len(),
            total_lines
        )
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_output;

    #[test]
    fn truncate_short_input_returned_asis_without_hint() {
        let out = truncate_output("line1
line2
", 100, 4000);
        assert_eq!(out, "line1
line2");
        assert!(!out.contains("截断"));
    }

    #[test]
    fn truncate_when_too_many_lines_adds_hint_with_counts() {
        let input = "l1
l2
l3
l4
l5
";
        let out = truncate_output(input, 2, 4000);
        assert!(out.starts_with("l1
l2
"), "应只保留前 2 行");
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
        let input = "a
b
";
        let out = truncate_output(input, 2, 4000);
        assert_eq!(out, "a
b");
        assert!(!out.contains("截断"));
    }
}
