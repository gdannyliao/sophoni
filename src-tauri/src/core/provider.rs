use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::domain::{
    AgentConfig, AgentToolArgs, AgentToolCall, AgentToolName, AgentToolSchema, ConversationTurn,
    ProviderResponse, SystemPrompt,
};
use super::errors::{AppError, AppResult};

/// Model-agnostic provider contract. Implementations (OpenAICompatibleProvider for GLM/MiniMax, future
/// Claude/Gemini providers) translate domain types to/from their wire format.
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
        Self { script, call_count: 0, error: None }
    }

    pub fn always(response: ProviderResponse) -> Self {
        Self::new(vec![response; 100])
    }

    pub fn always_error(message: &str) -> Self {
        Self { script: vec![], call_count: 0, error: Some(message.to_string()) }
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
        arguments: AgentToolArgs::Read { path: path.to_string() },
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

// ── OpenAICompatibleProvider: works with any OpenAI-compatible API (GLM, MiniMax, etc.) ──

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
            ConversationTurn::Assistant { content, tool_calls } => OpenAIMessage {
                role: "assistant".to_string(),
                content: content.clone(),
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.iter().map(Self::tool_call_to_openai).collect())
                },
                tool_call_id: None,
            },
            ConversationTurn::Tool { tool_call_id, result } => OpenAIMessage {
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
            AgentToolArgs::Grep { pattern, path, include } => (
                "grep",
                serde_json::json!({ "pattern": pattern, "path": path, "include": include }),
            ),
            AgentToolArgs::EditFile { path, old_string, new_string, replace_all } => (
                "edit_file",
                serde_json::json!({
                    "path": path,
                    "old_string": old_string,
                    "new_string": new_string,
                    "replace_all": replace_all
                }),
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
            other => return Err(AppError::Provider(format!("unknown tool: {other}"))),
        };
        let args: serde_json::Value = serde_json::from_str(&gtc.function.arguments)
            .map_err(|e| AppError::Provider(format!("invalid tool arguments: {e}")))?;
        let arguments = match name {
            AgentToolName::ReadFile => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("read_file missing path".into()))?
                    .to_string();
                AgentToolArgs::Read { path }
            }
            AgentToolName::WriteFile => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("write_file missing path".into()))?
                    .to_string();
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("write_file missing content".into()))?
                    .to_string();
                AgentToolArgs::Write { path, content }
            }
            AgentToolName::ListFiles => {
                let path = args.get("path").and_then(|v| v.as_str()).map(String::from);
                let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
                AgentToolArgs::ListFiles { path, recursive }
            }
            AgentToolName::Grep => {
                let pattern = args
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("grep missing pattern".into()))?
                    .to_string();
                let path = args.get("path").and_then(|v| v.as_str()).map(String::from);
                let include = args.get("include").and_then(|v| v.as_str()).map(String::from);
                AgentToolArgs::Grep { pattern, path, include }
            }
            AgentToolName::EditFile => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("edit_file missing path".into()))?
                    .to_string();
                let old_string = args
                    .get("old_string")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("edit_file missing old_string".into()))?
                    .to_string();
                let new_string = args
                    .get("new_string")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::Provider("edit_file missing new_string".into()))?
                    .to_string();
                let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
                AgentToolArgs::EditFile { path, old_string, new_string, replace_all }
            }
        };
        Ok(AgentToolCall { id: gtc.id, name, arguments })
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
