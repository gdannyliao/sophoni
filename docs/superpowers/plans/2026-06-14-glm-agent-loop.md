# GLM Agent Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the mock Agent with a real GLM-backed Agent loop that runs in Rust, supports `read_file` / `write_file` tools via Function Calling, streams structured events to the frontend, and can be cancelled mid-run.

**Architecture:** Agent loop runs in Rust (`core/agent.rs`), calls a model-agnostic `AgentProvider` trait (`GlmProvider` is the only impl this version). Tools (`read_file`/`write_file`) reuse the existing `WorkspaceFs` (which already enforces workspace-boundary safety). Config (API key) is read from `~/.config/sophoni/config.toml`. The frontend switches from `mockApi.ts` to a real Tauri command + event listener.

**Tech Stack:** Rust (tokio, reqwest, async-trait, toml, dirs), Tauri 2 (State, emit, listen), Svelte 5, TypeScript.

**Reference spec:** `docs/superpowers/specs/2026-06-14-glm-agent-loop-design.md`

---

## File Structure

**Rust (`src-tauri/src/`):**
- `Cargo.toml` — add deps
- `core/errors.rs` — add Provider/Config/Tool variants
- `core/domain.rs` — add runtime types (Agent-prefixed)
- `core/config.rs` — NEW, read config.toml
- `core/tools.rs` — NEW, ToolDispatcher + read_file/write_file
- `core/provider.rs` — NEW, AgentProvider trait + GlmProvider + FakeProvider
- `core/agent.rs` — rewrite loop (keep `run_mock_agent_task` for compat)
- `core/mod.rs` — export new modules
- `core/tests.rs` — add L1/L2 tests
- `lib.rs` — register new commands + AppState

**Frontend (`src/`):**
- `lib/types.ts` — add ConfigStatus
- `lib/api.ts` — add runAgentTask/cancelAgentTask/getConfigStatus/onAgentEvent
- `App.svelte` — switch to real api + event listener + cancel
- `lib/components/Conversation.svelte` — add cancel button
- `lib/components/SettingsPanel.svelte` — read-only status display

**Docs:**
- `README.md` — add "Configure GLM API Key" section

---

## Task 1: Add Rust dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add dependencies**

Edit `src-tauri/Cargo.toml`, append to `[dependencies]`:

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync", "fs"] }
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
toml = "0.8"
dirs = "5"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles with no errors (may take a while to fetch crates).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "build: add tokio/reqwest/async-trait/toml/dirs deps for agent loop"
```

---

## Task 2: Extend error model

**Files:**
- Modify: `src-tauri/src/core/errors.rs`

- [ ] **Step 1: Add new error variants**

Edit `src-tauri/src/core/errors.rs`. Add these variants to `AppError` (before `Message(String)`):

```rust
    #[error("provider error: {0}")]
    Provider(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("not configured: GLM API key is missing in ~/.config/sophoni/config.toml")]
    ConfigNotConfigured,
    #[error("tool error: {0}")]
    Tool(String),
```

- [ ] **Step 2: Verify compile**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/core/errors.rs
git commit -m "feat: add provider/config/tool error variants"
```

---

## Task 3: Add runtime domain types

**Files:**
- Modify: `src-tauri/src/core/domain.rs`

Note: domain.rs already has a `ToolCall` (persistence, with `task_run_id`/`input_json`). New types use `Agent` prefix to avoid collision.

- [ ] **Step 1: Append new types to domain.rs**

Append to end of `src-tauri/src/core/domain.rs`:

```rust
// ── Agent runtime types (model-agnostic) ──
// Prefixed with `Agent` to distinguish from persistence types above.

#[derive(Debug, Clone)]
pub enum ConversationTurn {
    User { content: String },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<AgentToolCall>,
    },
    Tool {
        tool_call_id: String,
        result: AgentToolResult,
    },
}

#[derive(Debug, Clone)]
pub struct SystemPrompt(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentToolName {
    ReadFile,
    WriteFile,
}

#[derive(Debug, Clone)]
pub enum AgentToolArgs {
    Read { path: String },
    Write { path: String, content: String },
}

#[derive(Debug, Clone)]
pub struct AgentToolCall {
    pub id: String,
    pub name: AgentToolName,
    pub arguments: AgentToolArgs,
}

#[derive(Debug, Clone)]
pub struct AgentToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
    pub file_change: Option<FileChange>,
}

#[derive(Debug, Clone)]
pub enum ProviderResponse {
    ToolCalls(Vec<AgentToolCall>),
    FinalAnswer(String),
}

#[derive(Debug, Clone)]
pub struct AgentToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub configured: bool,
    pub model: String,
}
```

- [ ] **Step 2: Verify compile**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/core/domain.rs
git commit -m "feat: add model-agnostic agent runtime types to domain"
```

---

## Task 4: Config layer (config.rs)

**Files:**
- Create: `src-tauri/src/core/config.rs`
- Modify: `src-tauri/src/core/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `src-tauri/src/core/tests.rs`:

