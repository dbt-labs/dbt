use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use dbt_schemas::schemas::ResolvedCloudConfig;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::AtomicBool};

use crate::{
    redis_config::RedisConfig, task_cache_noop::TaskCacheNoop, task_cache_redis::TaskCacheRedis,
};

/// This enum is used to track the status of a task, such as whether it has started,
#[derive(Debug, PartialEq)]
pub enum TaskState {
    NotStarted,
    InProgress,
    Completed,
    Stale,
}

// Heartbeat timing configuration
pub(crate) const INITIAL_DELAY: Duration = Duration::seconds(2); // 2 seconds
pub(crate) const BACKOFF_FACTOR: i64 = 2; // exponential
pub(crate) const MAX_DELAY: Duration = Duration::seconds(32); // max 32 seconds
pub(crate) const BUFFER_TIME: Duration = Duration::seconds(2); // 2 seconds

/// Create a task manager based on the provided URL.
pub async fn create_task_cache(
    url: &str,
    cloud_config: Option<&ResolvedCloudConfig>,
) -> Result<Arc<dyn TaskCache>, String> {
    let redis_config = RedisConfig::from_env();
    if url == "noop" {
        Ok(Arc::new(TaskCacheNoop::new()))
    } else if url.starts_with("redis://") {
        Ok(Arc::new(
            TaskCacheRedis::new_single(url, &redis_config.key_prefix, cloud_config).await?,
        ))
    } else if url == "redis-cluster" {
        Ok(Arc::new(
            TaskCacheRedis::new_cluster(
                &redis_config.uri().await?,
                redis_config.tls_check_hostnames,
                &redis_config.key_prefix,
                redis_config.tls_ca_data,
                cloud_config,
            )
            .await?,
        ))
    } else if url == "redis-single" {
        Ok(Arc::new(
            TaskCacheRedis::new_single(
                &redis_config.uri().await?,
                &redis_config.key_prefix,
                cloud_config,
            )
            .await?,
        ))
    } else {
        Err(format!("Unsupported task manager: {url}"))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct TaskValue {
    pub owner_id: String,
    pub timestamp: DateTime<Utc>, // Use DateTime<Utc> for precise UTC times
    pub value: Option<String>,    // Optional for cases like heartbeat
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct TaskInfo {
    pub upstreams: std::collections::HashMap<String, String>, // uid -> hash
}

impl TaskValue {
    /// Returns `Some(&self)` if the `owner_id` matches the expected owner, otherwise `None`.
    pub fn validate_owner(&self, expected_owner: &str) -> Option<&Self> {
        if self.owner_id == expected_owner {
            Some(self)
        } else {
            None
        }
    }
}

impl TaskValue {
    /// Create a new `TaskValue` with a specified timestamp.
    pub fn new(owner_id: String, timestamp: DateTime<Utc>, result: Option<String>) -> Self {
        Self {
            owner_id,
            timestamp,
            value: result,
        }
    }

    /// Create a new `TaskValue` with the current UTC timestamp.
    pub fn new_now(owner_id: String, result: Option<String>) -> Self {
        Self {
            owner_id,
            timestamp: Utc::now(),
            value: result,
        }
    }
}

pub type TaskValues = (
    Option<TaskValue>, // Start value
    Option<TaskValue>, // Heartbeat value
    Option<TaskValue>, // Stop value
);

#[async_trait]
pub trait TaskCache: Send + Sync {
    /// Check if the connection to the task manager is valid.
    async fn check_connection(&self) -> Result<(), String>;

    /// Atomically attempt to start the task if it hasn't already started.
    /// If the current value at the start key matches `the earlier read value`, it is overwritten.
    /// Returns true if we acquired the task, false if it's already owned by someone else.
    async fn try_start_task(
        &self,
        key: &str,
        new_value: &TaskValue,
        old_value: &Option<TaskValue>,
    ) -> Result<bool, String>;

    /// Stop the task and write the final result under the stop key.
    async fn stop_task(&self, key: &str, value: &TaskValue, info: &TaskInfo) -> Result<(), String>;

    /// Retrieve the current state and structured task values (start, heartbeat, stop).
    /// For Stale state, the stop value is the last completed stop value.
    async fn get_task_state(&self, key: &str) -> Result<(TaskState, TaskValues), String>;

    /// Start sending heartbeats for this task until `stop_signal` is set.
    async fn start_heartbeat(
        self: Arc<Self>,
        key: &str,
        value: &TaskValue,
        stop_signal: Arc<AtomicBool>,
    ) -> Result<(), String>;

    /// Stop sending heartbeats (if applicable).
    async fn stop_heartbeat(&self, stop_signal: Arc<AtomicBool>);

    /// Try to cleanup the task (delete start key), only if it still holds the earlier read value.
    async fn try_cleanup_task(
        &self,
        key: &str,
        old_value: &Option<TaskValue>,
    ) -> Result<(), String>;

    /// get the heartbeat value for the task.
    async fn get_heartbeat(&self, key: &str) -> Result<TaskValue, String>;

    /// get the heartbeat value for the task.
    async fn get_start(&self, key: &str) -> Result<TaskValue, String>;

    /// get the stop value for the task.
    async fn get_stop(&self, key: &str) -> Result<TaskValue, String>;

    /// Get the info data for a task (upstream dependencies and their hashes).
    async fn get_info(&self, key: &str) -> Result<Option<TaskInfo>, String>;

    /// Invalidate/delete all cache entries for a task.
    async fn invalidate_task(&self, key: &str) -> Result<(), String>;

    /// Clear all cache entries for this project/environment/account combination.
    /// This is primarily used for testing to ensure a clean state.
    async fn clear_all(&self) -> Result<(), String>;
}
