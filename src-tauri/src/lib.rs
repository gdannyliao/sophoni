mod core;

use std::path::PathBuf;

use core::agent::{run_mock_agent_task, AgentTaskResult};
use core::command_risk::{classify_command, CommandRisk};

#[tauri::command]
fn get_app_status() -> String {
    "Sophoni desktop agent foundation is ready".to_string()
}

#[tauri::command]
fn classify_command_risk(command: String, workspace_root: String) -> CommandRisk {
    classify_command(&command, &workspace_root)
}

#[tauri::command]
fn run_mock_task(
    workspace_root: String,
    prompt: String,
) -> Result<AgentTaskResult, core::errors::AppError> {
    run_mock_agent_task(PathBuf::from(workspace_root), &prompt)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            classify_command_risk,
            run_mock_task
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