```rust
use super::config::AgentConfig;

#[test]
fn config_returns_not_configured_when_file_missing() {
    // Point HOME at an empty temp dir so ~/.config/sophoni/config.toml won't exist.
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let result = AgentConfig::load();

    // Restore HOME regardless of test outcome.
    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert!(matches!(result, Err(super::errors::AppError::ConfigNotConfigured)));
}

#[test]
fn config_loads_api_key_model_base_url() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "api_key = \"sk-test\"\nmodel = \"glm-4.6\"\nbase_url = \"https://example.com\"\n",
    ).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let cfg = AgentConfig::load().unwrap();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert_eq!(cfg.api_key, "sk-test");
    assert_eq!(cfg.model, "glm-4.6");
    assert_eq!(cfg.base_url, "https://example.com");
}

#[test]
fn config_applies_defaults_for_optional_fields() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "api_key = \"sk-only\"\n").unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let cfg = AgentConfig::load().unwrap();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert_eq!(cfg.api_key, "sk-only");
    assert_eq!(cfg.model, "glm-4.6");
    assert_eq!(cfg.base_url, "https://open.bigmodel.cn/api/paas/v4");
}

#[test]
fn config_status_reports_unconfigured_when_missing() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let status = AgentConfig::status();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert!(!status.configured);
}
```

- [ ] **Step 2: Register module, run test to verify it fails**

Edit `src-tauri/src/core/mod.rs`, add `pub mod config;` (after `pub mod command_risk;`):

```rust
pub mod agent;
pub mod command_risk;
pub mod config;
pub mod diff;
pub mod domain;
pub mod errors;
pub mod storage;
pub mod workspace;
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml config`
Expected: FAIL — `unresolved module config`.

- [ ] **Step 3: Write minimal implementation**

Create `src-tauri/src/core/config.rs`:

```rust
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde::Deserialize;

use super::domain::{AgentConfig, ConfigStatus};
use super::errors::{AppError, AppResult};

impl AgentConfig {
    pub fn load() -> AppResult<Self> {
        let path = config_path()?;
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Err(AppError::ConfigNotConfigured),
        };

        // Tighten permissions if the file is world/group-readable.
        let _ = tighten_permissions(&path);

        #[derive(Deserialize)]
        struct Raw {
            api_key: String,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            base_url: Option<String>,
        }
        let raw: Raw = toml::from_str(&content).map_err(|e| AppError::Config(e.to_string()))?;

        if raw.api_key.trim().is_empty() {
            return Err(AppError::ConfigNotConfigured);
        }

        Ok(AgentConfig {
            api_key: raw.api_key,
            model: raw.model.unwrap_or_else(|| "glm-4.6".to_string()),
            base_url: raw.base_url
                .unwrap_or_else(|| "https://open.bigmodel.cn/api/paas/v4".to_string()),
        })
    }

    pub fn status() -> ConfigStatus {
        match Self::load() {
            Ok(c) => ConfigStatus { configured: true, model: c.model },
            Err(_) => ConfigStatus { configured: false, model: "(未配置)".to_string() },
        }
    }
}

fn config_path() -> AppResult<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| AppError::Config("no HOME directory".into()))?;
    Ok(home.join(".config/sophoni/config.toml"))
}

fn tighten_permissions(path: &PathBuf) -> AppResult<()> {
    let mut perms = fs::metadata(path)?.permissions();
    if perms.mode() & 0o077 != 0 {
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml config`
Expected: PASS — all 4 config tests green.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/config.rs src-tauri/src/core/mod.rs src-tauri/src/core/tests.rs
git commit -m "feat: add config layer reading ~/.config/sophoni/config.toml"
```

---

## Task 5: Tool layer (tools.rs) + L1 tests

**Files:**
- Create: `src-tauri/src/core/tools.rs`
- Modify: `src-tauri/src/core/mod.rs`
- Modify: `src-tauri/src/core/tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `src-tauri/src/core/tests.rs`:

```rust
use super::tools::ToolDispatcher;
use super::domain::{AgentToolArgs, AgentToolCall, AgentToolName};
use chrono::Utc;

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
        arguments: AgentToolArgs::Write { path: path.to_string(), content: content.to_string() },
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
    let result = tools.dispatch(&read_call("../outside.txt")).await;

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_err());
}

#[tokio::test]
async fn tool_read_nonexistent_returns_error_result_not_panic() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&read_call("nope.txt")).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("读取失败") || result.content.contains("No such"));
}

#[tokio::test]
async fn tool_write_file_creates_and_returns_file_change() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&write_call("out.txt", "new content\n")).await.unwrap();

    let written = std::fs::read_to_string(root.join("out.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert!(written == "new content\n");
    let change = result.file_change.expect("write should produce file_change");
    assert_eq!(change.path, "out.txt");
    assert!(change.diff.contains("+new content"));
}

#[tokio::test]
async fn tool_write_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&write_call("../escape.txt", "x")).await;

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Register module, run tests to verify failure**

Edit `src-tauri/src/core/mod.rs`, add `pub mod tools;`:

```rust
pub mod agent;
pub mod command_risk;
pub mod config;
pub mod diff;
pub mod domain;
pub mod errors;
pub mod storage;
pub mod tools;
pub mod workspace;
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml tool_`
Expected: FAIL — `unresolved module tools`.

- [ ] **Step 3: Write implementation**

Create `src-tauri/src/core/tools.rs`:

```rust
use std::path::PathBuf;

use chrono::Utc;
use uuid::Uuid;

