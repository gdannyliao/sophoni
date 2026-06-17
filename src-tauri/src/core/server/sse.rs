use tokio::sync::mpsc;

use crate::core::agent::EventSink;
use crate::core::domain::AgentEvent;

/// 把 AgentEvent 通过 channel 推给 SSE 响应流。实现 EventSink trait，
/// 让 agent 循环无感知——IPC 走 AppEventSink（emit Tauri 事件），
/// HTTP 走 SseEventSink（推到 channel，由 /chat handler 转成 SSE 帧）。
pub struct SseEventSink {
    pub tx: mpsc::Sender<AgentEvent>,
}

impl EventSink for SseEventSink {
    fn emit(&self, event: &AgentEvent) {
        // blocking_send：绝不丢事件（token 丢了会导致手机端文本不完整）。
        // agent 循环串行，channel 容量 256，实际不会长时间阻塞。
        // 注意：EventSink::emit 是同步方法，blocking_send 在同步上下文安全。
        let _ = self.tx.blocking_send(event.clone());
    }
}
