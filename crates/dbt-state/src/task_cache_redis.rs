use async_trait::async_trait;
use chrono::{Duration, Utc};
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::{Client, TlsCertificates};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;
use tokio::time::Duration as TokioDuration;

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::{emit_info_log_message, emit_warn_log_message};
use dbt_schemas::schemas::ResolvedCloudConfig;

use crate::task_cache::{
    BACKOFF_FACTOR, BUFFER_TIME, INITIAL_DELAY, MAX_DELAY, TaskCache, TaskState, TaskValue,
    TaskValues,
};

const EXPIRATION_TIME: i64 = 30 * 24 * 60 * 60; // 30 days in seconds

pub struct TaskCacheRedis<T: ConnectionLike> {
    connection: Mutex<T>,
    project_id: String,
    environment_id: String,
    account_id: String,
    key_prefix: String,
}

impl TaskCacheRedis<MultiplexedConnection> {
    /// Get a TaskCacheRedis instance for a non-cluster redis
    pub async fn new_single(
        uri: &str,
        key_prefix: &str,
        cloud_config: Option<&ResolvedCloudConfig>,
    ) -> Result<Self, String> {
        let client =
            Client::open(uri).map_err(|e| format!("Failed to create Redis client: {e}"))?;
        let connection = client
            .get_multiplexed_tokio_connection()
            .await
            .map_err(|e| format!("Failed to get Redis connection: {e}"))?;

        let project_id = cloud_config
            .and_then(|c| c.project_id.clone())
            .ok_or_else(|| {
                String::from("cloud config project_id is required for redis task cache")
            })?;
        let environment_id = cloud_config
            .and_then(|c| c.environment_id.clone())
            .ok_or_else(|| {
                String::from("cloud config environment_id is required for redis task cache")
            })?;
        let account_id = cloud_config
            .and_then(|c| c.account_identifier.clone())
            .ok_or_else(|| {
                String::from("cloud config account_identifier is required for redis task cache")
            })?;

        Ok(Self {
            connection: Mutex::new(connection),
            project_id,
            environment_id,
            account_id,
            key_prefix: key_prefix.to_string(),
        })
    }
}

impl TaskCacheRedis<ClusterConnection> {
    /// Get a TaskCacheRedis instance for a redis cluster
    pub async fn new_cluster(
        uri: &str,
        tls_check_hostnames: bool,
        key_prefix: &str,
        tls_ca_data: Option<String>,
        cloud_config: Option<&ResolvedCloudConfig>,
    ) -> Result<Self, String> {
        let mut builder = ClusterClient::builder(vec![uri.to_string()])
            .danger_accept_invalid_hostnames(!tls_check_hostnames);
        if let Some(ca_data) = &tls_ca_data {
            builder = builder.certs(TlsCertificates {
                root_cert: Some(ca_data.as_bytes().to_vec()),
                client_tls: None,
            });
        }
        let client = builder
            .build()
            .map_err(|e| format!("Failed to create Redis client: {e}"))?;
        let connection = client
            .get_async_connection()
            .await
            .map_err(|e| format!("Failed to get Redis connection: {e}"))?;

        let project_id = cloud_config
            .and_then(|c| c.project_id.clone())
            .ok_or_else(|| {
                String::from("cloud config project_id is required for redis task cache")
            })?;
        let environment_id = cloud_config
            .and_then(|c| c.environment_id.clone())
            .ok_or_else(|| {
                String::from("cloud config environment_id is required for redis task cache")
            })?;
        let account_id = cloud_config
            .and_then(|c| c.account_identifier.clone())
            .ok_or_else(|| {
                String::from("cloud config account_identifier is required for redis task cache")
            })?;

        Ok(Self {
            connection: Mutex::new(connection),
            project_id,
            environment_id,
            account_id,
            key_prefix: key_prefix.to_string(),
        })
    }
}

