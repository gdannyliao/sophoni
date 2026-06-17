mod core;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core::agent::{run_agent_task as run_agent_task_inner, run_mock_agent_task, AgentTaskResult, EventSink};
use core::command_risk::{classify_command, CommandRisk, RiskLevel};
use core::domain::{AgentConfig, AgentEvent, ConfigStatus, Conversation, ConversationSummary, ConversationTurn, SystemPrompt};
use core::errors::AppError;
use core::provider::OpenAICompatibleProvider;
use core::storage::Storage;
use core::tools::{ConfirmHandler, ToolDispatcher};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{oneshot, Mutex};

struct AppState {
    cancel: Arc<AtomicBool>,
    confirm_pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    storage: Arc<Mutex<Storage>>,
}

struct AppEventSink {
    app: AppHandle,
}

impl EventSink for AppEventSink {
    fn emit(&self, event: &AgentEvent) {
        let _ = self.app.emit("agent-event", event);
    }
}

struct TauriConfirmHandler {
    app: AppHandle,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}

#[async_trait::async_trait]
impl ConfirmHandler for TauriConfirmHandler {
    async fn confirm(&self, command: &str, reason: &str) -> bool {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(request_id.clone(), tx);
        }
        let _ = self.app.emit(
            "command-confirm",
            serde_json::json!({
                "requestId": request_id,
                "command": command,
                "reason": reason,
            }),
        );
        match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
            Ok(Ok(allowed)) => allowed,
            _ => {
                let mut pending = self.pending.lock().await;
                pending.remove(&request_id);
                false
            }
        }
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
fn get_risk_level() -> Result<String, AppError> {
    let (config, _) = AgentConfig::load()?;
    Ok(match config.risk_level {
        RiskLevel::Standard => "standard".into(),
        RiskLevel::Relaxed => "relaxed".into(),
        RiskLevel::Unrestricted => "unrestricted".into(),
    })
}

#[tauri::command]
fn set_risk_level(level: String) -> Result<(), AppError> {
    let risk_level = match level.as_str() {
        "standard" => RiskLevel::Standard,
        "relaxed" => RiskLevel::Relaxed,
        "unrestricted" => RiskLevel::Unrestricted,
        _ => return Err(AppError::Config(format!("未知风险等级: {level}"))),
    };
    core::config::save_risk_level(risk_level)?;
    Ok(())
}

#[tauri::command]
fn get_workspace_path() -> Result<Option<String>, AppError> {
    let (config, _) = AgentConfig::load()?;
    Ok(config.workspace_path)
}

#[tauri::command]
fn set_workspace_path(path: String) -> Result<(), AppError> {
    core::config::save_workspace_path(&path)?;
    Ok(())
}

#[tauri::command]
async fn resolve_command_confirm(
    state: State<'_, AppState>,
    request_id: String,
    allowed: bool,
) -> Result<(), AppError> {
    let mut pending = state.confirm_pending.lock().await;
    if let Some(tx) = pending.remove(&request_id) {
        let _ = tx.send(allowed);
    }
    Ok(())
}

