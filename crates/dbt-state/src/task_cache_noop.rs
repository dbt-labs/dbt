use crate::task_cache::{TaskCache, TaskInfo, TaskState, TaskValue, TaskValues};
use async_trait::async_trait;
use std::sync::{Arc, atomic::AtomicBool};

pub struct TaskCacheNoop;

impl TaskCacheNoop {
    pub fn new() -> Self {
        Self
    }
}
impl Default for TaskCacheNoop {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskCache for TaskCacheNoop {
    async fn check_connection(&self) -> Result<(), String> {
        Ok(()) // No-op
    }

    async fn try_start_task(
        &self,
        _key: &str,
        _new_value: &TaskValue,
        _old_value: &Option<TaskValue>,
    ) -> Result<bool, String> {
        Ok(true) // Always allows the task to start
    }

    async fn stop_task(
        &self,
        _key: &str,
        _value: &TaskValue,
        _info: &TaskInfo,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn get_task_state(&self, _task_id: &str) -> Result<(TaskState, TaskValues), String> {
        Ok((TaskState::NotStarted, (None, None, None)))
    }

    async fn start_heartbeat(
        self: Arc<Self>,
        _key: &str,
        _value: &TaskValue,
        _stop_signal: Arc<AtomicBool>,
    ) -> Result<(), String> {
        Ok(()) // No-op
    }

    async fn stop_heartbeat(&self, _stop_signal: Arc<AtomicBool>) {
        // No-op
    }
    async fn try_cleanup_task(
        &self,
        _key: &str,
        _old_value: &Option<TaskValue>,
    ) -> Result<(), String> {
        // No-op
        Ok(())
    }
    /// get the heartbeat value for the task.
    async fn get_start(&self, _key: &str) -> Result<TaskValue, String> {
        // todo: or is better to return an error?
        Ok(TaskValue::new_now("".to_string(), None))
    }
    async fn get_stop(&self, _key: &str) -> Result<TaskValue, String> {
        Ok(TaskValue::new_now("".to_string(), None))
    }
    async fn get_heartbeat(&self, _key: &str) -> Result<TaskValue, String> {
        Ok(TaskValue::new_now("".to_string(), None))
    }

    async fn get_info(&self, _key: &str) -> Result<Option<TaskInfo>, String> {
        Ok(None) // No-op
    }

    async fn invalidate_task(&self, _key: &str) -> Result<(), String> {
        Ok(()) // No-op
    }

    async fn clear_all(&self) -> Result<(), String> {
        Ok(()) // No-op
    }
}