impl<T: ConnectionLike> TaskCacheRedis<T> {
    /// Stamps `value.timestamp` after acquiring the connection, then writes
    /// it. The deadline we promise must reflect when the write actually
    /// goes out, not how long we waited behind other traffic on the
    /// connection this is shared with every waiter's poll.
    ///
    /// The write is conditioned on the start key still holding
    /// `expected_start_json`: returns `Ok(false)` instead of writing if some
    /// other owner has since reclaimed the task, so a worker evicted as
    /// stale can't keep clobbering the new owner's heartbeat with its own.
    async fn set_heartbeat_value(
        &self,
        task_id: &str,
        value: &mut TaskValue,
        delay: Duration,
        expected_start_json: &str,
    ) -> Result<bool, String> {
        let mut con = self.connection.lock().await;
        value.timestamp = heartbeat_deadline(Utc::now(), delay);
        let heartbeat_json = serde_json::to_string(value).map_err(|e| e.to_string())?;
        let start_key = self.key(task_id, "start");
        let heartbeat_key = self.key(task_id, "heartbeat");

        cas_then_set(
            &mut *con,
            &start_key,
            expected_start_json,
            &[(&heartbeat_key, &heartbeat_json)],
        )
        .await
    }

    /// Get a single value for an already fully-qualified key (see [`Self::key`]).
    async fn _get_json_value(&self, full_key: &str) -> Result<Option<TaskValue>, String> {
        let mut con = self.connection.lock().await;

        let results: (Option<String>, i32) = redis::pipe()
            .get(full_key)
            .expire(full_key, EXPIRATION_TIME)
            .query_async(&mut *con)
            .await
            .map_err(|e| e.to_string())?;

        results
            .0
            .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
            .transpose()
    }