/// 解析当前会话：复用已有（返回历史 turns + 排除自身的记忆）或新建。
/// 返回 (conversation, is_new, history_turns, memory_context, existing_categories)。
fn resolve_conversation(
    storage: &Storage,
    workspace_id: &uuid::Uuid,
    existing_conversation_id: Option<&str>,
) -> Result<(Conversation, bool, Vec<ConversationTurn>, String, Vec<String>), AppError> {
    // 复用分支：existing_conversation_id 合法且会话存在
    if let Some(id_str) = existing_conversation_id {
        if let Ok(id) = uuid::Uuid::parse_str(id_str) {
            if let Ok(conv) = storage.get_conversation(&id) {
                // 历史 turns：续聊时拼进 provider，让模型看到同会话之前的完整对话
                let history_turns = storage.get_conversation_turns(&id).unwrap_or_default();
                // 跨会话记忆：排除当前会话自身，避免记忆自引用
                let memories = storage.list_conversation_memories(workspace_id, Some(&conv.id))?;
                let memory_context = core::agent::build_memory_context(&memories);
                let existing_categories: Vec<String> = memories
                    .iter()
                    .filter_map(|m| m.category.clone())
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                return Ok((conv, false, history_turns, memory_context, existing_categories));
            }
        }
    }
    // 新建分支：无 id / id 非法 / 会话不存在
    let conv = storage.create_conversation(workspace_id, &uuid::Uuid::new_v4().to_string())?;
    let memories = storage.list_conversation_memories(workspace_id, None)?;
    let memory_context = core::agent::build_memory_context(&memories);
    let existing_categories: Vec<String> = memories
        .iter()
        .filter_map(|m| m.category.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    Ok((conv, true, vec![], memory_context, existing_categories))
}

#[tauri::command]
async fn run_agent_task(
    state: State<'_, AppState>,
    app: AppHandle,
    prompt: String,
    existing_conversation_id: Option<String>,
) -> Result<AgentTaskResult, AppError> {
    state.cancel.store(false, Ordering::Relaxed);
    tracing::info!(%prompt, "ipc run_agent_task");

    let (config, _provider) = AgentConfig::load()?;
    let risk_level = config.risk_level;
    let search_config = config.search_config.clone();
    let (workspace, workspace_mode) = match &config.workspace_path {
        Some(path) => (path.clone(), core::tools::WorkspaceMode::Full),
        None => ("/tmp/sophoni-chat".to_string(), core::tools::WorkspaceMode::ChatOnly),
    };
    let provider = OpenAICompatibleProvider::new(config);

    let confirm_handler = Arc::new(TauriConfirmHandler {
        app: app.clone(),
        pending: state.confirm_pending.clone(),
    });
    let tools = ToolDispatcher::new(PathBuf::from(&workspace))
        .with_risk_level(risk_level)
        .with_confirm_handler(confirm_handler)
        .with_workspace_mode(workspace_mode);
    let mut tools = tools;
    if let Some(sc) = search_config {
        tools = tools.with_search_config(sc);
    }
    let sink = AppEventSink { app };

    // 在 async 外创建/复用 conversation + 读历史 turns 与记忆（Storage/Connection 不是 Send，不能跨 await）
    // resolve_conversation 返回 (conv, is_new, history_turns, memory_context, existing_categories)
    let (conversation, is_new, history_turns, memory_context, existing_categories) = {
        let home = dirs::home_dir().ok_or_else(|| AppError::Config("no HOME".into()))?;
        let db_path = home.join(".config/sophoni/sophoni.db");
        let storage = Storage::open(&db_path)?;
        let ws = storage.get_or_create_workspace(&workspace)?;
        resolve_conversation(&storage, &ws.id, existing_conversation_id.as_deref())?
    };

    let result = run_agent_task_inner(
        Box::new(provider),
        &tools,
        &sink,
        &state.cancel,
        SystemPrompt(String::new()),
        prompt,
        vec![],
        conversation.id,
        memory_context,
        existing_categories,
        history_turns,
    )
    .await?;

    // 任务结束后写 DB（同步，不跨 await）+ 解析 category
    {
        let home = dirs::home_dir().ok_or_else(|| AppError::Config("no HOME".into()))?;
        let db_path = home.join(".config/sophoni/sophoni.db");
        let storage = Storage::open(&db_path)?;

        // events：复用会话时合并历史 events + 本轮 events，保证前端能完整回放历史 UI；
        // 新会话直接写本轮 events。turns 始终写「历史 + 本轮」的完整累积。
        let final_events: Vec<AgentEvent> = if is_new {
            result.events.clone()
        } else {
            let history_events: Vec<AgentEvent> = storage
                .get_conversation(&conversation.id)
                .ok()
                .and_then(|c| serde_json::from_str(&c.events_json).ok())
                .unwrap_or_default();
            [history_events, result.events.clone()].concat()
        };
        let events_json = serde_json::to_string(&final_events).unwrap_or_else(|_| "[]".to_string());
        let _ = storage.update_conversation_events(&conversation.id, &events_json);

        // turns：本轮 run_agent_task 返回的是「历史 + 本轮」完整 turns，直接整体写入
        let turns_json = serde_json::to_string(&result.turns).unwrap_or_else(|_| "[]".to_string());
        let _ = storage.update_conversation_turns(&conversation.id, &turns_json);

        // 先去掉 <think> 标签，再解析 category
        let clean_text = core::agent::strip_think_tags(&result.summary);
        let (category, clean_summary) = core::agent::parse_category(&clean_text);
        // 新会话用本轮 summary 作标题；复用会话保留原标题（避免后续消息覆盖掉首条标题）
        if is_new {
            let title = if clean_summary.is_empty() {
                conversation.id.to_string()
            } else {
                clean_summary.clone()
            };
            let _ = storage.update_conversation_title(&conversation.id, &title);
        }
        if let Some(cat) = &category {
            let _ = storage.update_conversation_category(&conversation.id, cat);
        }
    }

    Ok(result)
}

#[tauri::command]
async fn list_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<ConversationSummary>, AppError> {
    let storage = state.storage.lock().await;
    let (config, _) = AgentConfig::load()?;
    let workspace = config
        .workspace_path
        .ok_or_else(|| AppError::Config("未选择工作区".into()))?;
    let ws = storage.get_or_create_workspace(&workspace)?;
    Ok(storage.list_conversations(&ws.id)?)
}

#[tauri::command]
async fn get_conversation(
    state: State<'_, AppState>,
    id: String,
) -> Result<Conversation, AppError> {
    let storage = state.storage.lock().await;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|e| AppError::Config(format!("无效会话 ID: {e}")))?;
    Ok(storage.get_conversation(&uuid)?)
}

#[tauri::command]
async fn delete_conversation(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    let storage = state.storage.lock().await;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|e| AppError::Config(format!("无效会话 ID: {e}")))?;
    storage.delete_conversation(&uuid)?;
    Ok(())
}

#[tauri::command]
fn cancel_agent_task(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化 tracing：默认 INFO 级别输出到 stderr，可用 RUST_LOG 环境变量覆盖
    tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let home = dirs::home_dir().expect("no HOME directory");
    let db_path = home.join(".config/sophoni/sophoni.db");
    let storage = Storage::open(&db_path).expect("failed to open DB");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_pending: Arc::new(Mutex::new(HashMap::new())),
            storage: Arc::new(Mutex::new(storage)),
        })
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            classify_command_risk,
            run_mock_task,
            get_config_status,
            run_agent_task,
            cancel_agent_task,
            get_risk_level,
            set_risk_level,
            resolve_command_confirm,
            get_workspace_path,
            set_workspace_path,
            list_conversations,
            get_conversation,
            delete_conversation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
