use async_trait::async_trait;

use super::domain::{
    AgentToolArgs, AgentToolCall, AgentToolName, AgentToolSchema, ConversationTurn,
    ProviderResponse, SystemPrompt,
};
use super::errors::{AppError, AppResult};

/// Model-agnostic provider contract. Implementations (GlmProvider, future
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
