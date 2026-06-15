use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::domain::{
    AgentConfig, AgentToolArgs, AgentToolCall, AgentToolName, AgentToolSchema, ConversationTurn,
    ProviderResponse, SystemPrompt,
};
use super::errors::{AppError, AppResult};

const READ_RUNTIME_LOG_DEFAULT_MAX_LINES: usize = 80;
const READ_RUNTIME_LOG_MAX_LINES: u64 = 200;
const LIST_ACCEPTANCE_RUNS_DEFAULT_LIMIT: usize = 5;
const LIST_ACCEPTANCE_RUNS_MAX_LIMIT: u64 = 20;

/// 从工具参数 JSON 取必填字符串字段，缺失则返回错误（错误消息含工具名）。
fn req_str(args: &serde_json::Value, key: &str, tool: &str) -> AppResult<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| AppError::Provider(format!("{tool} missing {key}")))
}

/// 从工具参数 JSON 取可选字符串字段，缺失返回 None。
fn opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// 从工具参数 JSON 取可选布尔字段，缺失默认 false。
fn opt_bool(args: &serde_json::Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Model-agnostic provider contract. Implementations (OpenAICompatibleProvider, future
/// OpenAI/Claude providers) translate domain types to/from their wire format.
#[async_trait]
pub trait AgentProvider: Send {
    async fn complete(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[AgentToolSchema],
    ) -> AppResult<ProviderResponse>;
}

/// Test-only provider that plays back a scripted sequence of responses.
/// Used by agent loop tests (L2) to exercise the loop deterministically.
#[cfg(test)]
pub struct FakeProvider {
    script: Vec<ProviderResponse>,
    call_count: usize,
    error: Option<String>,
}

#[cfg(test)]
impl FakeProvider {
    pub fn new(script: Vec<ProviderResponse>) -> Self {
        Self {
            script,
            call_count: 0,
            error: None,
        }
    }

    pub fn always(response: ProviderResponse) -> Self {
        Self::new(vec![response; 100])
    }

    pub fn always_error(message: &str) -> Self {
        Self {
            script: vec![],
            call_count: 0,
            error: Some(message.to_string()),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl AgentProvider for FakeProvider {
    async fn complete(
        &mut self,
        _system: &SystemPrompt,
        _turns: &[ConversationTurn],
        _tools: &[AgentToolSchema],
    ) -> AppResult<ProviderResponse> {
        if let Some(msg) = &self.error {
            return Err(AppError::Provider(msg.clone()));
        }
        let response = self
            .script
            .get(self.call_count)
            .cloned()
            .unwrap_or_else(|| {
                ProviderResponse::FinalAnswer("(script exhausted, forcing end)".into())
            });
        self.call_count += 1;
        Ok(response)
    }
}

/// Test helper: construct a read_file tool call.
#[cfg(test)]
pub fn fake_read_call(id: &str, path: &str) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        name: AgentToolName::ReadFile,
        arguments: AgentToolArgs::Read {
            path: path.to_string(),
        },
    }
}

/// Test helper: construct a write_file tool call.
#[cfg(test)]
pub fn fake_write_call(id: &str, path: &str, content: &str) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        name: AgentToolName::WriteFile,
        arguments: AgentToolArgs::Write {
            path: path.to_string(),
            content: content.to_string(),
        },
    }
}

// ── OpenAICompatibleProvider: real GLM API client ──

pub struct OpenAICompatibleProvider {
    config: AgentConfig,
    http: reqwest::Client,
}

impl OpenAICompatibleProvider {
    pub fn new(config: AgentConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self { config, http }
    }

    /// Translate a model-agnostic turn into the GLM wire format.
    pub(crate) fn turn_to_openai_message(turn: &ConversationTurn) -> OpenAIMessage {
        match turn {
            ConversationTurn::User { content } => OpenAIMessage {
                role: "user".to_string(),
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: None,
            },
            ConversationTurn::Assistant {
                content,
                tool_calls,
            } => OpenAIMessage {
                role: "assistant".to_string(),
                content: content.clone(),
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.iter().map(Self::tool_call_to_openai).collect())
                },
                tool_call_id: None,
            },
            ConversationTurn::Tool {
                tool_call_id,
                result,
            } => OpenAIMessage {
                role: "tool".to_string(),
                content: Some(result.content.clone()),
                tool_calls: None,
                tool_call_id: Some(tool_call_id.clone()),
            },
        }
    }

    fn tool_call_to_openai(call: &AgentToolCall) -> OpenAIToolCall {
        let (name, arguments) = match &call.arguments {
            AgentToolArgs::Read { path } => ("read_file", serde_json::json!({ "path": path })),
            AgentToolArgs::Write { path, content } => (
                "write_file",
                serde_json::json!({ "path": path, "content": content }),
            ),
            AgentToolArgs::ListFiles { path, recursive } => (
                "list_files",
                serde_json::json!({ "path": path, "recursive": recursive }),
            ),
            AgentToolArgs::Grep {
                pattern,
                path,
                include,
            } => (
                "grep",
                serde_json::json!({ "pattern": pattern, "path": path, "include": include }),
            ),
            AgentToolArgs::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
            } => (
                "edit_file",
                serde_json::json!({
                    "path": path,
                    "old_string": old_string,
                    "new_string": new_string,
                    "replace_all": replace_all
                }),
            ),
            AgentToolArgs::ReadAcceptanceReport { run_id } => (
                "read_acceptance_report",
                serde_json::json!({ "run_id": run_id }),
            ),
            AgentToolArgs::ReadRuntimeLog {
                run_id,
                file_name,
                max_lines,
            } => (
                "read_runtime_log",
                serde_json::json!({
                    "run_id": run_id,
                    "file_name": file_name,
                    "max_lines": max_lines
                }),
            ),
            AgentToolArgs::ListAcceptanceRuns { limit } => (
                "list_acceptance_runs",
                serde_json::json!({ "limit": limit }),
            ),
            AgentToolArgs::RunCommand { command } => (
                "run_command",
                serde_json::json!({ "command": command }),
            ),
        };
        OpenAIToolCall {
            id: call.id.clone(),
            kind: "function".to_string(),
            function: OpenAIFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn tool_schema_to_openai(schema: &AgentToolSchema) -> OpenAIToolDef {
        OpenAIToolDef {
            kind: "function".to_string(),
            function: OpenAIToolFunctionDef {
                name: schema.name.to_string(),
                description: schema.description.to_string(),
                parameters: schema.parameters.clone(),
            },
        }
    }

    /// Translate the GLM response DTO into a model-agnostic ProviderResponse.
    pub(crate) fn translate_response(resp: OpenAIResponse) -> AppResult<ProviderResponse> {
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Provider("response has no choices".into()))?;

        let tool_calls = choice.message.tool_calls.unwrap_or_default();
        if tool_calls.is_empty() {
            let text = choice.message.content.unwrap_or_default();
            Ok(ProviderResponse::FinalAnswer(text))
        } else {
            let calls = tool_calls
                .into_iter()
                .map(Self::parse_tool_call)
                .collect::<AppResult<Vec<_>>>()?;
            Ok(ProviderResponse::ToolCalls(calls))
        }
    }

    fn parse_tool_call(gtc: OpenAIToolCall) -> AppResult<AgentToolCall> {
        let name = match gtc.function.name.as_str() {
            "read_file" => AgentToolName::ReadFile,
            "write_file" => AgentToolName::WriteFile,
            "list_files" => AgentToolName::ListFiles,
            "grep" => AgentToolName::Grep,
            "edit_file" => AgentToolName::EditFile,
            "read_acceptance_report" => AgentToolName::ReadAcceptanceReport,
            "read_runtime_log" => AgentToolName::ReadRuntimeLog,
            "list_acceptance_runs" => AgentToolName::ListAcceptanceRuns,
            "run_command" => AgentToolName::RunCommand,
            other => return Err(AppError::Provider(format!("unknown tool: {other}"))),
        };
        let args: serde_json::Value = serde_json::from_str(&gtc.function.arguments)
            .map_err(|e| AppError::Provider(format!("invalid tool arguments: {e}")))?;
        let tool = gtc.function.name.as_str();
        let arguments = match name {
            AgentToolName::ReadFile => {
                let path = req_str(&args, "path", tool)?;
                AgentToolArgs::Read { path }
            }
            AgentToolName::WriteFile => {
                let path = req_str(&args, "path", tool)?;
                let content = req_str(&args, "content", tool)?;
                AgentToolArgs::Write { path, content }
            }
            AgentToolName::ListFiles => {
                let path = opt_str(&args, "path");
                let recursive = opt_bool(&args, "recursive");
                AgentToolArgs::ListFiles { path, recursive }
            }
            AgentToolName::Grep => {
                let pattern = req_str(&args, "pattern", tool)?;
                let path = opt_str(&args, "path");
                let include = opt_str(&args, "include");
                AgentToolArgs::Grep {
                    pattern,
                    path,
                    include,
                }
            }
            AgentToolName::EditFile => {
                let path = req_str(&args, "path", tool)?;
                let old_string = req_str(&args, "old_string", tool)?;
                let new_string = req_str(&args, "new_string", tool)?;
                let replace_all = opt_bool(&args, "replace_all");
                AgentToolArgs::EditFile {
                    path,
                    old_string,
                    new_string,
                    replace_all,
                }
            }
            AgentToolName::ReadAcceptanceReport => {
                let run_id = opt_str(&args, "run_id");
                AgentToolArgs::ReadAcceptanceReport { run_id }
            }
            AgentToolName::ReadRuntimeLog => {
                let run_id = opt_str(&args, "run_id");
                let file_name = req_str(&args, "file_name", tool)?;
                let max_lines = args
                    .get("max_lines")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, READ_RUNTIME_LOG_MAX_LINES) as usize)
                    .unwrap_or(READ_RUNTIME_LOG_DEFAULT_MAX_LINES);
                AgentToolArgs::ReadRuntimeLog {
                    run_id,
                    file_name,
                    max_lines,
                }
            }
            AgentToolName::ListAcceptanceRuns => {
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, LIST_ACCEPTANCE_RUNS_MAX_LIMIT) as usize)
                    .unwrap_or(LIST_ACCEPTANCE_RUNS_DEFAULT_LIMIT);
                AgentToolArgs::ListAcceptanceRuns { limit }
            }
            AgentToolName::RunCommand => {
                let command = req_str(&args, "command", tool)?;
                AgentToolArgs::RunCommand { command }
            }
        };
        Ok(AgentToolCall {
            id: gtc.id,
            name,
            arguments,
        })
    }
}