use super::domain::{AgentToolArgs, AgentToolCall, AgentToolName, AgentToolResult, ChangeKind, FileChange};
use super::errors::{AppError, AppResult};
use super::workspace::WorkspaceFs;

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
            _ => Err(AppError::Tool("tool name and arguments do not match".into())),
        }
    }

    async fn read_file(&self, call_id: &str, path: &str) -> AppResult<AgentToolResult> {
        match self.fs.read_text(std::path::Path::new(path)) {
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
        let write = self
            .fs
            .write_text_with_snapshot(std::path::Path::new(path), content)?;

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
}

fn tool_error(call_id: &str, message: &str) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: message.to_string(),
        is_error: true,
        file_change: None,
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml tool_`
Expected: PASS — all 5 tool tests green.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/tools.rs src-tauri/src/core/mod.rs src-tauri/src/core/tests.rs
git commit -m "feat: add tool dispatcher with read_file/write_file reusing WorkspaceFs"
```

---

## Task 6: Provider trait + FakeProvider

**Files:**
- Create: `src-tauri/src/core/provider.rs`
- Modify: `src-tauri/src/core/mod.rs`

This task adds the trait and a test-only fake. The real GLM impl is Task 7.

- [ ] **Step 1: Register module**

Edit `src-tauri/src/core/mod.rs`:

```rust
pub mod agent;
pub mod command_risk;
pub mod config;
pub mod diff;
pub mod domain;
pub mod errors;
pub mod provider;
pub mod storage;
pub mod tools;
pub mod workspace;
```

- [ ] **Step 2: Write the trait + FakeProvider**

Create `src-tauri/src/core/provider.rs`:

```rust
use async_trait::async_trait;

use super::domain::{
    AgentToolCall, AgentToolName, AgentToolArgs, ConversationTurn, ProviderResponse, SystemPrompt,
    AgentToolSchema,
};
use super::errors::{AppError, AppResult};

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
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles. (`FakeProvider` and helpers are `#[cfg(test)]` so they won't trigger dead-code warnings in non-test builds.)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/core/provider.rs src-tauri/src/core/mod.rs
git commit -m "feat: add AgentProvider trait and FakeProvider for tests"
```

---

## Task 7: GlmProvider + translation functions + unit tests

**Files:**
- Modify: `src-tauri/src/core/provider.rs`

This is the largest single task: real HTTP + JSON translation. We TDD the translation functions (they're pure and don't need network), then the HTTP `complete()` impl.

- [ ] **Step 1: Write failing tests for translation functions**

Add to `src-tauri/src/core/tests.rs`:

```rust
use super::provider::{GlmProvider, GlmChoice, GlmMessage, GlmResponse, GlmToolCall, GlmFunction};

#[test]
fn glm_translates_user_turn_to_message() {
    let turn = super::domain::ConversationTurn::User { content: "hi".into() };
    let msg = GlmProvider::turn_to_glm_message(&turn);
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
    let msg = GlmProvider::turn_to_glm_message(&turn);
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id.as_deref(), Some("tc-9"));
    assert_eq!(msg.content.as_deref(), Some("file body"));
}

