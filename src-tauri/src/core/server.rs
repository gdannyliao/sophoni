// 注意：这是 server 模块的入口文件（非目录）。所有子模块在同目录的 server/ 下。
// 由于 core/mod.rs 已有 #![allow(dead_code)]，本模块暂未接路由的代码不会告警。

pub mod auth;
pub mod handlers;
pub mod qrcode;
pub mod sse;
pub mod state;

pub use state::ServerState;

use axum::{
    middleware,
    routing::{get, post, put},
    Router,
};
use tower_http::cors::CorsLayer;

use auth::require_token;
use handlers::*;

/// 组装完整路由。/pair 不需鉴权，其余走 require_token 中间件。
pub fn build_router(state: ServerState) -> Router {
    let public = Router::new().route("/pair", post(pair));

    let protected = Router::new()
        .route("/conversations", get(list_conversations))
        .route("/conversations/:id", get(get_conversation))
        .route("/chat", post(chat))
        .route("/workspaces", get(list_workspaces))
        .route(
            "/workspaces/active",
            get(get_active_workspace).put(set_active_workspace),
        )
        .route("/config/risk-level", get(get_risk_level))
        .route("/files/:path", get(read_file))
        .layer(middleware::from_fn_with_state(state.clone(), require_token));

    Router::new()
        .merge(public)
        .merge(protected)
        .layer(CorsLayer::permissive()) // 局域网内，允许手机 webview 跨域
        .with_state(state)
}