#[async_trait]
impl AgentProvider for OpenAICompatibleProvider {
    async fn complete(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[AgentToolSchema],
    ) -> AppResult<ProviderResponse> {
        let mut messages = Vec::with_capacity(turns.len() + 1);
        messages.push(OpenAIMessage {
            role: "system".to_string(),
            content: Some(system.0.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
        for turn in turns {
            messages.push(Self::turn_to_openai_message(turn));
        }

        let openai_tools: Vec<OpenAIToolDef> = tools.iter().map(Self::tool_schema_to_openai).collect();
        let req = OpenAIRequest {
            model: self.config.model.clone(),
            messages,
            tools: Some(openai_tools),
            tool_choice: Some("auto".to_string()),
        };

        let url = format!("{}/chat/completions", self.config.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.config.api_key)
            .json(&req)
            .send()
            .await
            .map_err(|e| AppError::Provider(format!("http error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Provider(format!("HTTP {status}: {body}")));
        }

        let openai_resp: OpenAIResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Provider(format!("failed to parse response: {e}")))?;

        Self::translate_response(openai_resp)
    }
}

// ── GLM wire-format DTOs (private to this module) ──

#[derive(Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAIToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct OpenAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAIFunction,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct OpenAIFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize)]
struct OpenAIToolDef {
    #[serde(rename = "type")]
    kind: String,
    function: OpenAIToolFunctionDef,
}

#[derive(Serialize)]
struct OpenAIToolFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub(crate) struct OpenAIResponse {
    pub choices: Vec<OpenAIChoice>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct OpenAIChoice {
    pub message: OpenAIMessage,
}

#[cfg(test)]
mod tests {
    use super::super::domain::{AgentToolArgs, AgentToolResult, ConversationTurn, ProviderResponse};
    use super::{
        OpenAIChoice, OpenAIFunction, OpenAIMessage, OpenAICompatibleProvider, OpenAIResponse,
        OpenAIToolCall,
    };

    #[test]
    fn glm_translates_user_turn_to_message() {
        let turn = ConversationTurn::User { content: "hi".into() };
        let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn);
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.as_deref(), Some("hi"));
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn glm_translates_tool_turn_to_message() {
        let turn = ConversationTurn::Tool {
            tool_call_id: "tc-9".into(),
            result: AgentToolResult {
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
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call-1");
                match &calls[0].arguments {
                    AgentToolArgs::Read { path } => assert_eq!(path, "README.md"),
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
            ProviderResponse::FinalAnswer(t) => assert_eq!(t, "all done"),
            _ => panic!("expected FinalAnswer"),
        }
    }

    #[test]
    fn glm_parses_list_files_tool_call() {
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c1".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "list_files".into(),
                            arguments: r#"{"path":"src","recursive":true}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::ListFiles { path, recursive } => {
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
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c2".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "grep".into(),
                            arguments: r#"{"pattern":"invoke","include":"*.ts"}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::Grep {
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
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c-runtime".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "read_runtime_log".into(),
                            arguments: r#"{"run_id":"2026-06-15T09-00-00Z","file_name":"runtime.log","max_lines":3}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::ReadRuntimeLog {
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
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c-runtime-huge".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "read_runtime_log".into(),
                            arguments: r#"{"file_name":"runtime.log","max_lines":18446744073709551615}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
                AgentToolArgs::ReadRuntimeLog { max_lines, .. } => {
                    assert_eq!(*max_lines, 200);
                }
                _ => panic!("expected ReadRuntimeLog args"),
            },
            _ => panic!("expected ToolCalls"),
        }
    }

    #[test]
    fn glm_parses_list_acceptance_runs_default_limit() {
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c-list-acceptance".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "list_acceptance_runs".into(),
                            arguments: r#"{}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::ListAcceptanceRuns { limit } => {
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
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c-list-acceptance-huge".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "list_acceptance_runs".into(),
                            arguments: r#"{"limit":18446744073709551615}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
                AgentToolArgs::ListAcceptanceRuns { limit } => {
                    assert_eq!(*limit, 20);
                }
                _ => panic!("expected ListAcceptanceRuns args"),
            },
            _ => panic!("expected ToolCalls"),
        }
    }

    #[test]
    fn glm_parses_read_acceptance_report_optional_run_id() {
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c-report".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "read_acceptance_report".into(),
                            arguments: r#"{}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::ReadAcceptanceReport { run_id } => {
                        assert!(run_id.is_none());
                    }
                    _ => panic!("expected ReadAcceptanceReport args"),
                }
            }
            _ => panic!("expected ToolCalls"),
        }
    }

    #[test]
    fn glm_parses_edit_file_tool_call() {
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c1".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "edit_file".into(),
                            arguments: r#"{"path":"a.txt","old_string":"hello","new_string":"world"}"#
                                .into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::EditFile {
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
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c2".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "edit_file".into(),
                            arguments:
                                r#"{"path":"a.txt","old_string":"foo","new_string":"bar","replace_all":true}"#
                                    .into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
                AgentToolArgs::EditFile { replace_all, .. } => {
                    assert!(*replace_all);
                }
                _ => panic!("expected EditFile args"),
            },
            _ => panic!("expected ToolCalls"),
        }
    }

    #[test]
    fn glm_parses_edit_file_missing_field_is_error() {
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c3".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "edit_file".into(),
                            arguments: r#"{"path":"a.txt"}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let result = OpenAICompatibleProvider::translate_response(resp);
        assert!(result.is_err());
    }

    #[test]
    fn glm_parses_run_command_tool_call() {
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c1".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "run_command".into(),
                            arguments: r#"{"command":"cargo test"}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
        match translated {
            ProviderResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                match &calls[0].arguments {
                    AgentToolArgs::RunCommand { command } => {
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
        let resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: "c2".into(),
                        kind: "function".into(),
                        function: OpenAIFunction {
                            name: "run_command".into(),
                            arguments: r#"{}"#.into(),
                        },
                    }]),
                    tool_call_id: None,
                },
            }],
        };
        let result = OpenAICompatibleProvider::translate_response(resp);
        assert!(result.is_err());
    }
}
