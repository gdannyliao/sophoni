use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use super::domain::{AgentToolArgs, AgentToolName};
use super::domain::{
    AgentConfig, AgentToolCall, AgentToolSchema, ConversationTurn,
    ProviderResponse, SystemPrompt,
};
use super::errors::{AppError, AppResult};
use super::tool_spec::{find_tool, ToolRegistry};
use tracing::{error, info, warn};

/// 接收流式文本增量的回调。provider 解析 SSE 时，每当收到一段 `delta.content`
/// 就调用它。agent.rs 负责把回调桥接到 `EventSink`（构造 `kind="token"` 事件）。
/// 用 `&dyn Fn` 是为了让 trait 方法签名简单、可对象化，且不引入对 agent 模块的依赖。
pub type TokenSink<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// Model-agnostic provider contract. Implementations (OpenAICompatibleProvider, future
/// OpenAI/Claude providers) translate domain types to/from their wire format.
///
/// `complete` 是非流式入口；`complete_streaming` 是流式入口。`complete` 提供默认实现
/// （内部用 no-op token 回调转调 `complete_streaming`），因此只支持整段返回的假实现
/// （如 `FakeProvider`）只需实现 `complete_streaming`，旧测试零改动。
#[async_trait]
pub trait AgentProvider: Send {
    async fn complete(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[AgentToolSchema],
    ) -> AppResult<ProviderResponse> {
        self.complete_streaming(system, turns, tools, &|_delta| {})
            .await
    }

