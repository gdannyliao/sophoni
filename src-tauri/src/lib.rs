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
    ScheduledTask, SystemPrompt,
};
use core::errors::AppError;
use core::provider::OpenAICompatibleProvider;
use core::storage::Storage;
use core::tools::ConfirmHandler;
use core::tool_spec::{build_tool_registry, ToolRegistry};
use core::workspace::WorkspaceFs;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{oneshot, Mutex};

/// sophoni 的 SQLite 数据库路径（~/.config/sophoni/sophoni.db）。
/// 集中定义，消灭此前 3 处硬编码重复。
fn db_path() -> Result<std::path::PathBuf, AppError> {
    let home = dirs::home_dir().ok_or_else(|| AppError::Config("no HOME".into()))?;
    Ok(home.join(".config/sophoni/sophoni.db"))
}

struct AppState {
    cancel: Arc<AtomicBool>,
    confirm_pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    storage: Arc<Mutex<Storage>>,
    server_state: core::server::ServerState,
    server_port: Arc<std::sync::atomic::AtomicU16>,
    scheduler: Arc<Mutex<Option<Arc<core::scheduler::Scheduler>>>>,
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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchConfigPayload {
    tavily_key: Option<String>,
    google_key: Option<String>,
    google_cx: Option<String>,
}

#[tauri::command]
fn get_search_config() -> Result<SearchConfigPayload, AppError> {
    let (config, _) = AgentConfig::load().unwrap_or_else(|_| {
        (
            AgentConfig {
                api_key: String::new(),
                model: String::new(),
                base_url: String::new(),
                risk_level: core::command_risk::RiskLevel::Standard,
                workspace_path: None,
                search_config: None,
            },
            String::new(),
        )
    });
    Ok(SearchConfigPayload {
        tavily_key: config
            .search_config
            .as_ref()
            .and_then(|c| c.tavily_key.clone()),
        google_key: config
            .search_config
            .as_ref()
            .and_then(|c| c.google_key.clone()),
        google_cx: config
            .search_config
            .as_ref()
            .and_then(|c| c.google_cx.clone()),
    })
}

#[tauri::command]
fn save_search_config(
    tavily_key: Option<String>,
    google_key: Option<String>,
    google_cx: Option<String>,
) -> Result<(), AppError> {
    core::config::save_search_config(tavily_key, google_key, google_cx)?;
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
    let search_config = config.search_config.clone();
    let (workspace, workspace_mode) = match &config.workspace_path {
        Some(path) => (path.clone(), core::tools::WorkspaceMode::Full),
        None => (
            "/tmp/sophoni-chat".to_string(),
            core::tools::WorkspaceMode::ChatOnly,
        ),
    };
    let fs = WorkspaceFs::new(PathBuf::from(&workspace));
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build http client");
    let registry: ToolRegistry = build_tool_registry(
        fs,
        risk_level,
        Some(confirm_handler),
        search_config,
        http_client,
    );
    let registry = Arc::new(registry);
    let provider = OpenAICompatibleProvider::new(config, registry.clone());

    // 在 async 外创建/复用 conversation + 读历史 turns 与记忆（Storage/Connection 不是 Send，不能跨 await）
    let (conversation, is_new, history_turns, memory_context, existing_categories) = {
        let storage = Storage::open(&db_path()?)?;
        let ws = storage.get_or_create_workspace(&workspace)?;
        resolve_conversation(&storage, &ws.id, existing_conversation_id.as_deref())?
    };

    let result = run_agent_task_inner(
        Box::new(provider),
        &registry,
        risk_level,
        workspace_mode,
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

    persist_task_result(&conversation.id, is_new, &result)?;
    Ok(result)
}

/// 任务结束后写 DB：合并 events、写 turns、解析 category、更新标题。
/// 抽成独立函数让编排逻辑可测（不依赖 Tauri runtime）。
fn persist_task_result(
    conversation_id: &uuid::Uuid,
    is_new: bool,
    result: &AgentTaskResult,
) -> Result<(), AppError> {
    let storage = Storage::open(&db_path()?)?;

    // events：复用会话时合并历史 events + 本轮 events；新会话直接写本轮
    let final_events: Vec<AgentEvent> = if is_new {
        result.events.clone()
    } else {
        let history_events: Vec<AgentEvent> = storage
            .get_conversation(conversation_id)
            .ok()
            .and_then(|c| serde_json::from_str(&c.events_json).ok())
            .unwrap_or_default();
        [history_events, result.events.clone()].concat()
    };
    let events_json = serde_json::to_string(&final_events).unwrap_or_else(|_| "[]".to_string());
    let _ = storage.update_conversation_events(conversation_id, &events_json);

    // turns：本轮返回的是「历史 + 本轮」完整 turns，整体写入
    let turns_json = serde_json::to_string(&result.turns).unwrap_or_else(|_| "[]".to_string());
    let _ = storage.update_conversation_turns(conversation_id, &turns_json);

    // category + 标题（仅新会话更新标题）
    let clean_text = core::agent::strip_think_tags(&result.summary);
    let (category, clean_summary) = core::agent::parse_category(&clean_text);
    if is_new {
        let title = if clean_summary.is_empty() {
            conversation_id.to_string()
        } else {
            clean_summary.clone()
        };
        let _ = storage.update_conversation_title(conversation_id, &title);
    }
    if let Some(cat) = &category {
        let _ = storage.update_conversation_category(conversation_id, cat);
    }
    Ok(())
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

// ── 定时任务 IPC 命令（供前端 SchedulePanel 管理 UI 用）──

#[tauri::command]
async fn list_scheduled_tasks(state: State<'_, AppState>) -> Result<Vec<ScheduledTask>, AppError> {
    let storage = state.storage.lock().await;
    Ok(storage.list_scheduled_tasks()?)
}

#[tauri::command]
async fn update_scheduled_task(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), AppError> {
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|e| AppError::Config(format!("无效 ID: {e}")))?;
    {
        let storage = state.storage.lock().await;
        storage.update_scheduled_task_enabled(&uuid, enabled)?;
    }
    // 通知 scheduler reload
    let scheduler = state.scheduler.lock().await;
    if let Some(sched) = scheduler.as_ref() {
        let sched = sched.clone();
        drop(scheduler);
        sched.reload_task(uuid).await;
    }
    Ok(())
}

#[tauri::command]
async fn delete_scheduled_task_cmd(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|e| AppError::Config(format!("无效 ID: {e}")))?;
    {
        let storage = state.storage.lock().await;
        storage.delete_scheduled_task(&uuid)?;
    }
    // 通知 scheduler stop
    let scheduler = state.scheduler.lock().await;
    if let Some(sched) = scheduler.as_ref() {
        let sched = sched.clone();
        drop(scheduler);
        sched.stop_task(uuid).await;
    }
    Ok(())
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
    let db_path_val = db_path().expect("no HOME directory");
    let storage = Storage::open(&db_path_val).expect("failed to open DB");

    let server_state = core::server::ServerState::new();
    let server_port: Arc<std::sync::atomic::AtomicU16> =
        Arc::new(std::sync::atomic::AtomicU16::new(0));

    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_pending: Arc::new(Mutex::new(HashMap::new())),
            storage: Arc::new(Mutex::new(storage)),
            server_state: server_state.clone(),
            server_port: server_port.clone(),
            scheduler: Arc::new(Mutex::new(None)),
        })
        .setup({
            let server_state = server_state.clone();
            let server_port = server_port.clone();
            let db_path_for_scheduler = db_path().expect("no HOME directory");
            move |app| {
                // 构造 Scheduler（需要 AppHandle 做 fire 回调）
                let app_handle = app.handle().clone();
                let cancel = app_handle.state::<AppState>().inner().cancel.clone();
                let fire_cancel = cancel.clone();
                let fire_app = app_handle.clone();
                let fire: core::scheduler::FireFn = Arc::new(move |prompt: String| {
                    let cancel = fire_cancel.clone();
                    let app = fire_app.clone();
                    Box::pin(async move {
                        let confirm: Arc<dyn core::tools::ConfirmHandler> =
                            Arc::new(core::scheduler::AutoRejectConfirmHandler);
                        let sink: Arc<dyn EventSink> =
                            Arc::new(AppEventSink { app });
                        run_agent_task_core(prompt, None, cancel, confirm, sink)
                            .await
                            .map(|_| ())
                    })
                });
                let scheduler = Arc::new(core::scheduler::Scheduler::new(
                    fire,
                    db_path_for_scheduler.clone(),
                ));

                // 注入通知通道（让定时任务工具能 reload/stop）
                let sched_for_reload = scheduler.clone();
                let sched_for_stop = scheduler.clone();
                core::tool_spec::init_scheduled_task_channels(
                    Arc::new(move |task_id| {
                        let s = sched_for_reload.clone();
                        tauri::async_runtime::spawn(async move { s.reload_task(task_id).await; });
                    }),
                    Arc::new(move |task_id| {
                        let s = sched_for_stop.clone();
                        tauri::async_runtime::spawn(async move { s.stop_task(task_id).await; });
                    }),
                    db_path_for_scheduler.clone(),
                );

                // 启动调度
                let sched_to_start = scheduler.clone();
                tauri::async_runtime::spawn(async move {
                    sched_to_start.start().await;
                });

                // 存入 AppState
                {
                    let state = app_handle.state::<AppState>();
                    let mut s = state.scheduler.blocking_lock();
                    *s = Some(scheduler);
                }

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
            get_search_config,
            save_search_config,
            list_conversations,
            get_conversation,
            delete_conversation,
            get_pair_qrcode,
            list_scheduled_tasks,
            update_scheduled_task,
            delete_scheduled_task_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(mobile)]
fn run_mobile() {
    tauri::Builder::default()
        .plugin(tauri_plugin_barcode_scanner::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![get_app_status,])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
