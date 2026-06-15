use super::test_support::TempWorkspace;
use std::fs;

// ── tool layer tests (L1) ──

use super::domain::{AgentToolArgs, AgentToolCall, AgentToolName};
use super::tools::ToolDispatcher;

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
    let large_report = format!(r#"{{"ok":true,"failureSummary":null,"body":"{}"}}"#, "x".repeat(70 * 1024));
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


// ── OpenAI translation tests ──

use super::provider::{
    OpenAIChoice, OpenAIFunction, OpenAIMessage, OpenAICompatibleProvider, OpenAIResponse, OpenAIToolCall,
};

#[test]
fn glm_translates_user_turn_to_message() {
    let turn = super::domain::ConversationTurn::User { content: "hi".into() };
    let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn);
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.as_deref(), Some("hi"));
    assert!(msg.tool_calls.is_none());
    assert!(msg.tool_call_id.is_none());
}

#[test]
fn glm_translates_tool_turn_to_message() {
    let turn = super::domain::ConversationTurn::Tool {
        tool_call_id: "tc-9".into(),
        result: super::domain::AgentToolResult {
            tool_call_id: "tc-9".into(),
            content: "file body".into(),
            is_error: false,
            file_change: None,
        },
    };
    let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn);
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id.as_deref(), Some("tc-9"));
    assert_eq!(msg.content.as_deref(), Some("file body"));
}

#[test]
fn glm_translates_response_with_tool_calls() {
    let resp = OpenAIResponse {
        choices: vec![OpenAIChoice {
            message: OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![OpenAIToolCall {
                    id: "call-1".into(),
                    kind: "function".into(),
                    function: OpenAIFunction {
                        name: "read_file".into(),
                        arguments: "{\"path\":\"README.md\"}".into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call-1");
            match &calls[0].arguments {
                super::domain::AgentToolArgs::Read { path } => assert_eq!(path, "README.md"),
                _ => panic!("expected Read args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_translates_response_without_tool_calls_as_final_answer() {
    let resp = OpenAIResponse {
        choices: vec![OpenAIChoice {
            message: OpenAIMessage {
                role: "assistant".into(),
                content: Some("all done".into()),
                tool_calls: None,
                tool_call_id: None,
            },
        }],
    };
    let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::FinalAnswer(t) => assert_eq!(t, "all done"),
        _ => panic!("expected FinalAnswer"),
    }
}

// ── list_files 工具测试 ──

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

#[tokio::test]
async fn list_files_empty_dir_returns_placeholder() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, false)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.content.contains("截断"));
    let lines: Vec<&str> = result
        .content
        .lines()
        .filter(|l| l.contains(".txt"))
        .collect();
    assert_eq!(lines.len(), 200);
}

#[tokio::test]
async fn list_files_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&list_call(Some("../outside"), false))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

// ── grep 工具测试 ──

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

#[tokio::test]
async fn grep_finds_matches() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.ts"), "const x = invoke(\"foo\");\n").unwrap();
    std::fs::write(root.join("b.ts"), "no match here\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("x", Some("../outside"), None))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

// ── 搜索边界测试 ──

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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, true)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.content.contains("leaf.txt"));
}

// ── 搜索工具翻译测试 ──

#[test]
fn glm_parses_list_files_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "list_files".into(),
                        arguments: r#"{"path":"src","recursive":true}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ListFiles { path, recursive } => {
                    assert_eq!(path.as_deref(), Some("src"));
                    assert!(*recursive);
                }
                _ => panic!("expected ListFiles args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_grep_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c2".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "grep".into(),
                        arguments: r#"{"pattern":"invoke","include":"*.ts"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::Grep {
                    pattern,
                    path,
                    include,
                } => {
                    assert_eq!(pattern, "invoke");
                    assert!(path.is_none());
                    assert_eq!(include.as_deref(), Some("*.ts"));
                }
                _ => panic!("expected Grep args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_read_runtime_log_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-runtime".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "read_runtime_log".into(),
                        arguments: r#"{"run_id":"2026-06-15T09-00-00Z","file_name":"runtime.log","max_lines":3}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ReadRuntimeLog {
                    run_id,
                    file_name,
                    max_lines,
                } => {
                    assert_eq!(run_id.as_deref(), Some("2026-06-15T09-00-00Z"));
                    assert_eq!(file_name, "runtime.log");
                    assert_eq!(*max_lines, 3);
                }
                _ => panic!("expected ReadRuntimeLog args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_read_runtime_log_huge_max_lines_with_safe_clamp() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-runtime-huge".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "read_runtime_log".into(),
                        arguments: r#"{"file_name":"runtime.log","max_lines":18446744073709551615}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
            super::domain::AgentToolArgs::ReadRuntimeLog { max_lines, .. } => {
                assert_eq!(*max_lines, 200);
            }
            _ => panic!("expected ReadRuntimeLog args"),
        },
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_list_acceptance_runs_default_limit() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-list-acceptance".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "list_acceptance_runs".into(),
                        arguments: r#"{}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ListAcceptanceRuns { limit } => {
                    assert_eq!(*limit, 5);
                }
                _ => panic!("expected ListAcceptanceRuns args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_list_acceptance_runs_huge_limit_with_safe_clamp() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-list-acceptance-huge".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "list_acceptance_runs".into(),
                        arguments: r#"{"limit":18446744073709551615}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
            super::domain::AgentToolArgs::ListAcceptanceRuns { limit } => {
                assert_eq!(*limit, 20);
            }
            _ => panic!("expected ListAcceptanceRuns args"),
        },
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_read_acceptance_report_optional_run_id() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-report".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "read_acceptance_report".into(),
                        arguments: r#"{}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ReadAcceptanceReport { run_id } => {
                    assert!(run_id.is_none());
                }
                _ => panic!("expected ReadAcceptanceReport args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

// ── edit_file 工具测试 ──

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

#[tokio::test]
async fn edit_file_basic_replace() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "hello world\nfoo bar\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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

    let tools = super::tools::ToolDispatcher::new(root.clone());
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
    let tools = super::tools::ToolDispatcher::new(root.clone());
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
    let tools = super::tools::ToolDispatcher::new(root.clone());
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

// ── edit_file 翻译测试 ──

#[test]
fn glm_parses_edit_file_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "edit_file".into(),
                        arguments: r#"{"path":"a.txt","old_string":"hello","new_string":"world"}"#
                            .into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::EditFile {
                    path,
                    old_string,
                    new_string,
                    replace_all,
                } => {
                    assert_eq!(path, "a.txt");
                    assert_eq!(old_string, "hello");
                    assert_eq!(new_string, "world");
                    assert!(!replace_all);
                }
                _ => panic!("expected EditFile args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_edit_file_with_replace_all() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c2".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "edit_file".into(),
                        arguments: r#"{"path":"a.txt","old_string":"foo","new_string":"bar","replace_all":true}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
            super::domain::AgentToolArgs::EditFile { replace_all, .. } => {
                assert!(*replace_all);
            }
            _ => panic!("expected EditFile args"),
        },
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_edit_file_missing_field_is_error() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c3".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "edit_file".into(),
                        arguments: r#"{"path":"a.txt"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let result = super::provider::OpenAICompatibleProvider::translate_response(resp);
    assert!(result.is_err());
}