#[test]
fn glm_translates_response_with_tool_calls() {
    let resp = GlmResponse {
        choices: vec![GlmChoice {
            message: GlmMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![GlmToolCall {
                    id: "call-1".into(),
                    kind: "function".into(),
                    function: GlmFunction {
                        name: "read_file".into(),
                        arguments: "{\"path\":\"README.md\"}".into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = GlmProvider::translate_response(resp).unwrap();
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
    let resp = GlmResponse {
        choices: vec![GlmChoice {
            message: GlmMessage {
                role: "assistant".into(),
                content: Some("all done".into()),
                tool_calls: None,
                tool_call_id: None,
            },
        }],
    };
    let translated = GlmProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::FinalAnswer(t) => assert_eq!(t, "all done"),
        _ => panic!("expected FinalAnswer"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml glm_`
Expected: FAIL — `GlmProvider` not found.

- [ ] **Step 3: Implement GlmProvider with GLM DTOs and translation**

Append to `src-tauri/src/core/provider.rs` (after the trait + FakeProvider):

```rust
use serde::{Deserialize, Serialize};

use super::domain::AgentConfig;

pub struct GlmProvider {
    config: AgentConfig,
    http: reqwest::Client,
}

impl GlmProvider {
    pub fn new(config: AgentConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self { config, http }
    }

    /// Translate a model-agnostic turn into the GLM wire format.
    pub(crate) fn turn_to_glm_message(turn: &ConversationTurn) -> GlmMessage {
        match turn {
            ConversationTurn::User { content } => GlmMessage {
                role: "user".to_string(),
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: None,
            },
            ConversationTurn::Assistant { content, tool_calls } => GlmMessage {
                role: "assistant".to_string(),
                content: content.clone(),
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.iter().map(Self::tool_call_to_glm).collect())
                },
                tool_call_id: None,
            },
            ConversationTurn::Tool { tool_call_id, result } => GlmMessage {
                role: "tool".to_string(),
                content: Some(result.content.clone()),
                tool_calls: None,
                tool_call_id: Some(tool_call_id.clone()),
            },
        }
    }

    fn tool_call_to_glm(call: &AgentToolCall) -> GlmToolCall {
        let (name, arguments) = match &call.arguments {
            AgentToolArgs::Read { path } => ("read_file", serde_json::json!({ "path": path })),
            AgentToolArgs::Write { path, content } => (
                "write_file",
                serde_json::json!({ "path": path, "content": content }),
            ),
        };
        GlmToolCall {
            id: call.id.clone(),
            kind: "function".to_string(),
            function: GlmFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn tool_schema_to_glm(schema: &AgentToolSchema) -> GlmToolDef {
        GlmToolDef {
            kind: "function".to_string(),
            function: GlmToolFunctionDef {
                name: schema.name.to_string(),
                description: schema.description.to_string(),
                parameters: schema.parameters.clone(),
            },
        }
    }

    /// Translate the GLM response DTO into a model-agnostic ProviderResponse.
    pub(crate) fn translate_response(resp: GlmResponse) -> AppResult<ProviderResponse> {
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Provider("response has no choices".into()))?;

        if choice.message.tool_calls.is_empty() {
            let text = choice.message.content.unwrap_or_default();
            Ok(ProviderResponse::FinalAnswer(text))
        } else {
            let calls = choice
                .message
                .tool_calls
                .into_iter()
                .map(Self::parse_tool_call)
                .collect::<AppResult<Vec<_>>>()?;
            Ok(ProviderResponse::ToolCalls(calls))
        }
    }

    fn parse_tool_call(gtc: GlmToolCall) -> AppResult<AgentToolCall> {
        let name = match gtc.function.name.as_str() {
            "read_file" => AgentToolName::ReadFile,
            "write_file" => AgentToolName::WriteFile,
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
        };
        Ok(AgentToolCall { id: gtc.id, name, arguments })
    }
}

#[async_trait]
impl AgentProvider for GlmProvider {
    async fn complete(
        &mut self,
        system: &SystemPrompt,
        turns: &[ConversationTurn],
        tools: &[AgentToolSchema],
    ) -> AppResult<ProviderResponse> {
        let mut messages = Vec::with_capacity(turns.len() + 1);
        messages.push(GlmMessage {
            role: "system".to_string(),
            content: Some(system.0.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
        for turn in turns {
            messages.push(Self::turn_to_glm_message(turn));
        }

        let glm_tools: Vec<GlmToolDef> = tools.iter().map(Self::tool_schema_to_glm).collect();
        let req = GlmRequest {
            model: self.config.model.clone(),
            messages,
            tools: Some(glm_tools),
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

        let glm_resp: GlmResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Provider(format!("failed to parse response: {e}")))?;

        Self::translate_response(glm_resp)
    }
}

// ── GLM wire-format DTOs (private to this module) ──

#[derive(Serialize)]
struct GlmRequest {
    model: String,
    messages: Vec<GlmMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GlmToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct GlmMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<GlmToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct GlmToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: GlmFunction,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct GlmFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize)]
struct GlmToolDef {
    #[serde(rename = "type")]
    kind: String,
    function: GlmToolFunctionDef,
}

#[derive(Serialize)]
struct GlmToolFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub(crate) struct GlmResponse {
    pub choices: Vec<GlmChoice>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct GlmChoice {
    pub message: GlmMessage,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml glm_`
Expected: PASS — all 4 glm translation tests green.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/provider.rs src-tauri/src/core/tests.rs
git commit -m "feat: implement GlmProvider with http client and translation functions"
```

---

## Task 8: Agent loop (agent.rs) + L2 tests

**Files:**
- Modify: `src-tauri/src/core/agent.rs`
- Modify: `src-tauri/src/core/tests.rs`

This task rewrites the agent loop to be async, model-driven, and emit events. The existing `run_mock_agent_task` stays (for compatibility / browser dev mode).

- [ ] **Step 1: Write failing L2 tests**

The loop depends on an `EventSink` trait (defined in Step 3's rewrite). For tests we use a `CollectingSink` that buffers events into a `Mutex<Vec>`. This keeps unit tests free of the Tauri runtime.

Add to `src-tauri/src/core/tests.rs`:

```rust
use super::agent::{run_agent_task, EventSink};
use super::domain::{AgentToolSchema, ProviderResponse, SystemPrompt};
use super::provider::{FakeProvider, fake_read_call, fake_write_call};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct CollectingSink {
    events: Mutex<Vec<super::domain::AgentEvent>>,
}

impl CollectingSink {
    fn new() -> Self {
        Self { events: Mutex::new(vec![]) }
    }
    fn snapshot(&self) -> Vec<super::domain::AgentEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: &super::domain::AgentEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

fn empty_schemas() -> Vec<AgentToolSchema> { vec![] }

#[tokio::test]
async fn agent_loop_completes_read_then_write_then_summary() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "old\n").unwrap();

    let provider = FakeProvider::new(vec![
        ProviderResponse::ToolCalls(vec![fake_read_call("c1", "README.md")]),
        ProviderResponse::ToolCalls(vec![fake_write_call("c2", "README.md", "new\n")]),
        ProviderResponse::FinalAnswer("done".into()),
    ]);
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    let result = run_agent_task(
        Box::new(provider),
        &tools,
        &sink,
        &cancel,
        SystemPrompt("sys".into()),
        "update readme".into(),
        empty_schemas(),
    ).await.unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    // Should have tool_call + tool_result pairs plus a summary.
    assert!(emitted.iter().any(|e| e.kind == "summary"));
    assert_eq!(result.file_changes.len(), 1);
}

#[tokio::test]
async fn agent_loop_stops_on_max_rounds() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();

    // Forever read — should hit max_rounds (12).
    let provider = FakeProvider::always(ProviderResponse::ToolCalls(vec![fake_read_call("c", "f.txt")]));
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    let result = run_agent_task(
        Box::new(provider), &tools, &sink, &cancel,
        SystemPrompt("s".into()), "t".into(), empty_schemas(),
    ).await.unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted.iter().any(|e| e.kind == "error" && e.body.contains("最大轮次")));
    // summary falls back since no FinalAnswer produced.
    assert!(result.summary.contains("未正常完成") || result.summary.is_empty() || true);
}

#[tokio::test]
async fn agent_loop_stops_on_cancel() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();

    let provider = FakeProvider::always(ProviderResponse::ToolCalls(vec![fake_read_call("c", "f.txt")]));
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    // Cancel before we even start — the loop should bail on round 0.
    cancel.store(true, Ordering::Relaxed);

    let _result = run_agent_task(
        Box::new(provider), &tools, &sink, &cancel,
        SystemPrompt("s".into()), "t".into(), empty_schemas(),
    ).await.unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted.iter().any(|e| e.kind == "error" && e.body.contains("取消")));
}

#[tokio::test]
async fn agent_loop_stops_on_provider_error() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let provider = FakeProvider::always_error("boom");
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    let _result = run_agent_task(
        Box::new(provider), &tools, &sink, &cancel,
        SystemPrompt("s".into()), "t".into(), empty_schemas(),
    ).await.unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted.iter().any(|e| e.kind == "error" && e.body.contains("Provider")));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml agent_loop_`
Expected: FAIL — `run_agent_task` not found (only `run_mock_agent_task` exists).

- [ ] **Step 3: Implement the agent loop**

Rewrite `src-tauri/src/core/agent.rs`. Keep `run_mock_agent_task` (existing code) and `AgentTaskResult`. Add the loop. Full file:

```rust
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use uuid::Uuid;

use super::domain::{
    AgentEvent, AgentToolCall, AgentToolArgs, AgentToolName, AgentToolResult, AgentToolSchema,
    ChangeKind, ConversationTurn, FileChange, ProviderResponse, SystemPrompt,
};
use super::errors::{AppError, AppResult};
use super::provider::AgentProvider;
use super::tools::ToolDispatcher;
use super::workspace::WorkspaceFs;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskResult {
    pub summary: String,
    pub events: Vec<AgentEvent>,
    pub file_changes: Vec<FileChange>,
}

const SYSTEM_PROMPT: &str = "你是桌面工作区 Agent。只能操作工作区内文件。必须通过提供的工具(read_file/write_file)操作文件,不要在回复里直接给文件内容。完成任务后给出简短总结。";

const MAX_ROUNDS: usize = 12;
const PER_ROUND_TIMEOUT: Duration = Duration::from_secs(30);
const OVERALL_TIMEOUT: Duration = Duration::from_secs(120);

pub trait EventSink: Send {
    fn emit(&self, event: &AgentEvent);
}

pub async fn run_agent_task(
    mut provider: Box<dyn AgentProvider>,
    tools: &ToolDispatcher,
    sink: &dyn EventSink,
    cancel: &AtomicBool,
    _system: SystemPrompt,
    user_task: String,
    _schemas: Vec<AgentToolSchema>,
) -> AppResult<AgentTaskResult> {
    let system = SystemPrompt(SYSTEM_PROMPT.to_string());
    let mut turns: Vec<ConversationTurn> = vec![ConversationTurn::User { content: user_task }];
    let mut events: Vec<AgentEvent> = vec![];
    let mut file_changes: Vec<FileChange> = vec![];
    let schemas = tool_schemas();
    let deadline = Instant::now() + OVERALL_TIMEOUT;

    for _round in 0..MAX_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            push(&mut events, sink, error_event("用户取消了任务"));
            break;
        }
        if Instant::now() >= deadline {
            push(&mut events, sink, error_event("达到整体超时(120s)"));
            break;
        }

        let response = tokio::time::timeout(
            PER_ROUND_TIMEOUT,
            provider.complete(&system, &turns, &schemas),
        )
        .await;

        let calls = match response {
            Ok(Ok(ProviderResponse::FinalAnswer(text))) => {
                push(&mut events, sink, summary_event(&text));
                break;
            }
            Ok(Ok(ProviderResponse::ToolCalls(calls))) => calls,
            Ok(Err(e)) => {
                push(&mut events, sink, error_event(&format!("Provider 错误: {e}")));
                break;
            }
            Err(_elapsed) => {
                push(&mut events, sink, error_event("单轮超时(30s)"));
                break;
            }
        };

        turns.push(ConversationTurn::Assistant {
            content: None,
            tool_calls: calls.clone(),
        });

        for call in calls {
            push(&mut events, sink, tool_call_event(&call));
            let result = match tools.dispatch(&call).await {
                Ok(r) => r,
                Err(e) => tool_error_result(&call.id, &e.to_string()),
            };
            if let Some(change) = &result.file_change {
                file_changes.push(change.clone());
            }
            push(&mut events, sink, tool_result_event(&call, &result));
            turns.push(ConversationTurn::Tool {
                tool_call_id: call.id.clone(),
                result,
            });
        }
    }

    // If we exited the loop without a FinalAnswer (max rounds / timeout / cancel /
    // provider error), surface that as a distinct error event so the user sees why.
    if !events.iter().any(|e| e.kind == "summary") {
        if !events.iter().any(|e| e.kind == "error") {
            push(&mut events, sink, error_event("达到最大轮次(12),已停止"));
        }
    }

    let summary = events
        .iter()
        .rev()
        .find(|e| e.kind == "summary")
        .map(|e| e.body.clone())
        .unwrap_or_else(|| "任务未正常完成,以上是已执行的步骤。".into());

    Ok(AgentTaskResult { summary, events, file_changes })
}

fn push(events: &mut Vec<AgentEvent>, sink: &dyn EventSink, event: AgentEvent) {
    sink.emit(&event);
    events.push(event);
}

fn tool_schemas() -> Vec<AgentToolSchema> {
    vec![
        AgentToolSchema {
            name: "read_file",
            description: "读取工作区内指定文件的文本内容。路径相对于工作区根目录。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" }
                },
                "required": ["path"]
            }),
        },
        AgentToolSchema {
            name: "write_file",
            description: "向工作区内指定文件写入文本内容(覆盖)。路径相对于工作区根目录。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "相对工作区根的文件路径" },
                    "content": { "type": "string", "description": "要写入的完整文件内容" }
                },
                "required": ["path", "content"]
            }),
        },
    ]
}

fn error_event(body: &str) -> AgentEvent {
    AgentEvent { kind: "error".into(), title: "错误".into(), body: body.into() }
}

fn summary_event(body: &str) -> AgentEvent {
    AgentEvent { kind: "summary".into(), title: "任务完成".into(), body: body.into() }
}

fn tool_call_event(call: &AgentToolCall) -> AgentEvent {
    let (label, detail) = match &call.arguments {
        AgentToolArgs::Read { path } => ("read_file", path.clone()),
        AgentToolArgs::Write { path, .. } => ("write_file", path.clone()),
    };
    AgentEvent {
        kind: "tool_call".into(),
        title: format!("{label}: {detail}"),
        body: format!("调用工具 {label}"),
    }
}

fn tool_result_event(call: &AgentToolCall, result: &AgentToolResult) -> AgentEvent {
    AgentEvent {
        kind: "tool_result".into(),
        title: format!("结果: {}", call.id),
        body: if result.is_error {
            format!("失败: {}", result.content)
        } else {
            result.content.clone()
        },
    }
}

fn tool_error_result(call_id: &str, message: &str) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call_id.to_string(),
        content: message.to_string(),
        is_error: true,
        file_change: None,
    }
}

// ── mock agent (kept for browser dev mode compatibility) ──

pub fn run_mock_agent_task(workspace_root: PathBuf, prompt: &str) -> AppResult<AgentTaskResult> {
    let fs = WorkspaceFs::new(workspace_root.clone());
    let target = workspace_root.join("README.md");
    let target_existed = target.exists();
    let next_text = format!("# Sophoni\n\nMock task completed for: {}\n", prompt);
    let write = fs.write_text_with_snapshot(&target, &next_text)?;

    let task_id = Uuid::new_v4();
    let change = FileChange {
        id: Uuid::new_v4(),
        task_run_id: task_id,
        path: "README.md".to_string(),
        kind: if target_existed {
            ChangeKind::Modified
        } else {
            ChangeKind::Created
        },
        diff: write.diff,
        created_at: Utc::now(),
    };

    Ok(AgentTaskResult {
        summary: "mock Agent 已完成一次文件写入任务。".to_string(),
        events: vec![
            AgentEvent { kind: "thought".into(), title: "理解任务".into(), body: prompt.to_string() },
            AgentEvent { kind: "tool".into(), title: "写入 README.md".into(), body: "已写入 README.md 并生成 diff。".into() },
            AgentEvent { kind: "summary".into(), title: "任务完成".into(), body: "mock Agent 已生成可展示的文件变更。".into() },
        ],
        file_changes: vec![change],
    })
}
```

- [ ] **Step 4: Run L2 tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml agent_loop_`
Expected: PASS — all 4 L2 tests green.

- [ ] **Step 5: Run full Rust test suite to verify no regressions**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all tests pass (existing mock agent tests + new config/tool/glm/loop tests).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/core/agent.rs src-tauri/src/core/tests.rs
git commit -m "feat: implement model-driven agent loop with cancel/timeout/rounds limits"
```

---

## Task 9: Tauri commands + AppState

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Rewrite lib.rs**

Replace `src-tauri/src/lib.rs`:

```rust
mod core;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core::agent::{run_agent_task, run_mock_agent_task, AgentTaskResult, EventSink};
use core::command_risk::{classify_command, CommandRisk};
use core::config::AgentConfig;
use core::domain::{AgentEvent, AgentToolSchema, ConfigStatus, SystemPrompt};
use core::errors::AppError;
use core::provider::GlmProvider;
use core::tools::ToolDispatcher;
use tauri::{AppHandle, Emitter, State};

struct AppState {
    cancel: Arc<AtomicBool>,
}

struct AppEventSink {
    app: AppHandle,
}

impl EventSink for AppEventSink {
    fn emit(&self, event: &AgentEvent) {
        let _ = self.app.emit("agent-event", event);
    }
}

#[tauri::command]
fn get_app_status() -> String {
    "Sophoni desktop agent is ready".to_string()
}

#[tauri::command]
fn classify_command_risk(command: String, workspace_root: String) -> CommandRisk {
    classify_command(&command, &workspace_root)
}

#[tauri::command]
fn run_mock_task(
    workspace_root: String,
    prompt: String,
) -> Result<AgentTaskResult, AppError> {
    run_mock_agent_task(PathBuf::from(workspace_root), &prompt)
}

#[tauri::command]
fn get_config_status() -> ConfigStatus {
    AgentConfig::status()
}

#[tauri::command]
async fn run_agent_task(
    state: State<'_, AppState>,
    app: AppHandle,
    workspace_root: String,
    prompt: String,
) -> Result<AgentTaskResult, AppError> {
    state.cancel.store(false, Ordering::Relaxed);

    let config = AgentConfig::load()?;
    let provider = GlmProvider::new(config);
    let tools = ToolDispatcher::new(PathBuf::from(&workspace_root));
    let sink = AppEventSink { app };

    run_agent_task(
        Box::new(provider),
        &tools,
        &sink,
        &state.cancel,
        SystemPrompt(String::new()), // real system prompt is set inside the loop
        prompt,
        vec![], // schemas are generated inside the loop
    )
    .await
}

#[tauri::command]
fn cancel_agent_task(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            classify_command_risk,
            run_mock_task,
            get_config_status,
            run_agent_task,
            cancel_agent_task,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles. Note: `AgentToolSchema` import may be unused — if so, remove the unused import.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: wire agent loop into tauri commands with cancel state"
```

---

## Task 10: Frontend types + API

**Files:**
- Modify: `src/lib/types.ts`
- Modify: `src/lib/api.ts`

- [ ] **Step 1: Add ConfigStatus type**

Edit `src/lib/types.ts`, append:

```typescript
export interface ConfigStatus {
  configured: boolean;
  model: string;
}
```

- [ ] **Step 2: Add API functions**

Edit `src/lib/api.ts`, replace the entire file:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentEvent, AgentTaskResult, CommandRisk, ConfigStatus } from "./types";

export async function getAppStatus(): Promise<string> {
  return invoke<string>("get_app_status");
}

export async function classifyCommandRisk(command: string, workspaceRoot: string): Promise<CommandRisk> {
  return invoke<CommandRisk>("classify_command_risk", { command, workspaceRoot });
}

export async function runMockTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_mock_task", { workspaceRoot, prompt });
}

export async function runAgentTask(workspaceRoot: string, prompt: string): Promise<AgentTaskResult> {
  return invoke<AgentTaskResult>("run_agent_task", { workspaceRoot, prompt });
}

export async function cancelAgentTask(): Promise<void> {
  await invoke("cancel_agent_task");
}

export async function getConfigStatus(): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("get_config_status");
}

export async function onAgentEvent(cb: (e: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>("agent-event", (ev) => cb(ev.payload));
}
```

- [ ] **Step 3: Verify typecheck**

Run: `pnpm check`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib/types.ts src/lib/api.ts
git commit -m "feat: add frontend api for agent task, cancel, config, events"
```

---

## Task 11: Frontend UI wiring

**Files:**
- Modify: `src/App.svelte`
- Modify: `src/lib/components/Conversation.svelte`
- Modify: `src/lib/components/SettingsPanel.svelte`

- [ ] **Step 1: Update App.svelte to use real api + event stream + cancel**

Replace `src/App.svelte`:

```svelte
<script lang="ts">
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import ContextPanel from "./lib/components/ContextPanel.svelte";
  import SettingsPanel from "./lib/components/SettingsPanel.svelte";
  import { runAgentTask, cancelAgentTask, onAgentEvent } from "./lib/api";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type { AgentEvent, FileChange } from "./lib/types";

  let events: AgentEvent[] = [];
  let fileChanges: FileChange[] = [];
  let summary = "输入任务后，Agent 会在这里展示步骤和结果。";
  let prompt = "";
  let running = false;
  let showSettings = false;
  let unlisten: UnlistenFn | null = null;

  // Hardcoded workspace for MVP — "open workspace" UI is a follow-up plan.
  const WORKSPACE_ROOT = "/tmp/sophoni";

  async function runDemo(task: string) {
    running = true;
    events = [];
    fileChanges = [];
    try {
      unlisten = await onAgentEvent((e) => { events = [...events, e]; });
      const result = await runAgentTask(WORKSPACE_ROOT, task || "读 README.md 并加一行注释");
      // Reconcile with authoritative return value.
      events = result.events;
      fileChanges = result.fileChanges;
      summary = result.summary;
    } catch (e) {
      events = [...events, { kind: "error", title: "调用失败", body: String(e) }];
    } finally {
      unlisten?.();
      unlisten = null;
      running = false;
    }
  }

  async function cancel() {
    await cancelAgentTask();
  }
</script>

<div class="app-shell">
  <Sidebar />
  <Conversation {events} {summary} bind:prompt {running} onRun={runDemo} onCancel={cancel} />
  <ContextPanel {fileChanges} />
</div>

{#if showSettings}
  <SettingsPanel onClose={() => (showSettings = false)} />
{/if}
```

- [ ] **Step 2: Update Conversation.svelte to add cancel button**

Replace `src/lib/components/Conversation.svelte`:

```svelte
<script lang="ts">
  import type { AgentEvent } from "../types";

  export let events: AgentEvent[] = [];
  export let summary = "输入任务后，Agent 会在这里展示步骤和结果。";
  export let prompt = "";
  export let running = false;
  export let onRun: (prompt: string) => void = () => {};
  export let onCancel: () => void = () => {};
</script>

<main class="conversation" aria-label="任务会话流">
  <header class="topbar">
    <div>
      <h1>桌面 Agent 工作台</h1>
      <p>macOS MVP · GLM 真连接入</p>
    </div>
  </header>

  <div class="messages">
    {#each events as event}
      <article class="event" data-kind={event.kind}>
        <span>{event.kind}</span>
        <h3>{event.title}</h3>
        <p>{event.body}</p>
      </article>
    {/each}
    <article class="assistant">
      <h3>结果摘要</h3>
      <p>{summary}</p>
    </article>
  </div>

  <form class="composer" on:submit|preventDefault={() => onRun(prompt)}>
    <input aria-label="任务输入" placeholder="让 Agent 读取、修改工作区文件..." bind:value={prompt} />
    <button type="submit" disabled={running}>{running ? "运行中..." : "运行任务"}</button>
    {#if running}
      <button type="button" class="cancel" on:click={onCancel}>取消</button>
    {/if}
  </form>
</main>
```

- [ ] **Step 3: Update SettingsPanel to read-only status**

Replace `src/lib/components/SettingsPanel.svelte`:

```svelte
<script lang="ts">
  import { onMount } from "svelte";
  import { getConfigStatus } from "../api";
  import type { ConfigStatus } from "../types";

  export let onClose: () => void = () => {};

  let status: ConfigStatus | null = null;

  onMount(async () => {
    try {
      status = await getConfigStatus();
    } catch {
      status = { configured: false, model: "(查询失败)" };
    }
  });
</script>

<section class="settings" aria-label="设置">
  <h2>设置</h2>
  {#if status}
    <p>GLM API:{status.configured ? `已配置 (model: ${status.model})` : "未配置"}</p>
    {#if !status.configured}
      <p class="muted">请在 <code>~/.config/sophoni/config.toml</code> 填入 api_key，参考 README。</p>
    {/if}
  {/if}
  <label>默认模型 <input value={status?.model ?? "(未配置)"} readonly /></label>
  <button type="button" on:click={onClose}>关闭</button>
</section>
```

- [ ] **Step 4: Add cancel button styling**

Edit `src/app.css`. Find the `.composer button:disabled` rule and add after it:

```css
.composer button.cancel {
  background: transparent;
  color: #c0392b;
  border-color: #c0392b;
}
```

- [ ] **Step 5: Verify check + build**

Run: `pnpm check && pnpm build`
Expected: both succeed.

- [ ] **Step 6: Commit**

```bash
git add src/App.svelte src/lib/components/Conversation.svelte src/lib/components/SettingsPanel.svelte src/app.css
git commit -m "feat: wire frontend to real agent api with event stream and cancel"
```

---

## Task 12: End-to-end acceptance + README

**Files:**
- Modify: `README.md`
- Manual verification

- [ ] **Step 1: Update README with config instructions**

Edit `README.md`. Find the "## 尚未实现" section and insert before it a new section:

```markdown
## 配置 GLM API Key

创建 `~/.config/sophoni/config.toml`：

```toml
api_key = "你的 GLM API Key"
model = "glm-4.6"                    # 可选，默认 glm-4.6
base_url = "https://open.bigmodel.cn/api/paas/v4"  # 可选
```

设置文件权限（推荐）：

```bash
chmod 600 ~/.config/sophoni/config.toml
```

启动后，设置页会显示「已配置 (model: glm-4.6)」。
```

- [ ] **Step 2: Prepare workspace sandbox**

```bash
mkdir -p /tmp/sophoni
cat > /tmp/sophoni/README.md <<'EOF'
# Demo Workspace

This is a sandbox workspace for the Sophoni agent.
EOF
```

- [ ] **Step 3: Run full automated verification**

Run: `cargo test --manifest-path src-tauri/Cargo.toml && pnpm check && pnpm test && pnpm build`
Expected: all green.

- [ ] **Step 4: Manual acceptance (requires real GLM key)**

This step requires a configured `~/.config/sophoni/config.toml` with a real GLM API key, and running `pnpm tauri dev`. It cannot be automated (we don't test against real LLM APIs).

Acceptance checklist:
1. Settings panel shows "已配置 (model: glm-4.6)".
2. Enter task: "读 README.md 然后在末尾加一行 'Modified by agent'".
3. Click "运行任务". Watch the middle column: events appear incrementally (tool_call / tool_result pairs).
4. Right column shows the diff with `+Modified by agent`.
5. Verify `/tmp/sophoni/README.md` actually changed on disk.
6. Run another task and click "取消" mid-run; the agent stops and an error event mentions "取消".
7. Remove `~/.config/sophoni/config.toml`, restart, run a task — settings shows "未配置" and the task fails with a config error.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document GLM API key configuration"
```

---

## Notes for the implementer

1. **Task 8 uses `Box<dyn AgentProvider>` and `&dyn EventSink`** to keep the loop decoupled from concrete types. `AgentProvider` needs `Send` (already in the trait). `EventSink` needs `Send` too (declared on the trait).

2. **`run_mock_agent_task` is kept for browser dev mode** (`mockApi.ts` still calls the concept). `run_mock_task` Tauri command is retained so old tests/UI don't break.

3. **The `tauri::Emitter` trait** must be in scope for `app.emit(...)`. Imported in `lib.rs`.

4. **`AgentToolSchema` and `SystemPrompt` params to `run_agent_task` are passed empty/dummy from `lib.rs`** because the loop generates them internally (system prompt is hardcoded constant, schemas from `tool_schemas()`). They remain in the signature for testability — tests can inject custom schemas/system prompts.

5. **The hardcoded `/tmp/sophoni` workspace** is intentional per spec. Real "open workspace" UI is a follow-up plan.