    /// Get multiple values for a given list of keys.
    async fn get_json_values(&self, keys: &[String]) -> Result<Vec<Option<TaskValue>>, String> {
        let mut con = self.connection.lock().await;

        // Use MGET to fetch all keys in one go
        let values: Vec<Option<String>> = redis::cmd("MGET")
            .arg(keys)
            .query_async(&mut *con)
            .await
            .map_err(|e| e.to_string())?;
        // Reset expiration for all keys that exist
        for key in keys {
            let exists: bool = redis::cmd("EXISTS")
                .arg(key)
                .query_async(&mut *con)
                .await
                .map_err(|e| e.to_string())?;

            if exists {
                redis::cmd("EXPIRE")
                    .arg(key)
                    .arg(EXPIRATION_TIME)
                    .query_async::<bool>(&mut *con)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }

        // Deserialize each value into an Option<TaskValue>
        values
            .into_iter()
            .map(|value| {
                value
                    .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
                    .transpose()
            })
            .collect()
    }

    fn key(&self, task_id: &str, suffix: &str) -> String {
        // NOTE:
        // The key_prefix is used in cloud dev envs to namespace keys to
        // developer environments because a shared redis cluster is used
        //
        // keys use task_id as the hash tag for cluster-wise multi-key operations:
        // https://redis.io/docs/latest/operate/oss_and_stack/reference/cluster-spec/#hash-tags
        format!(
            "{}fs-orc:{}:{}:{}/node/{{{}}}/{}",
            self.key_prefix, self.account_id, self.environment_id, self.project_id, task_id, suffix,
        )
    }
}

/// CAS helper shared by every write that's gated on a *different* key than
/// the one(s) being written: reads `cas_key`, and only if its value equals
/// `expected_json` applies `writes` (each becomes `SET key value EX
/// EXPIRATION_TIME`) atomically, returning whether the CAS succeeded.
///
/// Used by `set_heartbeat_value` and `stop_task`, both of which gate a write
/// on the task's `start` key still matching the value its owner acquired the
/// lock with. Not used by `try_start_task` or `try_cleanup_task`: both of
/// those CAS against the very key they then write (or delete), a different
/// shape than "check one key, write others".
async fn cas_then_set<C: ConnectionLike>(
    con: &mut C,
    cas_key: &str,
    expected_json: &str,
    writes: &[(&str, &str)],
) -> Result<bool, String> {
    let mut lua = String::from("if redis.call('GET', KEYS[1]) == ARGV[1] then\n");
    for i in 0..writes.len() {
        let idx = i + 2;
        lua.push_str(&format!(
            "redis.call('SET', KEYS[{idx}], ARGV[{idx}], 'EX', {EXPIRATION_TIME})\n"
        ));
    }
    lua.push_str("return 1\nelse\nreturn 0\nend");

    let script = redis::Script::new(&lua);
    let mut invocation = script.prepare_invoke();
    invocation.key(cas_key).arg(expected_json);
    for (write_key, write_value) in writes {
        invocation.key(*write_key).arg(*write_value);
    }

    let result: i32 = invocation
        .invoke_async(con)
        .await
        .map_err(|e| e.to_string())?;

    Ok(result == 1)
}

/// Deadline a heartbeat write promises: the next tick, plus extra slack so a
/// slow write under connection contention isn't mistaken for a dead holder.
fn heartbeat_deadline(now: chrono::DateTime<Utc>, delay: Duration) -> chrono::DateTime<Utc> {
    let contention_buffer = Duration::microseconds(delay.num_microseconds().unwrap() / 4);
    now + delay + contention_buffer
}

/// How long a failed heartbeat write waits before retrying. Independent of
/// the success-path backoff (`delay`): that backoff can grow to MAX_DELAY,
/// and retrying at that pace after a failure could land after the deadline
/// our last successful write promised, letting a waiter think we're dead.
const HEARTBEAT_RETRY_DELAY: Duration = Duration::seconds(1);

/// How long to sleep before the next heartbeat attempt. A failure always
/// retries at the short, fixed `HEARTBEAT_RETRY_DELAY`, never at `delay`
/// itself, since `delay` may already have backed off well past what a
/// waiter will tolerate.
fn heartbeat_sleep_duration(delay: Duration, write_succeeded: bool) -> Duration {
    if write_succeeded {
        delay
    } else {
        HEARTBEAT_RETRY_DELAY
    }
}

/// The backoff for the *next* attempt. Only grows after a success; a
/// failure leaves it untouched so the retry doesn't inherit a backoff that
/// has already grown past what the last successful write promised.
fn next_heartbeat_delay(delay: Duration, write_succeeded: bool) -> Duration {
    if !write_succeeded {
        return delay;
    }
    let max = MAX_DELAY.num_microseconds().unwrap();
    let cur = delay.num_microseconds().unwrap();
    Duration::microseconds((cur * BACKOFF_FACTOR).min(max))
}

/// Whether this write's outcome is worth a log line: only the edges of a
/// failure streak (the first failure, and the recovery after one), not every
/// retry in between. A sustained Redis outage would otherwise log once per
/// second per held lock for as long as it lasts.
fn should_emit_heartbeat_warning(was_failing: bool, write_succeeded: bool) -> bool {
    write_succeeded == was_failing
}

#[async_trait]
impl<T: ConnectionLike + Sync + Send + 'static> TaskCache for TaskCacheRedis<T> {
    async fn check_connection(&self) -> Result<(), String> {
        let mut con = self.connection.lock().await;
        redis::cmd("PING")
            .query_async::<String>(&mut *con)
            .await
            .map_err(|e| format!("Failed to connect to Redis: {e}"))?;
        Ok(())
    }

