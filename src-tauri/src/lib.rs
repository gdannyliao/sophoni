mod core;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core::agent::{
    run_agent_task as run_agent_task_inner, run_mock_agent_task, AgentTaskResult, EventSink,
};
use core::command_risk::{classify_command, CommandRisk, RiskLevel};
use core::domain::{
    AgentConfig, AgentEvent, ConfigStatus, Conversation, ConversationSummary, ConversationTurn,
    SystemPrompt,
};
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
    server_state: core::server::ServerState,
    server_port: Arc<std::sync::atomic::AtomicU16>,
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
fn run_mock_task(workspace_root: String, prompt: String) -> Result<AgentTaskResult, AppError> {
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
) -> Result<
    (
        Conversation,
        bool,
        Vec<ConversationTurn>,
        String,
        Vec<String>,
    ),
    AppError,
> {
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
                return Ok((
                    conv,
                    false,
                    history_turns,
                    memory_context,
                    existing_categories,
                ));
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

/// 传输无关的 agent 任务核心逻辑。IPC（Tauri command）和 HTTP（axum handler）共用。
/// confirm_handler 决定高危命令确认去哪（IPC 的 TauriConfirmHandler / HTTP 的自动放行），
/// sink 决定事件去哪（IPC 的 AppEventSink / HTTP 的 SseEventSink）。
pub(crate) async fn run_agent_task_core(
    prompt: String,
    existing_conversation_id: Option<String>,
    cancel: Arc<AtomicBool>,
    confirm_handler: Arc<dyn ConfirmHandler>,
    sink: Arc<dyn EventSink>,
) -> Result<AgentTaskResult, AppError> {
    cancel.store(false, Ordering::Relaxed);
    tracing::info!(%prompt, "agent task started (core)");

    let (config, _provider) = AgentConfig::load()?;
    let risk_level = config.risk_level;
    let (workspace, workspace_mode) = match &config.workspace_path {
        Some(path) => (path.clone(), core::tools::WorkspaceMode::Full),
        None => (
            "/tmp/sophoni-chat".to_string(),
            core::tools::WorkspaceMode::ChatOnly,
        ),
    };
    let provider = OpenAICompatibleProvider::new(config);

    let tools = ToolDispatcher::new(PathBuf::from(&workspace))
        .with_risk_level(risk_level)
        .with_confirm_handler(confirm_handler)
        .with_workspace_mode(workspace_mode);

    // 在 async 外创建/复用 conversation + 读历史 turns 与记忆（Storage/Connection 不是 Send，不能跨 await）
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
        sink.as_ref(),
        &cancel,
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

        let turns_json = serde_json::to_string(&result.turns).unwrap_or_else(|_| "[]".to_string());
        let _ = storage.update_conversation_turns(&conversation.id, &turns_json);

        let clean_text = core::agent::strip_think_tags(&result.summary);
        let (category, clean_summary) = core::agent::parse_category(&clean_text);
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
async fn run_agent_task(
    state: State<'_, AppState>,
    app: AppHandle,
    prompt: String,
    existing_conversation_id: Option<String>,
) -> Result<AgentTaskResult, AppError> {
    tracing::info!(%prompt, "ipc run_agent_task");
    let confirm_handler = Arc::new(TauriConfirmHandler {
        app: app.clone(),
        pending: state.confirm_pending.clone(),
    }) as Arc<dyn ConfirmHandler>;
    let sink = Arc::new(AppEventSink { app }) as Arc<dyn EventSink>;
    run_agent_task_core(
        prompt,
        existing_conversation_id,
        state.cancel.clone(),
        confirm_handler,
        sink,
    )
    .await
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
    let uuid =
        uuid::Uuid::parse_str(&id).map_err(|e| AppError::Config(format!("无效会话 ID: {e}")))?;
    Ok(storage.get_conversation(&uuid)?)
}

#[tauri::command]
async fn delete_conversation(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    let storage = state.storage.lock().await;
    let uuid =
        uuid::Uuid::parse_str(&id).map_err(|e| AppError::Config(format!("无效会话 ID: {e}")))?;
    storage.delete_conversation(&uuid)?;
    Ok(())
}

#[tauri::command]
fn cancel_agent_task(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[derive(serde::Serialize)]
struct PairQrCode {
    url: String,
    svg: String,
    ip: String,
    port: u16,
    code: String,
}

/// 获取配对二维码（供桌面 UI「手机连接」面板展示）。
#[tauri::command]
async fn get_pair_qrcode(state: State<'_, AppState>) -> Result<PairQrCode, AppError> {
    let port = state.server_port.load(Ordering::Relaxed);
    let ip = core::server::qrcode::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".into());
    let code = state.server_state.current_pair_code().await;
    let url = core::server::qrcode::build_pair_url(&ip, port, &code);
    let svg = core::server::qrcode::render_qr_svg(&url);
    Ok(PairQrCode {
        url,
        svg,
        ip,
        port,
        code,
    })
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

    // 桌面端：SQLite + HTTP 服务层 + 全部 IPC command。
    // 移动端：瘦客户端壳，不初始化 SQLite/HTTP 服务（数据走 HTTP 到桌面端）。
    // 两条路径各自 run()，避免 Builder 泛型类型不匹配问题。
    #[cfg(not(mobile))]
    run_desktop();
    #[cfg(mobile)]
    run_mobile();
}

#[cfg(not(mobile))]
fn run_desktop() {
    let home = dirs::home_dir().expect("no HOME directory");
    let db_path = home.join(".config/sophoni/sophoni.db");
    let storage = Storage::open(&db_path).expect("failed to open DB");

    let server_state = core::server::ServerState::new();
    let server_port: Arc<std::sync::atomic::AtomicU16> =
        Arc::new(std::sync::atomic::AtomicU16::new(0));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_pending: Arc::new(Mutex::new(HashMap::new())),
            storage: Arc::new(Mutex::new(storage)),
            server_state: server_state.clone(),
            server_port: server_port.clone(),
        })
        .setup({
            let server_state = server_state.clone();
            let server_port = server_port.clone();
            move |_app| {
                let router = core::server::build_router(server_state);
                // 用 tauri::async_runtime::spawn（而非裸 tokio::spawn），
                // 因为 setup 闭包不在 async context，裸 tokio::spawn 会 panic（no reactor）。
                tauri::async_runtime::spawn(async move {
                    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
                        .await
                        .expect("server bind failed");
                    let port = listener.local_addr().expect("no local addr").port();
                    server_port.store(port, Ordering::Relaxed);
                    tracing::info!(port, "server: HTTP 服务启动");
                    axum::serve(listener, router).await.expect("server error");
                });
                Ok(())
            }
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
            get_pair_qrcode,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(mobile)]
fn run_mobile() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![get_app_status,])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
