mod core;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core::agent::{run_agent_task as run_agent_task_inner, run_mock_agent_task, AgentTaskResult, EventSink};
use core::command_risk::{classify_command, CommandRisk, RiskLevel};
use core::domain::{AgentConfig, AgentEvent, ConfigStatus, SystemPrompt};
use core::errors::AppError;
use core::provider::OpenAICompatibleProvider;
use core::tools::{ConfirmHandler, ToolDispatcher};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{oneshot, Mutex};

struct AppState {
    cancel: Arc<AtomicBool>,
    confirm_pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
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

#[tauri::command]
async fn run_agent_task(
    state: State<'_, AppState>,
    app: AppHandle,
    workspace_root: String,
    prompt: String,
) -> Result<AgentTaskResult, AppError> {
    state.cancel.store(false, Ordering::Relaxed);

    let (config, _provider) = AgentConfig::load()?;
    let risk_level = config.risk_level;
    let provider = OpenAICompatibleProvider::new(config);

    let confirm_handler = Arc::new(TauriConfirmHandler {
        app: app.clone(),
        pending: state.confirm_pending.clone(),
    });
    let tools = ToolDispatcher::new(PathBuf::from(&workspace_root))
        .with_risk_level(risk_level)
        .with_confirm_handler(confirm_handler);
    let sink = AppEventSink { app };

    run_agent_task_inner(
        Box::new(provider),
        &tools,
        &sink,
        &state.cancel,
        SystemPrompt(String::new()),
        prompt,
        vec![],
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
            confirm_pending: Arc::new(Mutex::new(HashMap::new())),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