// ── run_command 工具测试 ──

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
async fn run_command_ls_succeeds() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("visible.txt"), "x").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("ls")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(!result.is_error);
    assert!(result.content.contains("visible.txt"));
}

#[tokio::test]
async fn run_command_ls_nonexistent_is_error_with_exit_code() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("ls /nonexistent_dir_xyz")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
    assert!(result.content.contains("exit code"));
}

#[tokio::test]
async fn run_command_high_risk_rejected() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("rm -rf /")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
    assert!(result.content.contains("高风险"));
}

#[tokio::test]
async fn run_command_shell_injection_rejected() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("cargo test && rm -rf /")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

#[tokio::test]
async fn run_command_empty_rejected() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("   ")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

// ── run_command 翻译测试 ──

#[test]
fn glm_parses_run_command_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "run_command".into(),
                        arguments: r#"{"command":"cargo test"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::RunCommand { command } => {
                    assert_eq!(command, "cargo test");
                }
                _ => panic!("expected RunCommand args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_run_command_missing_field_is_error() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c2".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "run_command".into(),
                        arguments: r#"{}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let result = super::provider::OpenAICompatibleProvider::translate_response(resp);
    assert!(result.is_err());
}

// ── run_command 端到端（真实 Provider，需联网 + API Key）──
// 用 `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored run_command_live`
// 运行。默认 ignore，避免污染常规测试与 CI。

