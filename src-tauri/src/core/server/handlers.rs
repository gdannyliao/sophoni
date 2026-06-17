use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::core::domain::{Conversation, ConversationSummary, Workspace};
use crate::core::errors::AppError;
use crate::core::storage::Storage;

use super::state::ServerState;

/// POST /pair — 配对，用配对码换 token。唯一不需鉴权的路由。
pub async fn pair(
    State(state): State<ServerState>,
    Json(payload): Json<PairRequest>,
) -> Result<Json<PairResponse>, StatusCode> {
    match state.try_pair(&payload.code).await {
        Some(token) => Ok(Json(PairResponse { token })),
        None => Err(StatusCode::FORBIDDEN),
    }
}

#[derive(Deserialize)]
pub struct PairRequest {
    pub code: String,
}

#[derive(serde::Serialize)]
pub struct PairResponse {
    pub token: String,
}

fn open_storage() -> Result<Storage, (StatusCode, String)> {
    let home = dirs::home_dir().ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "no HOME".into()))?;
    let db_path = home.join(".config/sophoni/sophoni.db");
    Storage::open(&db_path).map_err(map_err)
}

/// GET /conversations — 当前工作区的会话列表。
pub async fn list_conversations() -> Result<Json<Vec<ConversationSummary>>, (StatusCode, String)> {
    let storage = open_storage()?;
    let workspace = current_workspace_path()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "未选择工作区".into()))?;
    let ws = storage.get_or_create_workspace(&workspace).map_err(map_err)?;
    let list = storage.list_conversations(&ws.id).map_err(map_err)?;
    Ok(Json(list))
}

/// GET /conversations/:id — 会话详情（含完整 events_json + turns_json）。
pub async fn get_conversation(Path(id): Path<String>) -> Result<Json<Conversation>, (StatusCode, String)> {
    let storage = open_storage()?;
    let uuid = uuid::Uuid::parse_str(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("无效会话 ID: {e}")))?;
    let conv = storage.get_conversation(&uuid).map_err(map_err)?;
    Ok(Json(conv))
}

/// GET /workspaces — 桌面所有工作区列表。
pub async fn list_workspaces() -> Result<Json<Vec<Workspace>>, (StatusCode, String)> {
    let storage = open_storage()?;
    let list = storage.list_workspaces().map_err(map_err)?;
    Ok(Json(list))
}

/// GET /workspaces/active — 当前激活工作区路径。
pub async fn get_active_workspace() -> Json<ActiveWorkspace> {
    Json(ActiveWorkspace {
        path: current_workspace_path(),
    })
}

/// PUT /workspaces/active — 切换激活工作区。
pub async fn set_active_workspace(
    Json(req): Json<SetWorkspaceRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    crate::core::config::save_workspace_path(&req.path).map_err(map_err)?;
    Ok(StatusCode::OK)
}

#[derive(serde::Serialize)]
pub struct ActiveWorkspace {
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct SetWorkspaceRequest {
    pub path: String,
}

/// GET /config/risk-level — 当前风险等级。
pub async fn get_risk_level() -> Result<Json<String>, (StatusCode, String)> {
    let (config, _) = crate::core::domain::AgentConfig::load().map_err(map_err)?;
    Ok(Json(match config.risk_level {
        crate::core::command_risk::RiskLevel::Standard => "standard".into(),
        crate::core::command_risk::RiskLevel::Relaxed => "relaxed".into(),
        crate::core::command_risk::RiskLevel::Unrestricted => "unrestricted".into(),
    }))
}

/// GET /files/:path — 读工作区内文件内容（供 ReviewView）。
pub async fn read_file(Path(path): Path<String>) -> Result<String, (StatusCode, String)> {
    let workspace = current_workspace_path()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "未选择工作区".into()))?;
    let full = std::path::PathBuf::from(&workspace).join(&path);
    // 路径逃逸检查：解析后必须在 workspace 内
    let canon = full.canonicalize().map_err(|e| (StatusCode::NOT_FOUND, format!("文件不存在: {e}")))?;
    let ws_canon = std::path::PathBuf::from(&workspace)
        .canonicalize()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("工作区无效: {e}")))?;
    if !canon.starts_with(&ws_canon) {
        return Err((StatusCode::FORBIDDEN, "路径越界工作区".into()));
    }
    std::fs::read_to_string(&canon).map_err(|e| (StatusCode::NOT_FOUND, format!("读取失败: {e}")))
}

fn current_workspace_path() -> Option<String> {
    crate::core::domain::AgentConfig::load().ok().and_then(|(c, _)| c.workspace_path)
}

fn map_err(e: AppError) -> (StatusCode, String) {
    tracing::error!(error = %e, "server handler error");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
