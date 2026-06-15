mod core;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core::agent::{run_agent_task as run_agent_task_inner, run_mock_agent_task, AgentTaskResult, EventSink};
use core::command_risk::{classify_command, CommandRisk};
use core::domain::{AgentConfig, AgentEvent, ConfigStatus, SystemPrompt};
use core::errors::AppError;
use core::provider::OpenAICompatibleProvider;
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
    let provider = OpenAICompatibleProvider::new(config);
    let tools = ToolDispatcher::new(PathBuf::from(&workspace_root));
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
