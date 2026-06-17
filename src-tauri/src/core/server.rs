// 注意：这是 server 模块的入口文件（非目录）。所有子模块在同目录的 server/ 下。
// 由于 core/mod.rs 已有 #![allow(dead_code)]，本模块暂未接路由的代码不会告警。

pub mod auth;
pub mod handlers;
pub mod qrcode;
pub mod sse;
pub mod state;

pub use state::ServerState;
