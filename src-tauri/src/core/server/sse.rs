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
        // try_send 而非 blocking_send：emit 在 agent 循环的 async 上下文里被调用，
        // blocking_send 会 panic（Cannot block the current thread from within a runtime）。
        // channel 容量 1024，远大于实际事件频率（token 已被后端 30ms 批量合并），
        // 不会真丢。满载时 try_send 返回 Err，丢弃该事件（优于 panic）。
        let _ = self.tx.try_send(event.clone());
    }
}