    async fn try_start_task(
        &self,
        key: &str,
        new_value: &TaskValue,
        old_value: &Option<TaskValue>,
    ) -> Result<bool, String> {
        let key = self.key(key, "start");
        let expected_value = old_value
            .as_ref()
            .map(|v| serde_json::to_string(v).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_else(|| "null".to_string());

        let lua = format!(
            "
            local current = redis.call('GET', KEYS[1])
            if current == false or current == ARGV[1] then
                redis.call('SET', KEYS[1], ARGV[2], 'EX', {EXPIRATION_TIME})
                return 1
            else
                return 0
            end
        "
        );

        let script = redis::Script::new(&lua);
        let result: i32 = script
            .key(&key)
            .arg(expected_value)
            .arg(serde_json::to_string(&new_value).map_err(|e| e.to_string())?)
            .invoke_async(&mut *self.connection.lock().await)
            .await
            .map_err(|e| e.to_string())?;

        Ok(result == 1)
    }

    async fn stop_task(
        &self,
        key: &str,
        value: &TaskValue,
        info: &crate::task_cache::TaskInfo,
        expected_start: &TaskValue,
    ) -> Result<(), String> {
        let start_key = self.key(key, "start");
        let stop_key = self.key(key, "stop");
        let info_key = self.key(key, "info");

        let expected_start_json =
            serde_json::to_string(expected_start).map_err(|e| e.to_string())?;
        let stop_json = serde_json::to_string(value).map_err(|e| e.to_string())?;
        let info_json = serde_json::to_string(info).map_err(|e| e.to_string())?;

        let wrote = cas_then_set(
            &mut *self.connection.lock().await,
            &start_key,
            &expected_start_json,
            &[(&stop_key, &stop_json), (&info_key, &info_json)],
        )
        .await?;

        if !wrote {
            // Another owner has since reclaimed this task; our completion is
            // stale and would have overwritten theirs.
            emit_warn_log_message(
                ErrorCode::StateStopRejected,
                format!(
                    "Discarded stale completion for task {key}: lock was reclaimed by another owner"
                ),
            );
        }

        Ok(())
    }

    async fn get_task_state(&self, task_id: &str) -> Result<(TaskState, TaskValues), String> {
        let keys = [
            self.key(task_id, "start"),
            self.key(task_id, "stop"),
            self.key(task_id, "heartbeat"),
        ];
        let values = self.get_json_values(&keys).await?;
        assert!(values.len() == 3, "Expected 3 values, got {}", values.len());

        let raw_start = values[0].as_ref();
        let raw_stop = values[1].as_ref();
        let raw_heartbeat = values[2].as_ref();

        let now = Utc::now();

        // Determine the expected owner_id from the start entry
        let expected_owner = match raw_start {
            Some(start_val) => &start_val.owner_id,
            None => {
                // If there's no start entry, we can't determine the owner
                return Ok((TaskState::NotStarted, (None, None, None)));
            }
        };

        // Validate owner_id for each entry
        // TODO: redundant validation on raw_start
        let start = raw_start
            .and_then(|val| val.validate_owner(expected_owner))
            .cloned();
        let stop = raw_stop
            .and_then(|val| val.validate_owner(expected_owner))
            .cloned();
        // If owner != expected_owner, then heartbeat is None
        let heartbeat = raw_heartbeat
            .and_then(|val| val.validate_owner(expected_owner))
            .cloned();

        let state = match (start.as_ref(), heartbeat.as_ref(), stop.as_ref()) {
            // Task has not started
            (None, _, _) => TaskState::NotStarted,

            // Task has started and completed
            (Some(start_val), maybe_hb_val, Some(stop_val)) => {
                if stop_val.timestamp >= start_val.timestamp {
                    TaskState::Completed
                } else if let Some(hb_val) = maybe_hb_val {
                    if now > hb_val.timestamp + BUFFER_TIME {
                        TaskState::Stale
                    } else {
                        TaskState::InProgress
                    }
                } else {
                    // If heartbeat is not set, then we should not be in progress
                    TaskState::Stale
                }
            }

            // Task has started, has heartbeat, but has not finished
            (Some(_), Some(hb_val), None) => {
                if now > hb_val.timestamp + BUFFER_TIME {
                    TaskState::Stale
                } else {
                    TaskState::InProgress
                }
            }

            // Task has started, no heartbeat
            (Some(start_val), None, None) => {
                if now > start_val.timestamp + BUFFER_TIME {
                    TaskState::Stale
                } else {
                    TaskState::InProgress
                }
            }
        };

        // If the state is Stale, we should include the raw stop value if it exists
        let task_values = if state == TaskState::Stale && raw_stop.is_some() {
            // Use the raw stop value even if owner validation failed previously
            let raw_stop_cloned = raw_stop.cloned();
            (start, heartbeat, raw_stop_cloned)
        } else {
            (start, heartbeat, stop)
        };

        Ok((state, task_values))
    }

    async fn start_heartbeat(
        self: Arc<Self>,
        key: &str,
        value: &TaskValue,
        start_value: &TaskValue,
        stop_signal: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let task_id = key.to_string();
        let mut value = value.clone();
        let expected_start_json = serde_json::to_string(start_value).map_err(|e| e.to_string())?;
        let task_cache = self;

        tokio::spawn(async move {
            let mut delay = INITIAL_DELAY;
            let mut was_failing = false;

            loop {
                if stop_signal.load(Ordering::Relaxed) {
                    break;
                }

                let write_result = task_cache
                    .set_heartbeat_value(&task_id, &mut value, delay, &expected_start_json)
                    .await;
                let write_succeeded = matches!(&write_result, Ok(true));

                match &write_result {
                    Ok(true) => {
                        if should_emit_heartbeat_warning(was_failing, true) {
                            emit_info_log_message("Heartbeat writes recovered");
                        }
                    }
                    Ok(false) => {
                        // Another owner reclaimed this task; further writes would
                        // only fight over a lock we no longer hold.
                        emit_warn_log_message(
                            ErrorCode::StateHeartbeatReclaimed,
                            "Heartbeat stopped: this worker's lock was reclaimed by another owner",
                        );
                    }
                    Err(err) => {
                        if should_emit_heartbeat_warning(was_failing, false) {
                            emit_warn_log_message(
                                ErrorCode::StateHeartbeatFailed,
                                format!(
                                    "Heartbeat write failed, retrying silently until it recovers: {err}"
                                ),
                            );
                        }
                    }
                }
                was_failing = !write_succeeded;

                if matches!(&write_result, Ok(false)) {
                    break;
                }

                tokio::time::sleep(TokioDuration::from_micros(
                    heartbeat_sleep_duration(delay, write_succeeded)
                        .num_microseconds()
                        .unwrap() as u64,
                ))
                .await;
                delay = next_heartbeat_delay(delay, write_succeeded);
            }
        });

        Ok(())
    }

    /// get the heartbeat value for the task.
    async fn get_heartbeat(&self, key: &str) -> Result<TaskValue, String> {
        let key = self.key(key, "heartbeat");
        self._get_json_value(&key)
            .await?
            .ok_or_else(|| format!("No heartbeat value found for key: {key}"))
    }

    /// get the heartbeat value for the task.
    async fn get_start(&self, key: &str) -> Result<TaskValue, String> {
        let key = self.key(key, "start");
        self._get_json_value(&key)
            .await?
            .ok_or_else(|| format!("No start value found for key: {key}"))
    }
    async fn get_stop(&self, key: &str) -> Result<TaskValue, String> {
        let key = self.key(key, "stop");
        self._get_json_value(&key)
            .await?
            .ok_or_else(|| format!("No stop value found for key: {key}"))
    }

    async fn stop_heartbeat(&self, stop_signal: Arc<AtomicBool>) {
        //simply stop the heartbeat
        stop_signal.store(true, Ordering::Relaxed);
    }

    async fn try_cleanup_task(
        &self,
        key: &str,
        old_value: &Option<TaskValue>,
    ) -> Result<(), String> {
        let mut con = self.connection.lock().await;
        let key = self.key(key, "start");

        let serialized_value = serde_json::to_string(&old_value).map_err(|e| e.to_string())?;

        let lua = r#"
            if redis.call('GET', KEYS[1]) == ARGV[1] then
                redis.call('DEL', KEYS[1])
                return 1
            else
                return 0
            end
        "#;

        let result: i32 = redis::Script::new(lua)
            .key(&key)
            .arg(serialized_value)
            .invoke_async(&mut *con)
            .await
            .map_err(|e| e.to_string())?;

        if result != 1 {
            // This is for tests only, so using tracing directly is fine
            tracing::debug!("old value updated during clean up")
        }
        Ok(())
    }

    async fn get_info(&self, key: &str) -> Result<Option<crate::task_cache::TaskInfo>, String> {
        let mut con = self.connection.lock().await;
        let full_key = self.key(key, "info");

        let results: (Option<String>, i32) = redis::pipe()
            .get(&full_key)
            .expire(&full_key, EXPIRATION_TIME)
            .query_async(&mut *con)
            .await
            .map_err(|e| e.to_string())?;

        results
            .0
            .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
            .transpose()
    }

    async fn invalidate_task(&self, key: &str) -> Result<(), String> {
        let mut con = self.connection.lock().await;

        // Delete all four keys atomically
        redis::pipe()
            .atomic()
            .del(self.key(key, "start"))
            .del(self.key(key, "stop"))
            .del(self.key(key, "heartbeat"))
            .del(self.key(key, "info"))
            .query_async::<()>(&mut *con)
            .await
            .map_err(|e| format!("Failed to invalidate task {}: {}", key, e))
    }

    async fn clear_all(&self) -> Result<(), String> {
        let mut con = self.connection.lock().await;

        // Build the pattern to match all keys for this project/environment/account
        let pattern = format!(
            "{}fs-orc:{}:{}:{}/*",
            self.key_prefix, self.account_id, self.environment_id, self.project_id
        );

        // Use SCAN to find all matching keys (safer than KEYS for large datasets)
        let mut cursor = 0u64;
        let mut all_keys: Vec<String> = Vec::new();

        loop {
            let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(1000)
                .query_async(&mut *con)
                .await
                .map_err(|e| format!("Failed to scan keys: {e}"))?;

            all_keys.extend(keys);
            cursor = new_cursor;

            if cursor == 0 {
                break;
            }
        }

        // Delete all found keys
        if !all_keys.is_empty() {
            redis::cmd("DEL")
                .arg(&all_keys)
                .query_async::<()>(&mut *con)
                .await
                .map_err(|e| format!("Failed to delete keys: {e}"))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_deadline_adds_a_quarter_of_the_delay_as_buffer() {
        let now = Utc::now();
        let deadline = heartbeat_deadline(now, Duration::seconds(8));
        assert_eq!(deadline - now, Duration::seconds(10));
    }

    #[test]
    fn heartbeat_deadline_buffer_grows_with_backoff() {
        let now = Utc::now();
        let short_margin = heartbeat_deadline(now, INITIAL_DELAY) - now - INITIAL_DELAY;
        let long_margin = heartbeat_deadline(now, MAX_DELAY) - now - MAX_DELAY;
        assert!(
            long_margin > short_margin,
            "a longer backoff should tolerate more jitter, not the same fixed buffer"
        );
    }

    #[test]
    fn failed_write_retries_fast_regardless_of_backoff() {
        assert_eq!(
            heartbeat_sleep_duration(MAX_DELAY, false),
            HEARTBEAT_RETRY_DELAY
        );
    }

    #[test]
    fn successful_write_sleeps_the_current_backoff() {
        assert_eq!(heartbeat_sleep_duration(INITIAL_DELAY, true), INITIAL_DELAY);
    }

    #[test]
    fn failed_write_does_not_grow_the_backoff() {
        assert_eq!(next_heartbeat_delay(INITIAL_DELAY, false), INITIAL_DELAY);
    }

    #[test]
    fn successful_write_doubles_the_backoff_up_to_the_max() {
        let doubled =
            Duration::microseconds(INITIAL_DELAY.num_microseconds().unwrap() * BACKOFF_FACTOR);
        assert_eq!(next_heartbeat_delay(INITIAL_DELAY, true), doubled);
        assert_eq!(next_heartbeat_delay(MAX_DELAY, true), MAX_DELAY);
    }

    #[test]
    fn first_failure_after_a_healthy_streak_warns() {
        assert!(should_emit_heartbeat_warning(false, false));
    }

    #[test]
    fn repeated_failures_do_not_warn_again() {
        assert!(!should_emit_heartbeat_warning(true, false));
    }

    #[test]
    fn recovery_after_a_failure_streak_warns() {
        assert!(should_emit_heartbeat_warning(true, true));
    }

    #[test]
    fn repeated_successes_do_not_warn() {
        assert!(!should_emit_heartbeat_warning(false, true));
    }
}