    /// 流式调用。`on_token` 收到 `delta.content` 增量；工具调用的分片参数在内部按
    /// SSE `index` 累积，累积完整后随 `ProviderResponse::ToolCalls` 一次性返回（参数不
    /// 通过 token 回调流式）。
    async fn complete_streaming(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[AgentToolSchema],
        on_token: TokenSink<'_>,
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
    async fn complete_streaming(
        &mut self,
        _system: &SystemPrompt,
        _turns: &[ConversationTurn],
        _tools: &[AgentToolSchema],
        _on_token: TokenSink<'_>,
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
    /// 工具 registry：wire 格式 ↔ AgentToolCall 的转换走各工具的 parse/serialize_args。
    registry: std::sync::Arc<ToolRegistry>,
}

impl OpenAICompatibleProvider {
    pub fn new(config: AgentConfig, registry: std::sync::Arc<ToolRegistry>) -> Self {
        let http = reqwest::Client::builder()
            // 仅对连接建立阶段设超时（防 DNS/TLS 握手卡死）；流式读取的总时长不在此限制，
            // 由 complete_streaming 内的 60s 无活动超时负责——否则正常的长结果流式输出
            // 会被 reqwest 的总超时误中断（表现为 stream error: error decoding response body）。
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self {
            config,
            http,
            registry,
        }
    }

    /// Translate a model-agnostic turn into the GLM wire format.
    pub(crate) fn turn_to_openai_message(
        turn: &ConversationTurn,
        registry: &ToolRegistry,
    ) -> OpenAIMessage {
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
                    Some(
                        tool_calls
                            .iter()
                            .map(|c| Self::tool_call_to_openai(c, registry))
                            .collect(),
                    )
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

    fn tool_call_to_openai(call: &AgentToolCall, registry: &ToolRegistry) -> OpenAIToolCall {
        let name = super::tool_spec::wire_name(&call.name);
        let spec = find_tool(registry, name).unwrap_or_else(|| {
            panic!("tool_call_to_openai: 工具 {name} 未在 registry 注册")
        });
        let arguments = spec.serialize_args(&call.arguments);
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
    pub(crate) fn translate_response(
        resp: OpenAIResponse,
        registry: &ToolRegistry,
    ) -> AppResult<ProviderResponse> {
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
                .map(|gtc| Self::parse_tool_call(gtc, registry))
                .collect::<AppResult<Vec<_>>>()?;
            Ok(ProviderResponse::ToolCalls(calls))
        }
    }

    /// 用 registry 查 spec，调 spec.parse 把 wire JSON 解析成 AgentToolCall。
    fn parse_tool_call(gtc: OpenAIToolCall, registry: &ToolRegistry) -> AppResult<AgentToolCall> {
        let spec = find_tool(registry, &gtc.function.name).ok_or_else(|| {
            AppError::Provider(format!("unknown tool: {}", gtc.function.name))
        })?;
        let args: serde_json::Value = serde_json::from_str(&gtc.function.arguments)
            .map_err(|e| AppError::Provider(format!("invalid tool arguments: {e}")))?;
        spec.parse(&gtc.id, &args)
    }
}

#[async_trait]
impl AgentProvider for OpenAICompatibleProvider {
    #[allow(unused_assignments)]
    async fn complete_streaming(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[AgentToolSchema],
        on_token: TokenSink<'_>,
    ) -> AppResult<ProviderResponse> {
        let mut messages = Vec::with_capacity(turns.len() + 1);
        messages.push(OpenAIMessage {
            role: "system".to_string(),
            content: Some(system.0.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
        for turn in turns {
            messages.push(Self::turn_to_openai_message(turn, &self.registry));
        }

        let openai_tools: Vec<OpenAIToolDef> = tools.iter().map(Self::tool_schema_to_openai).collect();
        let req = OpenAIRequest {
            model: self.config.model.clone(),
            messages,
            tools: Some(openai_tools),
            tool_choice: Some("auto".to_string()),
            stream: Some(true),
        };

        info!(
            model = %self.config.model,
            turns = turns.len(),
            tools = tools.len(),
            "provider: POST /chat/completions (stream)"
        );
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
            error!(%status, %body, "provider: HTTP error");
            return Err(AppError::Provider(format!("HTTP {status}: {body}")));
        }

        use futures_util::StreamExt;
        use std::time::{Duration, Instant};
        let mut stream = resp.bytes_stream();
        let mut line_buf = SseLineBuffer::new();
        let mut content_acc = String::new();
        let mut tool_acc = ToolCallAccumulator::new();

        // token 批量合并：避免每个 token 都触发一次 Tauri IPC（跨进程）+ 前端重渲染，
        // 否则高频 token 会淹没前端主线程导致 UI 卡死。按 30ms 时间窗口累积 delta，
        // 窗口满或循环结束时 flush。这样 IPC 频率上限约 33Hz，肉眼仍是逐字效果。
        const FLUSH_INTERVAL: Duration = Duration::from_millis(30);
        let mut pending = String::new();
        let mut last_flush = Instant::now();
        // 宏化 flush：清空缓冲并重置计时，空缓冲时跳过。
        macro_rules! flush_pending {
            () => {
                if !pending.is_empty() {
                    on_token(&pending);
                    pending.clear();
                }
                last_flush = Instant::now();
            };
        }

        // 无活动超时：每次等待下一个 chunk 最多 STREAM_IDLE_TIMEOUT，持续来数据则永不超时。
        // 只抓真正卡住（网络中断/模型无响应），不误杀正常的长结果流式输出
        // （原来 agent 层的 30s 总时长超时会砍断长报告的流式输出）。
        const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
        loop {
            let next = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await;
            let chunk_res = match next {
                Ok(Some(c)) => c,
                Ok(None) => break, // 流正常结束
                Err(_elapsed) => {
                    warn!(idle_secs = STREAM_IDLE_TIMEOUT.as_secs(), "provider: 流式响应无活动超时");
                    return Err(AppError::Provider(
                        "流式响应无活动超时(60s)，可能是网络中断或模型无响应".into(),
                    ));
                }
            };
            let bytes = chunk_res.map_err(|e| {
                warn!(error = %e, "provider: stream error");
                AppError::Provider(format!("stream error: {e}"))
            })?;
            let text = String::from_utf8_lossy(&bytes);
            for json_line in line_buf.feed(&text) {
                if json_line == "[DONE]" {
                    continue;
                }
                let parsed: StreamChunk = serde_json::from_str(&json_line).map_err(|e| {
                    warn!(error = %e, "provider: SSE parse error");
                    AppError::Provider(format!("failed to parse SSE chunk: {e} (line: {json_line})"))
                })?;
                let Some(choice) = parsed.choices.into_iter().next() else {
                    continue;
                };
                if let Some(delta_text) = choice.delta.content {
                    if !delta_text.is_empty() {
                        content_acc.push_str(&delta_text);
                        pending.push_str(&delta_text);
                        // 窗口到期才 flush，把高频 delta 合并成一次回调。
                        if last_flush.elapsed() >= FLUSH_INTERVAL {
                            flush_pending!();
                        }
                    }
                }
                if let Some(tc_list) = choice.delta.tool_calls {
                    for stc in tc_list {
                        tool_acc.ingest(stc);
                    }
                }
            }
        }
        // 流结束：冲刷残余 token，保证最后一段文本不丢。
        flush_pending!();

        // 累积完成，组装成非流式响应复用既有翻译逻辑。
        let finalized_tools = tool_acc.finalize();
        let has_tools = !finalized_tools.is_empty();
        let openai_resp = OpenAIResponse {
            choices: vec![OpenAIChoice {
                message: OpenAIMessage {
                    role: "assistant".to_string(),
                    content: if content_acc.is_empty() { None } else { Some(content_acc) },
                    tool_calls: if has_tools { Some(finalized_tools) } else { None },
                    tool_call_id: None,
                },
            }],
        };
        Self::translate_response(openai_resp, &self.registry)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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

// ── SSE 流式 DTO（OpenAI 兼容协议）──
// 每个 chunk 形如 `data: {"choices":[{"delta":{"content":"hi","tool_calls":[...]}}]}`
// tool_calls 在流式中按 `index` 分片：首片含 id/function.name，后续片只含
// function.arguments 的片段，需按 index 累积拼接。

#[derive(Deserialize, Debug)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize, Debug)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Deserialize, Debug, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Deserialize, Debug)]
struct StreamToolCall {
    /// 在 choices[0].delta.tool_calls 数组中的下标，用于累积分片。
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamToolFunction>,
}

#[derive(Deserialize, Debug, Default)]
struct StreamToolFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// 累积流式 tool_call 分片。最终聚合为完整 `OpenAIToolCall` 列表。
#[derive(Default)]
struct ToolCallAccumulator {
    /// index → 累积状态
    entries: std::collections::BTreeMap<usize, AccEntry>,
}

#[derive(Default)]
struct AccEntry {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn new() -> Self {
        Self::default()
    }

    fn ingest(&mut self, stc: StreamToolCall) {
        let entry = self.entries.entry(stc.index).or_default();
        if let Some(id) = stc.id {
            entry.id = id;
        }
        if let Some(func) = stc.function {
            if let Some(name) = func.name {
                entry.name = name;
            }
            if let Some(args) = func.arguments {
                entry.arguments.push_str(&args);
            }
        }
    }

    /// 累积完成，按 index 顺序产出完整 tool_call（wire 格式）。
    fn finalize(self) -> Vec<OpenAIToolCall> {
        self.entries
            .into_values()
            .map(|e| OpenAIToolCall {
                id: e.id,
                kind: "function".to_string(),
                function: OpenAIFunction {
                    name: e.name,
                    arguments: e.arguments,
                },
            })
            .collect()
    }
}

/// 把 SSE 原始字节流（可能含多个 `data: ...` 行，跨 chunk 边界）切成完整 JSON 行。
/// 维护一个行缓冲区：每遇到 `\n` 就把已累积的行输出，剩余部分保留。
struct SseLineBuffer {
    buf: String,
}

impl SseLineBuffer {
    fn new() -> Self {
        Self { buf: String::new() }
    }

    /// 喂入一段字节，返回本次新形成的完整行（已去 `data: ` 前缀和首尾空白）。
    fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            let trimmed = line.trim();
            if let Some(json) = trimmed.strip_prefix("data:").or_else(|| trimmed.strip_prefix("data: ")) {
                let json = json.trim();
                if json.is_empty() {
                    continue;
                }
                lines.push(json.to_string());
            }
            // 其他行（空行、`: comment`、event: 等）忽略
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::super::domain::{AgentToolArgs, AgentToolResult, ConversationTurn, ProviderResponse};
    use super::super::command_risk::RiskLevel;
    use super::super::tool_spec::build_tool_registry;
    use super::super::workspace::WorkspaceFs;
    use super::{
        OpenAIChoice, OpenAIFunction, OpenAIMessage, OpenAICompatibleProvider, OpenAIResponse,
        OpenAIToolCall, SseLineBuffer, StreamChunk, StreamToolCall, StreamToolFunction,
        ToolCallAccumulator,
    };
    use std::sync::Arc;

    /// 构造测试用 registry：parse/serialize 不碰 fs，路径随便给。
    fn test_registry() -> Arc<super::super::tool_spec::ToolRegistry> {
        let fs = WorkspaceFs::new(std::env::temp_dir().join(format!(
            "sophoni-provider-test-{}",
            uuid::Uuid::new_v4()
        )));
        let http = reqwest::Client::new();
        Arc::new(build_tool_registry(fs, RiskLevel::Standard, None, None, http))
    }

    #[test]
    fn glm_translates_user_turn_to_message() {
        let turn = ConversationTurn::User { content: "hi".into() };
        let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn, &test_registry());
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
        let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn, &test_registry());
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let result = OpenAICompatibleProvider::translate_response(resp, &test_registry());
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
        let translated = OpenAICompatibleProvider::translate_response(resp, &test_registry()).unwrap();
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
        let result = OpenAICompatibleProvider::translate_response(resp, &test_registry());
        assert!(result.is_err());
    }

    // ── SSE 流式解析单测 ──

    #[test]
    fn sse_line_buffer_parses_data_lines() {
        let mut buf = SseLineBuffer::new();
        let lines = buf.feed("data: {\"a\":1}\n\n");
        assert_eq!(lines, vec!["{\"a\":1}"]);
    }

    #[test]
    fn sse_line_buffer_handles_split_across_chunks() {
        let mut buf = SseLineBuffer::new();
        // 一个 SSE 事件跨多个字节块到达
        let first = buf.feed("data: {\"par");
        assert!(first.is_empty(), "未形成完整行不应返回");
        let second = buf.feed("t\":42}\n");
        assert_eq!(second, vec!["{\"part\":42}"]);
    }

    #[test]
    fn sse_line_buffer_ignores_non_data_lines() {
        let mut buf = SseLineBuffer::new();
        let lines = buf.feed(": keepalive\n\nevent: ping\ndata: {\"ok\":true}\n\n");
        assert_eq!(lines, vec!["{\"ok\":true}"]);
    }

    #[test]
    fn sse_line_buffer_accepts_done_marker() {
        let mut buf = SseLineBuffer::new();
        let lines = buf.feed("data: [DONE]\n\n");
        assert_eq!(lines, vec!["[DONE]"]);
    }

    #[test]
    fn tool_call_accumulator_assembles_fragmented_arguments() {
        let mut acc = ToolCallAccumulator::new();
        // 首片：id + name
        acc.ingest(StreamToolCall {
            index: 0,
            id: Some("call-1".into()),
            function: Some(StreamToolFunction {
                name: Some("read_file".into()),
                arguments: Some("{\"path\":\"RE".into()),
            }),
        });
        // 第二片：arguments 续片
        acc.ingest(StreamToolCall {
            index: 0,
            id: None,
            function: Some(StreamToolFunction {
                name: None,
                arguments: Some("ADME.md\"}".into()),
            }),
        });
        let calls = acc.finalize();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-1");
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(calls[0].function.arguments, r#"{"path":"README.md"}"#);
    }

    #[test]
    fn tool_call_accumulator_orders_by_index() {
        let mut acc = ToolCallAccumulator::new();
        acc.ingest(StreamToolCall {
            index: 2,
            id: Some("c2".into()),
            function: Some(StreamToolFunction {
                name: Some("grep".into()),
                arguments: Some("{}".into()),
            }),
        });
        acc.ingest(StreamToolCall {
            index: 0,
            id: Some("c0".into()),
            function: Some(StreamToolFunction {
                name: Some("read_file".into()),
                arguments: Some("{}".into()),
            }),
        });
        let calls = acc.finalize();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "c0");
        assert_eq!(calls[1].id, "c2");
    }

    #[test]
    fn stream_chunk_parses_content_delta() {
        let json = r#"{"choices":[{"delta":{"content":"hello"}}]}"#;
        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        let choice = chunk.choices.into_iter().next().unwrap();
        assert_eq!(choice.delta.content.as_deref(), Some("hello"));
        assert!(choice.delta.tool_calls.is_none());
    }

    #[test]
    fn stream_chunk_parses_tool_call_delta() {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#;
        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        let choice = chunk.choices.into_iter().next().unwrap();
        let tc = choice.delta.tool_calls.unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].index, 0);
        assert_eq!(tc[0].id.as_deref(), Some("c1"));
        assert_eq!(tc[0].function.as_ref().unwrap().name.as_deref(), Some("read_file"));
    }
}