#[tokio::test]
#[ignore]
async fn run_command_live_invokes_tool_against_real_provider() {
    use super::agent::{run_agent_task, EventSink};
    use super::domain::{AgentConfig, AgentEvent, SystemPrompt};

    let (config, provider_name) = AgentConfig::load().expect("AgentConfig 未配置");
    eprintln!("使用真实 Provider: {provider_name} / {}", config.model);

    // 临时工作区，初始化为 git 仓库，放一个可见文件。
    let root = std::env::temp_dir().join(format!("sophoni-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# live\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(&root)
        .output();

    let provider: Box<dyn super::provider::AgentProvider> =
        Box::new(super::provider::OpenAICompatibleProvider::new(config));
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let cancel = std::sync::atomic::AtomicBool::new(false);

    struct Collector(std::sync::Mutex<Vec<AgentEvent>>);
    impl EventSink for Collector {
        fn emit(&self, event: &AgentEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }
    let sink = Collector(std::sync::Mutex::new(Vec::new()));

    let task = "在工作区跑一次 git status，把命令输出原样告诉我。".to_string();
    let result = run_agent_task(
        provider,
        &tools,
        &sink,
        &cancel,
        SystemPrompt(String::new()),
        task,
        vec![],
    )
    .await
    .expect("run_agent_task 出错");

    eprintln!("Agent summary: {}", result.summary);

    let events = sink.0.lock().unwrap();
    eprintln!("收到 {} 个事件", events.len());
    for ev in events.iter() {
        eprintln!("  [{}] {}", ev.kind, ev.title);
    }

    // 核心断言：至少出现一次 run_command 工具调用，且最终至少有一个非 error 的结果。
    let invoked_run_command = events
        .iter()
        .any(|e| e.kind == "tool_call" && e.title.starts_with("run_command:"));
    let has_non_error_result = events.iter().any(|e| {
        e.kind == "tool_result" && !e.body.starts_with("失败")
    });

    let _ = std::fs::remove_dir_all(&root);

    assert!(invoked_run_command, "Agent 没有调用 run_command 工具");
    assert!(
        has_non_error_result,
        "run_command 没有产生成功结果（可能命令被拒或执行失败）"
    );
}

// ── 场景 2：失败自纠闭环（真实 Provider）──
// 给 Agent 一个带编译错误的 Cargo crate，要求它修好并用 cargo check 验证。
// 验证 Agent 能跨多轮：run_command(失败) → 看stderr → edit_file → run_command(成功)。
// 用 `cargo test -- --ignored run_command_self_heal` 运行。

#[tokio::test]
#[ignore]
async fn run_command_self_heal_fixes_compile_error_against_real_provider() {
    use super::agent::{run_agent_task, EventSink};
    use super::domain::{AgentConfig, AgentEvent, SystemPrompt};

    let (config, provider_name) = AgentConfig::load().expect("AgentConfig 未配置");
    eprintln!("使用真实 Provider: {provider_name} / {}", config.model);

    // 临时 Cargo crate，src/lib.rs 里故意少一个分号。
    let root = std::env::temp_dir().join(format!("sophoni-heal-{}", uuid::Uuid::new_v4()));
    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"heal_demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"src/lib.rs\"\n",
    )
    .unwrap();
    // 缺少分号 → cargo check 必然报 "expected `;`"。
    // （注意：函数末尾表达式不带分号是合法返回语法，必须用语句间缺分号才能制造真实错误。）
    std::fs::write(
        src_dir.join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    let x = a\n    let y = b\n    x + y\n}\n",
    )
    .unwrap();

    let provider: Box<dyn super::provider::AgentProvider> =
        Box::new(super::provider::OpenAICompatibleProvider::new(config));
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let cancel = std::sync::atomic::AtomicBool::new(false);

    struct Collector(std::sync::Mutex<Vec<AgentEvent>>);
    impl EventSink for Collector {
        fn emit(&self, event: &AgentEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }
    let sink = Collector(std::sync::Mutex::new(Vec::new()));

    let task = "这个 Rust 工程的 src/lib.rs 有编译错误。请用 cargo check 验证，\
                根据报错修正代码，再跑一次 cargo check 确认修复成功。"
        .to_string();
    let result = run_agent_task(
        provider,
        &tools,
        &sink,
        &cancel,
        SystemPrompt(String::new()),
        task,
        vec![],
    )
    .await
    .expect("run_agent_task 出错");
    eprintln!("Agent summary: {}", result.summary);

    let events = sink.0.lock().unwrap();
    eprintln!("收到 {} 个事件", events.len());

    let mut check_calls = 0usize;
    let mut failed_checks = 0usize;
    let mut successful_checks = 0usize;
    let mut edit_calls = 0usize;
    let mut saw_edit = false;

    for ev in events.iter() {
        eprintln!("  [{}] {}", ev.kind, ev.title);
        match ev.kind.as_str() {
            "tool_call" => {
                if ev.title.starts_with("run_command:") && ev.title.contains("cargo check") {
                    check_calls += 1;
                } else if ev.title.starts_with("edit_file:") {
                    edit_calls += 1;
                    saw_edit = true;
                }
            }
            "tool_result" => {
                // 仅统计 cargo check 相关结果：成功体以 "exit code:" 开头，
                // 失败体以 "失败:" 开头且含编译错误。
                let is_check_result = ev.body.contains("cargo check")
                    || ev.body.contains("error[")
                    || ev.body.contains("error:")
                    || ev.body.starts_with("exit code:");
                if !is_check_result {
                    continue;
                }
                if ev.body.starts_with("失败:") {
                    failed_checks += 1;
                } else if ev.body.contains("exit code: 0") {
                    successful_checks += 1;
                }
            }
            _ => {}
        }
    }

    let succeeded_after_edit = successful_checks >= 1 && saw_edit
        && successful_checks + failed_checks >= 2;
    eprintln!(
        "诊断: cargo check 调用 {check_calls} 次（成功 {successful_checks}，失败 {failed_checks}），edit_file {edit_calls} 次"
    );

    let _ = std::fs::remove_dir_all(&root);

    // 硬断言：Agent 用 run_command 跑了 check、改了代码、且最终 check 成功。
    assert!(
        check_calls >= 1,
        "Agent 没有用 run_command 跑 cargo check"
    );
    assert!(
        edit_calls >= 1,
        "Agent 没有调用 edit_file 修复代码"
    );
    assert!(
        successful_checks >= 1,
        "cargo check 从未成功，Agent 没能完成修复（可能是模型能力不足）"
    );
    assert!(
        succeeded_after_edit,
        "未形成'check失败→edit→check成功'闭环：check总次数 {}，成功 {}，edit {}",
        successful_checks + failed_checks,
        successful_checks,
        edit_calls
    );
}

// ── truncate_output 测试（规格成功标准 #4：输出截断让模型知道信息不全）──

#[test]
fn truncate_short_input_returned_asis_without_hint() {
    let out = super::tools::truncate_output("line1\nline2\n", 100, 4000);
    assert_eq!(out, "line1\nline2");
    assert!(!out.contains("截断"));
}

#[test]
fn truncate_when_too_many_lines_adds_hint_with_counts() {
    // 5 行输入，max_lines=2 → 应截断，提示显示"前 2/5 行"。
    let input = "l1\nl2\nl3\nl4\nl5\n";
    let out = super::tools::truncate_output(input, 2, 4000);
    assert!(out.starts_with("l1\nl2\n"), "应只保留前 2 行");
    assert!(
        out.contains("截断"),
        "截断提示必须出现，让模型知道输出不全"
    );
    assert!(
        out.contains("手动运行"),
        "提示应引导模型知道完整输出需另行获取"
    );
    assert!(
        out.contains("前 2/5 行"),
        "提示应显示实际/总行数，got: {out}"
    );
}

#[test]
fn truncate_when_too_many_chars_adds_hint() {
    // 单行但字符数超 max_chars。
    let long_line = "a".repeat(100);
    let out = super::tools::truncate_output(&long_line, 100, 10);
    assert!(out.contains("截断"));
    // 字符上限 10 → 只保留前 10 个字符，提示里的"总行数"是 1（单行）。
    assert!(out.contains("前 1/1 行"));
    // 主体不应超过 10 个 'a'。
    let body = out.lines().next().unwrap();
    assert_eq!(body.chars().filter(|c| *c == 'a').count(), 10);
}

#[test]
fn truncate_empty_input_returns_empty() {
    let out = super::tools::truncate_output("", 100, 4000);
    assert_eq!(out, "");
    assert!(!out.contains("截断"));
}

#[test]
fn truncate_exactly_at_limit_not_truncated() {
    // 恰好等于上限，不应截断。
    let input = "a\nb\n";
    let out = super::tools::truncate_output(input, 2, 4000);
    assert_eq!(out, "a\nb");
    assert!(!out.contains("截断"));
}
