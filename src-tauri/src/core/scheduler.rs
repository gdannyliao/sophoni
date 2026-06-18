//! 定时任务调度引擎。应用启动时加载 enabled 任务，每个任务一个 tokio task，
//! 到点触发。高危命令自动拒绝（无人值守）。
//!
//! scheduler 不直接调 run_agent_task_core（在 lib.rs，避免循环依赖），
//! 而是通过 fire 回调，由 lib.rs 注入实际逻辑。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::domain::ScheduledTask;
use super::errors::AppResult;
use super::storage::Storage;
use super::tools::ConfirmHandler;

/// 自动拒绝高危命令的 handler（定时任务无人值守）。
pub struct AutoRejectConfirmHandler;

#[async_trait::async_trait]
impl ConfirmHandler for AutoRejectConfirmHandler {
    async fn confirm(&self, _command: &str, _reason: &str) -> bool {
        false
    }
}

/// fire 回调类型：接收 prompt，返回异步结果。
/// 由 lib.rs 注入，内部调 run_agent_task_core。
pub type FireFn = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<()>> + Send>>
        + Send
        + Sync,
>;

/// 计算下一次触发时间距现在的秒数（今天没到 = 今天，已过 = 明天）。
pub fn seconds_until(hour: u32, minute: u32) -> u64 {
    let now = chrono::Local::now();
    let today_target = now
        .date_naive()
        .and_hms_opt(hour, minute, 0)
        .unwrap();
    let now_naive = now.naive_local();
    let target = if today_target <= now_naive {
        let tomorrow = now_naive + chrono::Duration::days(1);
        tomorrow
            .date()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    } else {
        today_target
    };
    (target - now_naive).num_seconds().max(0) as u64
}

pub struct Scheduler {
    /// 每个任务一个 tokio task 的 JoinHandle，key = task id
    handles: Arc<Mutex<HashMap<uuid::Uuid, JoinHandle<()>>>>,
    /// fire 回调（由 lib.rs 注入）
    fire: FireFn,
    /// DB 路径（每次 open，避免持有 Connection 跨 await）
    db_path: std::path::PathBuf,
}

impl Scheduler {
    pub fn new(fire: FireFn, db_path: std::path::PathBuf) -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            fire,
            db_path,
        }
    }

    /// 应用启动时调用：加载所有 enabled 任务，为每个启动定时循环。
    pub async fn start(&self) {
        let tasks = {
            let storage = match Storage::open(&self.db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "scheduler: failed to open DB on start");
                    return;
                }
            };
            storage.list_scheduled_tasks().unwrap_or_default()
        };
        for task in tasks.iter().filter(|t| t.enabled) {
            self.spawn_task(task.clone()).await;
        }
        tracing::info!("scheduler: started with {} enabled tasks", tasks.iter().filter(|t| t.enabled).count());
    }

    /// 为一个任务启动定时循环 tokio task。
    async fn spawn_task(&self, task: ScheduledTask) {
        let task_id = task.id;
        let task_prompt = task.prompt.clone();
        let task_hour = task.hour;
        let task_minute = task.minute;
        let fire = self.fire.clone();
        let db_path = self.db_path.clone();
        let handles = self.handles.clone();

        let handle = tokio::spawn(async move {
            loop {
                let delay_secs = seconds_until(task_hour, task_minute);
                tracing::info!(
                    task_id = %task_id,
                    delay_secs,
                    "scheduler: waiting for next trigger"
                );

                tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

                tracing::info!(task_id = %task_id, prompt = %task_prompt, "scheduler: firing");
                let fire_result = (fire)(task_prompt.clone()).await;
                if let Err(e) = fire_result {
                    tracing::error!(task_id = %task_id, error = %e, "scheduler: fire failed");
                }

                // 更新 last_run_at
                if let Ok(storage) = Storage::open(&db_path) {
                    let _ = storage.update_task_last_run(&task_id, &chrono::Utc::now().to_rfc3339());
                }
            }
        });

        let mut h = handles.lock().await;
        h.insert(task_id, handle);
    }

    /// create/update 后调用：停掉旧 tokio task，如果任务仍 enabled 则重新 spawn。
    pub async fn reload_task(&self, task_id: uuid::Uuid) {
        // 停掉旧的
        let old_handle = {
            let mut h = self.handles.lock().await;
            h.remove(&task_id)
        };
        if let Some(handle) = old_handle {
            handle.abort();
        }
        // 从 DB 重新加载，如果 enabled 则重新 spawn
        let task = {
            let storage = match Storage::open(&self.db_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(%task_id, error = %e, "scheduler: reload failed to open DB");
                    return;
                }
            };
            storage
                .list_scheduled_tasks()
                .unwrap_or_default()
                .into_iter()
                .find(|t| t.id == task_id)
        };
        if let Some(task) = task.filter(|t| t.enabled) {
            self.spawn_task(task).await;
            tracing::info!(%task_id, "scheduler: reloaded task");
        } else {
            tracing::info!(%task_id, "scheduler: task disabled or deleted, not respawned");
        }
    }

    /// delete 后调用：停掉对应 tokio task。
    pub async fn stop_task(&self, task_id: uuid::Uuid) {
        let handle = {
            let mut h = self.handles.lock().await;
            h.remove(&task_id)
        };
        if let Some(handle) = handle {
            handle.abort();
            tracing::info!(%task_id, "scheduler: stopped task");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn seconds_until_future_time_within_24h() {
        // 目标设为当前小时 + 1，应该返回大约 1 小时后的秒数
        let now = chrono::Local::now();
        let target_hour = (now.hour() + 1) % 24;
        let secs = seconds_until(target_hour, 0);
        // 结果应在 0 到 24 小时之间（0-86400 秒）
        assert!(secs < 86400, "future time should be within 24h, got {secs}");
    }

    #[test]
    fn seconds_until_past_time_wraps_to_tomorrow() {
        // 目标设为 1 小时前（已过），应该返回大约 23 小时后的秒数
        let now = chrono::Local::now();
        let past_hour = if now.hour() == 0 { 23 } else { now.hour() - 1 };
        let secs = seconds_until(past_hour, now.minute());
        // 已过的时间点应该返回 > 12 小时（说明是明天的）
        assert!(secs > 43200, "past time should wrap to tomorrow (>12h), got {secs}");
    }

    #[test]
    fn auto_reject_handler_returns_false() {
        let handler = AutoRejectConfirmHandler;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(handler.confirm("rm -rf /", "高风险"));
        assert!(!result);
    }
}
